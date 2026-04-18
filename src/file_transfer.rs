/// File transfer protocol for Hive.
///
/// Provides chunked binary transfer between peers.  Files are split into
/// fixed-size chunks, each sent as a separate wire message.  The receiver
/// reassembles chunks and verifies a SHA-256 hash on completion.
///
/// This is a core Hive capability, not Hunter-specific.  It can be used
/// for firmware images, configuration bundles, binary updates, or any
/// large payload that doesn't fit the property-sync model.
///
/// # Wire protocol
///
/// Four new message types are added to the Hive protocol:
///
/// - `FILE_HEADER` (0x14): opens a transfer session with metadata
/// - `FILE_CHUNK`  (0x15): one chunk of file data
/// - `FILE_COMPLETE` (0x16): signals all chunks have been sent
/// - `FILE_CANCEL` (0x17): aborts a transfer in progress
///
/// Each transfer is identified by a `transfer_id` (u32) that is unique
/// per sender.  The receiver uses this ID to associate chunks with the
/// correct in-progress transfer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use sha2::{Sha256, Digest};
use tracing::{debug, info, warn, error};

// -- Wire protocol constants --

pub const FILE_HEADER: u8 = 0x14;
pub const FILE_CHUNK: u8 = 0x15;
pub const FILE_COMPLETE: u8 = 0x16;
pub const FILE_CANCEL: u8 = 0x17;

/// Default chunk size: 64 KB.  Small enough to interleave with property
/// updates on the same connection, large enough to avoid excessive framing
/// overhead.
pub const DEFAULT_CHUNK_SIZE: u32 = 64 * 1024;

/// How long to wait for the next chunk before considering a transfer stalled.
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30);

// -- Transfer ID generator --

static NEXT_TRANSFER_ID: AtomicU32 = AtomicU32::new(1);

fn next_transfer_id() -> u32 {
    NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed)
}

// -- Sender-side: encoding --

/// Metadata about a file being sent.  Returned by `encode_file_header()`
/// for the caller to track the transfer.
#[derive(Clone, Debug)]
pub struct OutboundTransfer {
    pub transfer_id: u32,
    pub filename: String,
    pub total_size: u64,
    pub sha256: [u8; 32],
    pub chunk_size: u32,
    pub total_chunks: u32,
}

/// Encode a FILE_HEADER message.
///
/// Returns the wire bytes and an `OutboundTransfer` descriptor.
pub fn encode_file_header(filename: &str, data: &[u8], chunk_size: u32) -> (Bytes, OutboundTransfer) {
    let transfer_id = next_transfer_id();
    let total_size = data.len() as u64;

    // Compute SHA-256
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash: [u8; 32] = hasher.finalize().into();

    let total_chunks = ((total_size + chunk_size as u64 - 1) / chunk_size as u64) as u32;

    let name_bytes = filename.as_bytes();
    let msg_len = 1 + 4 + 2 + name_bytes.len() + 8 + 32 + 4;
    let mut buf = BytesMut::with_capacity(msg_len);

    buf.put_u8(FILE_HEADER);
    buf.put_u32(transfer_id);
    buf.put_u16(name_bytes.len() as u16);
    buf.put_slice(name_bytes);
    buf.put_u64(total_size);
    buf.put_slice(&hash);
    buf.put_u32(chunk_size);

    let transfer = OutboundTransfer {
        transfer_id,
        filename: filename.to_string(),
        total_size,
        sha256: hash,
        chunk_size,
        total_chunks,
    };

    (buf.freeze(), transfer)
}

/// Encode a single FILE_CHUNK message.
pub fn encode_file_chunk(transfer_id: u32, chunk_index: u32, data: &[u8]) -> Bytes {
    let msg_len = 1 + 4 + 4 + 4 + data.len();
    let mut buf = BytesMut::with_capacity(msg_len);

    buf.put_u8(FILE_CHUNK);
    buf.put_u32(transfer_id);
    buf.put_u32(chunk_index);
    buf.put_u32(data.len() as u32);
    buf.put_slice(data);

    buf.freeze()
}

/// Encode a FILE_COMPLETE message.
pub fn encode_file_complete(transfer_id: u32) -> Bytes {
    let mut buf = BytesMut::with_capacity(5);
    buf.put_u8(FILE_COMPLETE);
    buf.put_u32(transfer_id);
    buf.freeze()
}

/// Encode a FILE_CANCEL message.
pub fn encode_file_cancel(transfer_id: u32, reason: &str) -> Bytes {
    let reason_bytes = reason.as_bytes();
    let mut buf = BytesMut::with_capacity(1 + 4 + 2 + reason_bytes.len());
    buf.put_u8(FILE_CANCEL);
    buf.put_u32(transfer_id);
    buf.put_u16(reason_bytes.len() as u16);
    buf.put_slice(reason_bytes);
    buf.freeze()
}

