#![allow(unused_imports)]

// use futures::channel::mpsc;
use futures::channel::mpsc::{UnboundedSender};
// use futures::{SinkExt, AsyncBufReadExt};
use futures::{SinkExt};
use async_std::{
    io::BufReader,
    net::{TcpStream},
    prelude::*,
    task,
    sync::Arc,
};
use crypto::digest::Digest;
use crypto::sha1::Sha1;

use crate::hive::{ACK, HANDSHAKE};
use crate::signal::Signal;
use std::collections::HashMap;


#[derive(Debug)]
pub enum SocketEvent {
    NewPeer {
        name: String,
        stream: TcpStream,
    },
    Message {
        from: String,
        msg: String,
    },
    Hangup {
        from: String,
    },
}

pub struct Peer {
    pub name: String,
    pub stream: Arc<TcpStream>,
    pub update_peers: bool,
    address: Option<String>,
    is_http: bool,
}

fn as_u32_be(array: &[u8; 4]) -> u32 {
    ((array[0] as u32) << 24) +
        ((array[1] as u32) << 16) +
        ((array[2] as u32) << 8) +
        ((array[3] as u32) << 0)
}

impl Peer {
    pub fn set_name(&mut self, name: &str) {
        self.name = String::from(name);
    }
    pub fn address(&self) -> String {
        return match self.address.clone() {
            Some(t) => t,
            _ => String::from("")
        };
    }

    pub async fn new(name: String,
                     stream: TcpStream,
                     sender: UnboundedSender<SocketEvent>,
                     is_server: bool) -> Peer {
        let arc_str = Arc::new(stream);

        let mut peer = Peer {
            name,
            stream: arc_str.clone(),
            update_peers: false,
            address: Some(arc_str.peer_addr().unwrap().to_string()),
            is_http: false
        };

        // Start read loop
        let send_clone = sender.clone();
        let arc_str2 = arc_str.clone();

        if !is_server {
            let mut stream = &*arc_str;
            stream.write(&HANDSHAKE.as_bytes()).await;
            // stream.flush();


        }
        peer.handshake().await;

        task::spawn(async move {
            read_loop(send_clone, arc_str2).await;
        });
        return peer;
    }

    /*
    If the handshake line starts with "GET / HTTP/1.1\r\n" then we're looking at a web socket
    the handshake ends with \r\n\r\n, an empty response with only the \r\n byte, then we know
    to exit when the read length == 2

    HTTP/1.1 101 Switching Protocols
    Upgrade: websocket
    Connection: Upgrade
    Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=

    concatenate the client's Sec-WebSocket-Key and the string "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
    together (it's a "magic string"), take the SHA-1 hash of the result, and return the base64
    encoding of that hash.
     */
    async fn handshake(&mut self) {
        let mut headers:HashMap<String, String> = HashMap::new();
        let mut reader = BufReader::new(&*self.stream);
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                // done collecting header
                Ok(t) if (t == 2 || t == 0) => { break; }
                // just keep filling the buffer
                Ok(t) if (t > 2) => {
                    let rest = buf.split(":");
                    let vec: Vec<&str> = rest.collect();
                    if vec.len()>1{
                        let mut resp:String = String::from(vec[1]);
                        for  x in 2..vec.len(){
                            resp.push_str(":");
                            resp.push_str(vec[x]);
                        }
                        let end =resp.len() - 2;
                        headers.insert(String::from(vec[0]), String::from(&resp[0..end]));
                    } else {
                        let val = vec[0];
                        let end = val.len() - 2;
                        headers.insert(String::from("HEAD"), String::from(&val[0..end]));
                    }
                },
                _ => {break;}
            }
        }

        /*
        So if the Key was "dGhlIHNhbXBsZSBub25jZQ==258EAFA5-E914-47DA-95CA-C5AB0DC85B11",
        the Sec-WebSocket-Accept header's value is "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
         */
        if headers.get("HEAD").unwrap_or(&String::from("")).starts_with("GET / HTTP/"){
            // println!("<<<< This is a web socket");
            println!("headers {:?}", headers);
            self.is_http = true;
            self.name = String::from("Web Socket");
            let key = format!("{}{}",headers["Sec-WebSocket-Key"],"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
            let mut hasher = Sha1::new();


            hasher.input_str(&key);
            // hasher.input_str("dGhlIHNhbXBsZSBub25jZQ==");
            // should be: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=
            let hex = hasher.result_str();
            // let pp = base64::encode(hex);
            // println!("key {}",hex);
            let response = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", hex);
            println!("response: {}", response);
            let mut stream = &*self.stream;
            stream.write(&response.as_bytes()).await;
        }
    }

    pub async fn send(&self, msg: &str) {
        let stream = self.stream.clone();
        println!("Send to peer {}: {}", self.name, msg);
        Peer::send_on_stream(stream, msg).await.expect("failed to send to Peer");
    }

    pub async fn send_on_stream(stream: Arc<TcpStream>, message: &str) -> Result<bool, std::io::Error> {
        let mut bytes = Vec::new();
        let msg_length: u32 = message.len() as u32;
        bytes.append(&mut msg_length.to_be_bytes().to_vec());
        bytes.append(&mut message.as_bytes().to_vec());
        let mut stream = &*stream;
        stream.write(&bytes).await?;
        // stream.flush().await;
        Result::Ok(true)
    }
}

