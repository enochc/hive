use std::collections::HashMap;
use std::sync::Arc;

use tokio::{
    io::BufReader,
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    sync::Mutex as TokioMutex,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use base64::{Engine as _, engine::general_purpose};
use bytes::{Bytes, BytesMut};
use crate::SinkExt;
use futures::channel::mpsc::UnboundedSender;
#[allow(unused_imports)]
use log::{debug, info, trace};
use sha1::{Digest, Sha1};
use websocket_codec::{Message, MessageCodec, Opcode};

use crate::hive::Result;
use crate::peer::SocketEvent;

const SEC_KEY: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Debug)]
pub struct WebSock {
    headers: HashMap<String, String>,
    write_stream: Arc<TokioMutex<OwnedWriteHalf>>,
    address: String,
}

impl WebSock {
    pub fn address(&self) -> String {
        self.address.clone()
    }
    
    pub async fn from_stream(
        mut reader: BufReader<OwnedReadHalf>,
        mut write_half: OwnedWriteHalf,
        address: String,
        sender: UnboundedSender<SocketEvent>,
    ) -> Result<WebSock> {
        let mut count = 0;
        let mut str: String;
        let mut headers = HashMap::new();

        loop {
            count += 1;
            str = "".to_string();
            let m = reader.read_line(&mut str).await?;
            let parts = str.split(":").collect::<Vec<_>>();
            debug!("... HEADER:: {:?}", str);
            headers.insert(parts.first().unwrap().to_string(), parts.last().unwrap().trim().to_string());

            let done = parts.len() < 2;
            if (count > 30 || m == 0 || done) && headers.get("Upgrade") == Some(&"websocket".to_string()) {
                let sec_resp = WebSock::get_sec(headers.get("Sec-WebSocket-Key").unwrap());
                let mut response_string = "HTTP/1.1 101 Switching Protocols\r\n".to_string();
                response_string.push_str("Upgrade: websocket\r\n");
                response_string.push_str("Connection: Upgrade\r\n");
                response_string.push_str("Sec-WebSocket-Protocol: hive\r\n");
                response_string.push_str(&format!("Sec-WebSocket-Accept: {}\r\n\r\n", sec_resp));

                write_half.write_all(response_string.as_bytes()).await?;
                write_half.flush().await?;

                let from = address.clone();
                let addr = address.clone();
                tokio::spawn(async move {
                    read_loop(&sender, reader, &addr).await;
                    debug!("<<< READ DONE, send hangup");
                    let _ = sender.send(SocketEvent::Hangup { from }).await;
                    debug!("<<SENT");
                });

                break;
            }
        }

        Ok(WebSock {
            headers,
            write_stream: Arc::new(TokioMutex::new(write_half)),
            address,
        })
    }

    fn get_sec(key: &str) -> String {
        let mut kk = String::with_capacity(key.len() + 38);
        kk.push_str(key);
        kk.push_str(SEC_KEY);
        let hash = Sha1::digest(kk.as_bytes());
        general_purpose::STANDARD.encode(hash)
    }

    pub async fn send_message(&self, msg: Bytes) -> Result<()> {
        let message = Message::binary(msg);
        let mut bytes = BytesMut::new();
        MessageCodec::server().encode(&message, &mut bytes)?;

        let mut stream = self.write_stream.lock().await;
        stream.write_all(bytes.as_ref()).await?;
        debug!("really send:: {:?}", message);
        stream.flush().await?;
        Ok(())
    }
}

async fn read_loop(
    sender: &UnboundedSender<SocketEvent>,
    mut reader: BufReader<OwnedReadHalf>,
    from_string: &String,
) {
    let mut is_running = true;
    while is_running {
        let mut buffer = [0u8; 128];
        let read = match reader.read(&mut buffer).await {
            Ok(n) => n,
            Err(e) => {
                debug!("read error: {:?}", e);
                break;
            }
        };
        
        let mut bytes = BytesMut::from(buffer.as_ref());
        let mut mc = MessageCodec::server();
        debug!("reading:: {:?}", read);
        
        if read == 0 {
            debug!("read 0 bytes, connection closed");
            is_running = false;
            continue;
        }
        
        let res = mc.decode(&mut bytes);
        debug!("DECODED: {:?}, from {:?}", res, from_string);
        match res {
            Ok(t) => {
                if let Some(message) = t {
                    match message.opcode() {
                        Opcode::Binary | Opcode::Text => {
                            debug!("OPCODE {:?}: {:?}", message.opcode(), message);
                            let received = message.data().to_owned();
                            let evnt = SocketEvent::Message {
                                from: from_string.clone(),
                                msg: received,
                            };
                            let _ = sender.send(evnt).await;
                        }
                        Opcode::Close => {
                            debug!("CLOSE socket");
                            is_running = false;
                        }
                        o => {
                            debug!("Unexpected opcode: {:?}", o);
                        }
                    }
                }
            }
            Err(e) => {
                debug!("ERROR: {:?}", e);
                is_running = false;
            }
        }
    }

    debug!("Web sock run done!");
}