/// Split file data into chunk messages.  Returns an iterator of encoded
/// chunk messages ready to send on the wire.
pub fn encode_all_chunks(transfer_id: u32, data: &[u8], chunk_size: u32) -> Vec<Bytes> {
    let cs = chunk_size as usize;
    data.chunks(cs)
        .enumerate()
        .map(|(i, chunk)| encode_file_chunk(transfer_id, i as u32, chunk))
        .collect()
}

// -- Receiver-side: decoding and reassembly --

/// State of an in-progress inbound file transfer.
pub struct InboundTransfer {
    pub transfer_id: u32,
    pub filename: String,
    pub total_size: u64,
    pub expected_sha256: [u8; 32],
    pub chunk_size: u32,
    pub total_chunks: u32,
    /// Received chunks indexed by chunk_index.
    chunks: HashMap<u32, Vec<u8>>,
    /// Total bytes received so far.
    pub bytes_received: u64,
    /// When the last chunk was received (for timeout detection).
    pub last_activity: Instant,
    /// Address of the peer sending this transfer.
    pub from_peer: String,
}

impl InboundTransfer {
    /// Store a received chunk.  Returns true if this was a new chunk
    /// (not a duplicate).
    pub fn receive_chunk(&mut self, chunk_index: u32, data: Vec<u8>) -> bool {
        self.last_activity = Instant::now();
        let len = data.len() as u64;
        if self.chunks.contains_key(&chunk_index) {
            debug!("duplicate chunk {} for transfer {}", chunk_index, self.transfer_id);
            return false;
        }
        self.chunks.insert(chunk_index, data);
        self.bytes_received += len;
        true
    }

    /// Check whether all chunks have been received.
    pub fn is_complete(&self) -> bool {
        self.chunks.len() as u32 >= self.total_chunks
    }

    /// Check whether this transfer has timed out.
    pub fn is_timed_out(&self, timeout: Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }

    /// Reassemble all chunks into the complete file data.
    /// Returns None if chunks are missing.
    pub fn reassemble(&self) -> Option<Vec<u8>> {
        let mut result = Vec::with_capacity(self.total_size as usize);
        for i in 0..self.total_chunks {
            match self.chunks.get(&i) {
                Some(chunk) => result.extend_from_slice(chunk),
                None => {
                    warn!(
                        "transfer {}: missing chunk {} of {}",
                        self.transfer_id, i, self.total_chunks
                    );
                    return None;
                }
            }
        }
        Some(result)
    }

    /// Verify the SHA-256 hash of the reassembled data.
    pub fn verify(&self, data: &[u8]) -> bool {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let computed: [u8; 32] = hasher.finalize().into();
        computed == self.expected_sha256
    }
}

/// Tracks all in-progress inbound transfers for a Hive instance.
pub struct FileTransferManager {
    /// Active transfers keyed by (from_address, transfer_id).
    transfers: HashMap<(String, u32), InboundTransfer>,
    /// Timeout for stalled transfers.
    pub timeout: Duration,
}

impl Default for FileTransferManager {
    fn default() -> Self {
        Self {
            transfers: HashMap::new(),
            timeout: TRANSFER_TIMEOUT,
        }
    }
}

/// The result of completing a file transfer.
#[derive(Clone, Debug)]
pub struct ReceivedFile {
    pub filename: String,
    pub data: Vec<u8>,
    pub sha256: [u8; 32],
    pub from_peer: String,
    pub transfer_id: u32,
}

