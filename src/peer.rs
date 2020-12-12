#![allow(unused_imports)]
// use futures::channel::mpsc;
use futures::channel::mpsc::{UnboundedSender};
// use futures::{SinkExt, AsyncReadExt, AsyncWriteExt};
use futures::{SinkExt};
use async_std::{
    io::BufReader,
    net::{ TcpStream},
    prelude::*,
    task,
    sync::Arc,
};
// use crate::hive::ACK;
use crate::signal::Signal;

// trait Socket {
//     async fn read(bytes:&[byte]){}
// }

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
        from:String,
    },
}

pub struct Peer{
    pub name:String,
    pub stream: Arc<TcpStream>,
    pub update_peers:bool,
    address:Option<String>,
}

fn as_u32_be(array: &[u8; 4]) -> u32 {
    ((array[0] as u32) << 24) +
        ((array[1] as u32) << 16) +
        ((array[2] as u32) <<  8) +
        ((array[3] as u32) <<  0)
}

impl Peer {
    pub fn set_name(&mut self, name:&str){
        self.name = String::from(name);
    }
    pub fn address(& self) -> String {
        return match self.address.clone() {
            Some(t) => t,
            _ => String::from("")
        };
    }

   pub fn new(name:String,
              stream:TcpStream,
              sender: UnboundedSender<SocketEvent>) -> Peer {
        let arc_str = Arc::new(stream);

        let peer = Peer{
            name,
            stream: arc_str.clone(),
            update_peers: false,
            address: Some(arc_str.peer_addr().unwrap().to_string()),
        };

        // Start read loop
        let send_clone = sender.clone();
        let arc_str2 = arc_str.clone();

        task::spawn(async move{
            read_loop(send_clone, arc_str2).await;
        });
        return peer;
    }
    pub async fn send(& self, msg: &str){
        let stream = self.stream.clone();
        println!("Send to peer {}: {}",self.name ,msg);
        Peer::send_on_stream(&stream, msg).await.expect("failed to send to Peer");
    }

    pub async fn send_on_stream(mut stream: &TcpStream, message: &str) -> Result<bool, std::io::Error> {
    // pub async fn send_on_stream<T: Socket>(stream:T, message: &str)  -> Result<bool, std::io::Error> {
        let mut bytes = Vec::new();
        let msg_length: u32 = message.len() as u32;
        bytes.append(&mut msg_length.to_be_bytes().to_vec());
        bytes.append(&mut message.as_bytes().to_vec());
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
async fn read_loop(sender: UnboundedSender<SocketEvent>, stream: Arc<TcpStream>){
    let mut reader = BufReader::new(&*stream);
    let from = match stream.peer_addr() {
        Ok(addr) => addr.to_string(),
        _ => String::from("no peer address"),
    };
    let mut is_running = true;
    while is_running{
        let mut sender = sender.clone();
        let mut size_buff = [0; 4];

        let r = reader.read(&mut size_buff).await;
        let from = String::from(&from);
        match r {
            Ok(read) => {

                if read == 0 {
                    // end connection, something bad happened, or the client just disconnected.
                    println!("Read zero bytes");
                    sender.send(SocketEvent::Hangup{from}).await.expect("Failed to send Hangup");
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
                                msg
                            };
                            if !sender.is_closed(){

                                sender.send(se).await.expect("Failed to send message");
                                // process message to hive, then send ack
                                // let stream = stream.clone();
                                // println!("<< SEND ACK");
                                // Peer::send_on_stream(stream, ACK).await.expect("failed to send Ack");

                            }
                        },
                        Err(e) => {
                            eprintln!("Failed to read message {:?}", e);
                            sender.send(SocketEvent::Hangup{
                                from
                            }).await.expect("Failed to send Hangup");
                        }
                    }
                }
            },
            Err(e) => {
                eprintln!("ERROR: {:?}",e);
                sender.send(SocketEvent::Hangup{from}).await
                    .expect("Failed to send hangup");
                is_running = false;
            }
        }
    }
    println!("<< Peer run done");
}