/*
 Messages are transferred between services in the following protocol:
 message - 4 bytes consisting of message size, then the following x bytes are the message
 so when reading, the first 4 bytes are read to determine the message size, then we read that many
 more bytes to complete the message

 properties: |p|=(properties)
 */
// TODO, why do I not need this? why is writing to the stream
// async fn write_loop(receiver: Option<UnboundedReceiver<SocketEvent>>, stream: Arc<TcpStream>){
//     match receiver {
//         Some(mut r) => {
//             task::spawn(async move{
//                 while let Some(event) = r.next().await {
//                     match event {
//                         SocketEvent::Message{from, msg} => {
//                             println!("<<< send to streem 3: {:?}, {:?}", stream.peer_addr().unwrap().to_string(), msg);
//                             send(stream.clone(),msg.as_ref()).await;
//                         },
//                         _ => {
//                             println!("<<<< WHY AM I HERE?: {:?}", event);
//                         }
//                     }
//
//                 }
//             });
//         },
//         _ => {}
//     }
// }
async fn read_loop(sender: UnboundedSender<SocketEvent>, stream: Arc<TcpStream>) {
    let mut reader = BufReader::new(&*stream);
    let from = match stream.peer_addr() {
        Ok(addr) => addr.to_string(),
        _ => String::from("no peer address"),
    };
    let mut is_running = true;
    while is_running {
        let mut sender = sender.clone();
        let mut size_buff = [0; 4];

        let r = reader.read(&mut size_buff).await;
        let from = String::from(&from);
        match r {
            Ok(read) => {
                if read == 0 {
                    // end connection, something bad happened, or the client just disconnected.
                    println!("Read zero bytes");
                    sender.send(SocketEvent::Hangup { from }).await.expect("Failed to send Hangup");
                    is_running = false;
                } else {
                    let message_size = as_u32_be(&size_buff);
                    let mut size_buff = vec![0u8; message_size as usize];
                    let red = reader.read_exact(&mut size_buff).await;
                    match red {
                        Ok(_t) => {
                            let msg = String::from(std::str::from_utf8(&size_buff).unwrap());
                            let se = SocketEvent::Message {
                                from,
                                msg,
                            };
                            if !sender.is_closed() {
                                sender.send(se).await.expect("Failed to send message");
                                // process message to hive, then send ack
                                // let stream = stream.clone();
                                // println!("<< SEND ACK");
                                // Peer::send_on_stream(stream, ACK).await.expect("failed to send Ack");
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to read message {:?}", e);
                            sender.send(SocketEvent::Hangup {
                                from
                            }).await.expect("Failed to send Hangup");
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("ERROR: {:?}", e);
                sender.send(SocketEvent::Hangup { from }).await
                    .expect("Failed to send hangup");
                is_running = false;
            }
        }
    }
    println!("<< Peer run done");
}