impl FileTransferManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode a FILE_HEADER message and start tracking the transfer.
    pub fn handle_file_header(&mut self, from: &str, msg: &mut Bytes) -> Option<u32> {
        if msg.remaining() < 4 + 2 {
            warn!("FILE_HEADER too short");
            return None;
        }
        let transfer_id = msg.get_u32();
        let filename_len = msg.get_u16() as usize;
        if msg.remaining() < filename_len + 8 + 32 + 4 {
            warn!("FILE_HEADER truncated after filename_len");
            return None;
        }
        let filename = String::from_utf8(msg.slice(..filename_len).to_vec())
            .unwrap_or_else(|_| "unknown".into());
        msg.advance(filename_len);

        let total_size = msg.get_u64();
        let mut sha256 = [0u8; 32];
        msg.copy_to_slice(&mut sha256);
        let chunk_size = msg.get_u32();

        let total_chunks = ((total_size + chunk_size as u64 - 1) / chunk_size as u64) as u32;

        info!(
            "file transfer started: id={}, name='{}', size={}, chunks={}, from={}",
            transfer_id, filename, total_size, total_chunks, from
        );

        let transfer = InboundTransfer {
            transfer_id,
            filename,
            total_size,
            expected_sha256: sha256,
            chunk_size,
            total_chunks,
            chunks: HashMap::new(),
            bytes_received: 0,
            last_activity: Instant::now(),
            from_peer: from.to_string(),
        };

        let key = (from.to_string(), transfer_id);
        self.transfers.insert(key, transfer);
        Some(transfer_id)
    }

    /// Decode a FILE_CHUNK message and store the chunk data.
    pub fn handle_file_chunk(&mut self, from: &str, msg: &mut Bytes) -> bool {
        if msg.remaining() < 4 + 4 + 4 {
            warn!("FILE_CHUNK too short");
            return false;
        }
        let transfer_id = msg.get_u32();
        let chunk_index = msg.get_u32();
        let data_len = msg.get_u32() as usize;

        if msg.remaining() < data_len {
            warn!("FILE_CHUNK data truncated");
            return false;
        }
        let data = msg.slice(..data_len).to_vec();
        msg.advance(data_len);

        let key = (from.to_string(), transfer_id);
        match self.transfers.get_mut(&key) {
            Some(transfer) => {
                let is_new = transfer.receive_chunk(chunk_index, data);
                if is_new {
                    debug!(
                        "transfer {}: chunk {}/{} ({} bytes total)",
                        transfer_id,
                        transfer.chunks.len(),
                        transfer.total_chunks,
                        transfer.bytes_received,
                    );
                }
                is_new
            }
            None => {
                warn!(
                    "received chunk for unknown transfer {} from {}",
                    transfer_id, from
                );
                false
            }
        }
    }

    /// Decode a FILE_COMPLETE message.  If all chunks are present and
    /// the hash verifies, returns the completed file.
    pub fn handle_file_complete(&mut self, from: &str, msg: &mut Bytes) -> Option<ReceivedFile> {
        if msg.remaining() < 4 {
            warn!("FILE_COMPLETE too short");
            return None;
        }
        let transfer_id = msg.get_u32();
        let key = (from.to_string(), transfer_id);

        let transfer = self.transfers.remove(&key)?;

        if !transfer.is_complete() {
            warn!(
                "FILE_COMPLETE for transfer {} but only {}/{} chunks received",
                transfer_id,
                transfer.chunks.len(),
                transfer.total_chunks,
            );
            return None;
        }

        let data = match transfer.reassemble() {
            Some(d) => d,
            None => {
                error!("transfer {}: reassembly failed (missing chunks)", transfer_id);
                return None;
            }
        };

        if !transfer.verify(&data) {
            error!(
                "transfer {}: SHA-256 mismatch for '{}'",
                transfer_id, transfer.filename
            );
            return None;
        }

        info!(
            "file transfer complete: '{}' ({} bytes) from {}",
            transfer.filename, data.len(), from
        );

        Some(ReceivedFile {
            filename: transfer.filename,
            data,
            sha256: transfer.expected_sha256,
            from_peer: from.to_string(),
            transfer_id,
        })
    }

    /// Handle a FILE_CANCEL message.  Removes the in-progress transfer.
    pub fn handle_file_cancel(&mut self, from: &str, msg: &mut Bytes) {
        if msg.remaining() < 4 + 2 {
            warn!("FILE_CANCEL too short");
            return;
        }
        let transfer_id = msg.get_u32();
        let reason_len = msg.get_u16() as usize;
        let reason = if msg.remaining() >= reason_len {
            let r = String::from_utf8(msg.slice(..reason_len).to_vec())
                .unwrap_or_else(|_| "unknown".into());
            msg.advance(reason_len);
            r
        } else {
            "unknown".into()
        };

        let key = (from.to_string(), transfer_id);
        if self.transfers.remove(&key).is_some() {
            info!(
                "file transfer {} cancelled by {}: {}",
                transfer_id, from, reason
            );
        }
    }

    /// Remove any transfers that have timed out.  Returns the IDs of
    /// timed-out transfers so the caller can send FILE_CANCEL responses.
    pub fn cleanup_stale(&mut self) -> Vec<(String, u32)> {
        let timeout = self.timeout;
        let stale: Vec<(String, u32)> = self.transfers
            .iter()
            .filter(|(_, t)| t.is_timed_out(timeout))
            .map(|(k, _)| k.clone())
            .collect();

        for key in &stale {
            if let Some(t) = self.transfers.remove(key) {
                warn!(
                    "file transfer {} from {} timed out ({}/{} chunks)",
                    t.transfer_id, t.from_peer, t.chunks.len(), t.total_chunks
                );
            }
        }
        stale
    }

    /// Number of active transfers.
    pub fn active_count(&self) -> usize {
        self.transfers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let data = b"Hello, this is a test file with enough content to span multiple chunks!";
        let chunk_size = 20;

        // Encode header
        let (header_bytes, transfer) = encode_file_header("test.txt", data, chunk_size);
        assert_eq!(transfer.total_chunks, 4); // 70 bytes / 20 = 3.5 -> 4 chunks

        // Encode chunks
        let chunks = encode_all_chunks(transfer.transfer_id, data, chunk_size);
        assert_eq!(chunks.len(), 4);

        // Encode complete
        let complete = encode_file_complete(transfer.transfer_id);

        // Decode on receiver side
        let mut manager = FileTransferManager::new();

        // Process header
        let mut hdr = header_bytes.clone();
        hdr.advance(1); // skip FILE_HEADER byte (dispatcher already consumed it)
        let tid = manager.handle_file_header("peer-a", &mut hdr);
        assert_eq!(tid, Some(transfer.transfer_id));
        assert_eq!(manager.active_count(), 1);

        // Process chunks
        for chunk_msg in &chunks {
            let mut c = chunk_msg.clone();
            c.advance(1); // skip FILE_CHUNK byte
            manager.handle_file_chunk("peer-a", &mut c);
        }

        // Process complete
        let mut comp = complete.clone();
        comp.advance(1); // skip FILE_COMPLETE byte
        let result = manager.handle_file_complete("peer-a", &mut comp);

        assert!(result.is_some());
        let received = result.unwrap();
        assert_eq!(received.filename, "test.txt");
        assert_eq!(received.data, data);
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn hash_mismatch_detected() {
        let data = b"original content";
        let chunk_size = 100;

        let (header_bytes, transfer) = encode_file_header("bad.bin", data, chunk_size);

        let mut manager = FileTransferManager::new();

        let mut hdr = header_bytes.clone();
        hdr.advance(1);
        manager.handle_file_header("peer-b", &mut hdr);

        // Send corrupted data
        let corrupted = b"CORRUPTED content";
        let bad_chunk = encode_file_chunk(transfer.transfer_id, 0, corrupted);
        let mut c = bad_chunk.clone();
        c.advance(1);
        manager.handle_file_chunk("peer-b", &mut c);

        let complete = encode_file_complete(transfer.transfer_id);
        let mut comp = complete.clone();
        comp.advance(1);
        let result = manager.handle_file_complete("peer-b", &mut comp);

        // Should be None because hash mismatch
        assert!(result.is_none());
    }

    #[test]
    fn cancel_removes_transfer() {
        let data = b"some file";
        let (header_bytes, transfer) = encode_file_header("cancel.bin", data, 100);

        let mut manager = FileTransferManager::new();

        let mut hdr = header_bytes.clone();
        hdr.advance(1);
        manager.handle_file_header("peer-c", &mut hdr);
        assert_eq!(manager.active_count(), 1);

        let cancel = encode_file_cancel(transfer.transfer_id, "user requested");
        let mut c = cancel.clone();
        c.advance(1);
        manager.handle_file_cancel("peer-c", &mut c);
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn timeout_cleanup() {
        let data = b"stale";
        let (header_bytes, _transfer) = encode_file_header("stale.bin", data, 100);

        let mut manager = FileTransferManager::new();
        manager.timeout = Duration::from_millis(0); // immediate timeout

        let mut hdr = header_bytes.clone();
        hdr.advance(1);
        manager.handle_file_header("peer-d", &mut hdr);

        // Sleeping to ensure timeout fires
        std::thread::sleep(Duration::from_millis(10));

        let stale = manager.cleanup_stale();
        assert_eq!(stale.len(), 1);
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn duplicate_chunk_ignored() {
        let data = b"duplicate test data with padding";
        let chunk_size = 10;

        let (header_bytes, transfer) = encode_file_header("dup.bin", data, chunk_size);

        let mut manager = FileTransferManager::new();
        let mut hdr = header_bytes.clone();
        hdr.advance(1);
        manager.handle_file_header("peer-e", &mut hdr);

        // Send chunk 0 twice
        let chunk0 = encode_file_chunk(transfer.transfer_id, 0, &data[..10]);
        let mut c1 = chunk0.clone();
        c1.advance(1);
        assert!(manager.handle_file_chunk("peer-e", &mut c1));

        let mut c2 = chunk0.clone();
        c2.advance(1);
        assert!(!manager.handle_file_chunk("peer-e", &mut c2)); // duplicate
    }
}
