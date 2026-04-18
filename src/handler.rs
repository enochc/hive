// use std::sync::mpmc::SendError;
use bytes::{BufMut, BytesMut};
use futures::channel::mpsc::{SendError, UnboundedSender};
use crate::futures::SinkExt;
#[allow(unused_imports)]
use log::{debug, error, info};
use crate::file_transfer::{self, DEFAULT_CHUNK_SIZE};
use crate::hive::{PEER_MESSAGE, PEER_MESSAGE_DIV};
use crate::peer::SocketEvent;
use crate::property::{Property, property_to_bytes, PropertyValue, PropertyType};

#[derive(Clone)]
pub struct Handler {
    pub (crate) sender: UnboundedSender<SocketEvent>,
    pub from_name: String,
}

impl Handler {

    pub async fn send_property_value(&mut self, prop_name: &str, prop_value: Option<&PropertyType>) {
        let p = Property::from_toml(prop_name, prop_value);
        self.send_property(p).await;
    }

    pub fn set_str(&self, id:&u64, value:&str){
        let _prop = Property::from_value(id, value.into());
    }

    // This is basically a set_property method, however, Hive itself does not deal directly with property values
    // It only propagates value change notifications, hence the name
    pub async fn set_property(&mut self, prop_name:&str, prop_value:Option<&PropertyValue>){
        let p = Property::from_toml(prop_name, Some(&prop_value.unwrap().toml()));
        self.send_property(p).await;
    }

    pub async fn send_property(&mut self, property:Property){

        let message = property_to_bytes(&property, true);
        debug!("... send_property: {:?}, {:?}", message, self.sender.is_closed());
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg: message
        };

        self.sender.send(socket_event).await.expect("Failed to send property");

    }

    pub async fn delete_property(&mut self, name: &str){
        let p = Property::from_name(name, None);
        let message = property_to_bytes(&p, true);
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg: message
        };
        self.sender.send(socket_event).await.unwrap();
    }

    pub async fn send_to_peer(&mut self, peer_name:&str, msg:&str) -> Result<(), SendError>{
        let mut bytes = BytesMut::with_capacity(peer_name.len()+msg.len()+2);
        bytes.put_u8(PEER_MESSAGE);
        bytes.put_slice(peer_name.as_bytes());
        bytes.put_u8(PEER_MESSAGE_DIV.as_bytes()[0]);
        bytes.put_slice(msg.as_bytes());
        debug!(".... send to peer: {:?}", bytes);
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg: bytes.freeze()
        };
        self.sender.send(socket_event).await //.expect("failed to send to peer");

    }

    pub async fn hangup(&mut self) {
        self.sender.send(SocketEvent::Hangup{from:String::from("")}).await
            .expect("failed to hangup");
    }

    /// Send a file to a specific peer.
    ///
    /// The file is chunked and sent as a sequence of FILE_HEADER,
    /// FILE_CHUNK, and FILE_COMPLETE messages.  The peer name is
    /// encoded as the first segment of a PEER_MESSAGE so the Hive
    /// event loop routes each chunk to the correct peer.
    pub async fn send_file(&mut self, peer_name: &str, filename: &str, data: &[u8]) {
        let chunk_size = DEFAULT_CHUNK_SIZE;
        let (header_msg, transfer) = file_transfer::encode_file_header(filename, data, chunk_size);

        debug!(
            "sending file '{}' ({} bytes, {} chunks) to peer '{}'",
            filename, data.len(), transfer.total_chunks, peer_name
        );

        // Send header
        self.send_raw(header_msg).await;

        // Send chunks
        let chunks = file_transfer::encode_all_chunks(transfer.transfer_id, data, chunk_size);
        for chunk in chunks {
            self.send_raw(chunk).await;
        }

        // Send complete
        let complete = file_transfer::encode_file_complete(transfer.transfer_id);
        self.send_raw(complete).await;
    }

    /// Send a raw pre-encoded message through the Hive event loop.
    async fn send_raw(&mut self, msg: bytes::Bytes) {
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg,
        };
        self.sender.send(socket_event).await.expect("Failed to send raw message");
    }

}

