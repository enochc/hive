use std::collections::HashMap;
use std::string::FromUtf8Error;

use async_std::{
    io::BufReader,
    net::TcpStream,
    task,
};
use base64;
use bytes::{BytesMut, Bytes};
use futures::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, SinkExt};
use futures::channel::mpsc::UnboundedSender;
use log::{debug, info};
use sha1::{Digest, Sha1};
use tokio_util::codec::Encoder;
use websocket_codec::{Message, MessageCodec, Opcode};
use websocket_codec::protocol::{DataLength, FrameHeader, FrameHeaderCodec};

use crate::peer::SocketEvent;
use std::error::Error;
use async_std::net::SocketAddr;

const SEC_KEY: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Debug)]
pub struct WebSock {
    headers: HashMap<String, String>,
    stream: TcpStream,
}

impl WebSock {
    pub fn address(&self)-> Result<String, std::io::Error> {
        return match self.stream.peer_addr(){
            Ok(a) => Ok(a.to_string()),
            Err(e) => {Result::Err(e)}
        }
    }
    pub async fn from_stream(mut reader: BufReader<TcpStream>,
                             mut stream: TcpStream,
                             mut sender: UnboundedSender<SocketEvent>) -> Result<WebSock, std::io::Error> {
        let mut count = 0;
        let mut str: String;
        let mut headers = HashMap::new();

        loop {
            count += 1;
            str = "".to_string();
            let m = AsyncBufReadExt::read_line(&mut reader, &mut str).await?;
            let parts = str.split(":").collect::<Vec<_>>();
            headers.insert(parts.first().unwrap().to_string(), parts.last().unwrap().trim().to_string());
            // info!("<< parts: {:?}, {:?}", parts.first().unwrap(), parts.last().unwrap().trim());

            let done = parts.len() < 2;
            if count > 30 || m == 0 || done {
                let sec_resp = WebSock::get_sec(headers.get("Sec-WebSocket-Key").unwrap());
                let mut response_string = "HTTP/1.1 101 Switching Protocols\r\n".to_string();
                response_string.push_str("Upgrade: websocket\r\n");
                response_string.push_str("Connection: Upgrade\r\n");
                response_string.push_str("Sec-WebSocket-Protocol: hive\r\n");
                response_string.push_str(&format!("Sec-WebSocket-Accept: {}\r\n\r\n", sec_resp));


                AsyncWriteExt::write(&mut stream, response_string.as_bytes()).await?;
                AsyncWriteExt::flush(&mut stream).await?;

                let from = stream.peer_addr().unwrap().to_string();
                let mut stream_clone = stream.clone();
                let addr = stream.peer_addr().unwrap().to_string();
                task::spawn(async move {
                    read_loop(&sender, stream_clone, &addr).await;
                    info!("<<< READ DONE, send hangup");
                    sender.send(SocketEvent::Hangup { from }).await.expect("failed to send hangup");
                    info!("<<SENT")
                });

                break;
            }
        }

        return Ok(WebSock {
            headers,
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

    pub async fn send_message(&self, msg:&[u8]) {

        send_message(msg.into(), &self.stream).await;
    }
}

async fn send_message(msg: &[u8], mut stream: &TcpStream) {
    let message = Message::binary(msg.to_owned());

    let mut bytes = BytesMut::new();
    MessageCodec::server()
        .encode(&message, &mut bytes)
        .expect("didn't expect MessageCodec::encode to return an error");

    AsyncWriteExt::write(&mut stream, bytes.as_ref()).await;
    info!("really send:: {:?}", stream.peer_addr().unwrap());
    stream.flush().await;
}

async fn read_loop(mut sender: &UnboundedSender<SocketEvent>, mut stream: TcpStream, from_string:&String) {
    use tokio_util::codec::Decoder;

    let mut is_running = true;
    while is_running {
        let mut reader = BufReader::new(stream.clone());

        let mut buffer = [0u8; 128];
        let read = AsyncReadExt::read(&mut reader, &mut buffer).await.expect("failed to read from web socket");
        let mut bytes = BytesMut::from(buffer.as_ref());
        let mut mc = MessageCodec::server();
        info!("reading:: {:?}", read);
        if read == 0 {
            info!("<<< read 0 from {:?}",stream.peer_addr());
            is_running = false;
            continue;
        }
        let res = mc.decode(&mut bytes); //.unwrap().unwrap();
        info!("<<< DECODED: {:?}, from {:?}", res, stream.peer_addr().unwrap());
        match res {
            Ok(t) => {
                let message = t.unwrap();
                match message.opcode() {
                    Opcode::Binary => {
                        info!("<<<< BINARY OPCODE: {:?}", message);
                    },
                    Opcode::Text => {
                        let received = message.data().to_vec();
                        let ret_string = String::from_utf8(received).unwrap();//format!("{:?}",received);
                        info!("<<<< MESSAGE STRING {:?}", ret_string);
                        // process message
                        let evnt = SocketEvent::Message {
                            from: from_string.clone(),
                            msg: ret_string,
                        };
                        sender.send(evnt).await;
                    }
                    Opcode::Close => {
                        debug!("<<<<<<<, CLOSE");
                        is_running = false;
                    }
                    o => {
                        info!("Unexpected opcode: {:?}", o)
                    }
                }
            },
            Err(e) => {
                info!("ERROR: {:?}", e);
                is_running = false;
            }
        }


        // if read == 0 {
        //     is_running = false;
        // }
    }

    info!("<< websock run done");
}