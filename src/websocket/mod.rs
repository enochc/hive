use std::collections::HashMap;

use async_std::{
    io::BufReader,
    net::TcpStream,
    task,
};
use base64;
use bytes::{Bytes, BytesMut};
use futures::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, SinkExt};
use futures::channel::mpsc::UnboundedSender;
#[allow(unused_imports)]
use log::{debug, info, trace};
use sha1::{Digest, Sha1};
use tokio_util::codec::Encoder;
use websocket_codec::{Message, MessageCodec, Opcode};

use crate::hive::Result;
use crate::peer::SocketEvent;

const SEC_KEY: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Debug, Clone)]
pub struct WebSock {
    _headers: HashMap<String, String>,
    stream: TcpStream,
}

impl WebSock {
    pub fn address(&self)-> String {
        return self.stream.peer_addr()
            .map_or_else(|_|{"... empty ...".into()}, |x|{x.to_string()});
    }
    pub async fn from_stream(mut reader: BufReader<TcpStream>,
                             mut stream: TcpStream,
                             mut sender: UnboundedSender<SocketEvent>) -> Result<WebSock> {
        let mut count = 0;
        let mut str: String;
        let mut headers = HashMap::new();

        loop {
            count += 1;
            str = "".to_string();
            let m = AsyncBufReadExt::read_line(&mut reader, &mut str).await?;
            let parts = str.split(":").collect::<Vec<_>>();
            debug!("... HEADER:: {:?}",str);
            headers.insert(parts.first().unwrap().to_string(), parts.last().unwrap().trim().to_string());

            let done = parts.len() < 2;
            if (count > 30 || m == 0 || done) && headers.get("Upgrade") == Some(&"websocket".to_string()) {
                let sec_resp = WebSock::get_sec(headers.get("Sec-WebSocket-Key").unwrap());
                let mut response_string = "HTTP/1.1 101 Switching Protocols\r\n".to_string();
                response_string.push_str("Upgrade: websocket\r\n");
                response_string.push_str("Connection: Upgrade\r\n");
                response_string.push_str("Sec-WebSocket-Protocol: hive\r\n");
                response_string.push_str(&format!("Sec-WebSocket-Accept: {}\r\n\r\n", sec_resp));

                AsyncWriteExt::write(&mut stream, response_string.as_bytes()).await?;
                AsyncWriteExt::flush(&mut stream).await?;

                let from = stream.peer_addr().unwrap().to_string();
                let stream_clone = stream.clone();
                let addr = stream.peer_addr().unwrap().to_string();
                task::spawn(async move {
                    read_loop(&sender, stream_clone, &addr).await;
                    debug!("<<< READ DONE, send hangup");
                    sender.send(SocketEvent::Hangup { from }).await.expect("failed to send hangup");
                    debug!("<<SENT");
                });

                break;
            }
        }

        return Ok(WebSock {
            _headers: headers,
            stream,
        });
    }

    fn get_sec(key: &str) -> String {
        // let fake_key = "dGhlIHNhbXBsZSBub25jZQ==";
        let mut kk = String::with_capacity(key.len() + 38);
        kk.push_str(key);
        kk.push_str(SEC_KEY);
        let hash = Sha1::digest(kk.as_bytes());
        return base64::encode(hash);
        //Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=
    }

    pub async fn send_message(&self, msg: Bytes) ->Result<()> {

        send_message(msg, &self.stream).await
    }
}

async fn send_message(msg: Bytes, mut stream: &TcpStream) -> Result<()> {
    let message = Message::binary(msg);

    let mut bytes = BytesMut::new();
    MessageCodec::server().encode(&message, &mut bytes)?;
        // .expect("didn't expect MessageCodec::encode to return an error");

    AsyncWriteExt::write(&mut stream, bytes.as_ref()).await?;
    // debug!("really send:: {:?}", stream.peer_addr().unwrap());
    debug!("really send:: {:?}", message);
    stream.flush().await?;
    Ok(())
}

async fn read_loop(mut sender: &UnboundedSender<SocketEvent>, stream: TcpStream, from_string:&String) {
    use tokio_util::codec::Decoder;

    let mut is_running = true;
    while is_running {
        let mut reader = BufReader::new(stream.clone());

        let mut buffer = [0u8; 128];
        let read = AsyncReadExt::read(&mut reader, &mut buffer).await.expect("failed to read from web socket");
        let mut bytes = BytesMut::from(buffer.as_ref());
        let mut mc = MessageCodec::server();
        debug!("reading:: {:?}", read);
        if read == 0 {
            debug!("read 0 from {:?}",stream.peer_addr());
            is_running = false;
            continue;
        }
        let res = mc.decode(&mut bytes);
        debug!("DECODED: {:?}, from {:?}", res, stream.peer_addr().unwrap());
        match res {
            Ok(t) => {
                let message = t.unwrap();
                match message.opcode() {
                    Opcode::Binary | Opcode::Text => {
                        debug!("OPCODE {:?}: {:?}",message.opcode(), message);
                        let received = message.data().to_owned();
                        let evnt = SocketEvent::Message {
                            from: from_string.clone(),
                            msg: received,
                        };
                        sender.send(evnt).await.expect("failed to send");
                    },
                    // Opcode::Text => {
                    //     let received = message.data().to_owned();
                    //     debug!("TEXT OPCODE {:?}", received);
                    //     // process message
                    //     let evnt = SocketEvent::Message {
                    //         from: from_string.clone(),
                    //         msg: received,
                    //     };
                    //     sender.send(evnt).await.expect("failed to send");
                    // }
                    Opcode::Close => {
                        debug!("CLOSE socket");
                        is_running = false;
                    }
                    o => {
                        debug!("Unexpected opcode: {:?}", o);
                    }
                }
            },
            Err(e) => {
                debug!("ERROR: {:?}", e);
                is_running = false;
            }
        }
    }

    debug!("Web sock run done!");
}