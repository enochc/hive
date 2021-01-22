
use base64;
use sha1::{Digest, Sha1};

use log::{debug, info};
use futures::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use std::collections::HashMap;

use async_std::{
    io::BufReader,
    net::TcpStream,
    task,
};

use futures::channel::mpsc::UnboundedSender;
use crate::peer::SocketEvent;
use bytes::{BytesMut};
use websocket_codec::protocol::{FrameHeaderCodec, FrameHeader, DataLength};
use websocket_codec::mask::{Mask, mask_slice};
use tokio_util::codec::Encoder;
use websocket_codec::{Message, MessageCodec};

const SEC_KEY:&str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Debug)]
pub struct WebSock{
    headers:HashMap<String, String>
}

impl WebSock {
    pub async fn from_stream(mut reader:BufReader<TcpStream>, mut stream:TcpStream, sender: UnboundedSender<SocketEvent>) -> Result<WebSock, std::io::Error>{
        let mut count = 0;
        let mut str:String;
        let mut headers = HashMap::new();
        // let mut reader = BufReader::new(stream.clone());
        loop {
            count += 1;
            str = "".to_string();
            let m = AsyncBufReadExt::read_line(&mut reader, &mut str).await?;
            let parts = str.split(":").collect::<Vec<_>>();
            headers.insert(parts.first().unwrap().to_string(), parts.last().unwrap().trim().to_string());
            info!("<< parts: {:?}, {:?}", parts.first().unwrap(), parts.last().unwrap().trim());

            let done = parts.len()<2;
            if count >30|| m==0 || done {
                let sec_resp = WebSock::get_sec(headers.get("Sec-WebSocket-Key").unwrap());
                let mut response_string = "HTTP/1.1 101 Switching Protocols\r\n".to_string();
                response_string.push_str("Upgrade: websocket\r\n");
                response_string.push_str("Connection: Upgrade\r\n");
                response_string.push_str("Sec-WebSocket-Protocol: hive\r\n");
                response_string.push_str(&format!("Sec-WebSocket-Accept: {}\r\n\r\n",sec_resp));
                // stream.write(response_string.as_bytes()).await;
                let mm =AsyncWriteExt::write(&mut stream, response_string.as_bytes()).await?;
                // stream.flush().await;
                AsyncWriteExt::flush(&mut stream).await?;


                task::spawn(async move {
                    read_loop(&sender, stream).await;
                });

                info!("<<<<<<<<<<<<<<<<<, break {:?}, {:?}",mm, sec_resp);
                break;
            }
        }

        return Ok(WebSock{
            headers
        })
    }

    fn get_sec(key:&str)->String{
        // let fake_key = "dGhlIHNhbXBsZSBub25jZQ==";
        let mut kk = String::with_capacity(key.len()+38);
        // let mut kk = String::with_capacity(fake_key.len()+38);
        kk.push_str(key);
        kk.push_str(SEC_KEY);

        let hash = Sha1::digest(kk.as_bytes());
        return base64::encode(hash);

        //Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=

    }


}

async fn read_loop(sender: &UnboundedSender<SocketEvent>, mut stream: TcpStream) {
    use tokio_util::codec::Decoder;
    use assert_allocations::assert_allocated_bytes;


    let mut is_running = true;
    while is_running {
        // let mut sender = sender.clone();
        let mut reader = BufReader::new(stream.clone());

        info!("<<<< SOCK LOOP");
        let mut buffer = [0u8;128];
        let read = AsyncReadExt::read(&mut reader, &mut buffer).await.expect("failed to read from web socket");
        let mut bytes = BytesMut::from(buffer.as_ref());

        let blob:FrameHeader = FrameHeaderCodec.decode(&mut bytes).unwrap().unwrap();
        match blob.data_len(){
            DataLength::Small(n) => {

                let v = &mut bytes[0..n as usize];
                let mask:Mask = blob.mask().unwrap();
                let mut vv = v.as_mut();
                mask_slice(  &mut vv, mask);

                let msg = String::from_utf8(v.to_vec()).unwrap();
                info!("<<<< TADA!!!! {:?}", msg);

                if msg.starts_with("hello") {
                    info!("<<<< test send");
                    let message = Message::text("hi");
                    let mut bytes = BytesMut::new();
                    MessageCodec::server()
                        .encode(&message, &mut bytes)
                        .expect("didn't expect MessageCodec::encode to return an error");

                    let mm =AsyncWriteExt::write(&mut stream, bytes.as_ref()).await;

                }
            }
            DataLength::Medium(_) => {}
            DataLength::Large(_) => {}
        }
        info!("_____ {:?}",blob);
        info!("the rest:: {:?}",bytes);

        if read == 0 {
            is_running = false;
        }


    }

    println!("<< websock run done");
}