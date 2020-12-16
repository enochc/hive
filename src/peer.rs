
use log::{debug};
use async_std::{
    io::BufReader,
    net::TcpStream,
    prelude::*,
    task,
};
// use futures::{SinkExt, AsyncReadExt, AsyncWriteExt};
use futures::SinkExt;
use futures::channel::mpsc::{UnboundedSender};

#[cfg(feature = "bluetooth")]
use crate::bluetooth::{peripheral::Peripheral, central::Central};




#[cfg(not(feature = "bluetooth"))]
#[derive(Debug)]
pub struct Peripheral {}

#[cfg(not(feature = "bluetooth"))]
#[derive(Debug)]
pub struct Central {}

#[derive(Debug)]
pub enum SocketEvent {
    NewPeer {
        name: String,
        stream: Option<TcpStream>,
        peripheral: Option<Peripheral>,
        central: Option<Central>,
        address: Option<String>,
    },
    Message {
        from: String,
        msg: String,
    },
    Hangup {
        from: String,
    },
}

#[derive(Debug)]
pub struct Peer {
    pub name: String,
    pub stream: Option<TcpStream>,
    pub update_peers: bool,
    peripheral: Option<Peripheral>,
    central: Option<Central>,
    address: Option<String>,
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

    pub fn new(name: String,
               stream: Option<TcpStream>,
               peripheral: Option<Peripheral>,
               central:Option<Central>,
               sender: UnboundedSender<SocketEvent>,
               address: Option<String>) -> Peer {

        return if stream.is_some() {
            let arc_str = stream.unwrap();
            let addr = arc_str.peer_addr().unwrap().to_string();
            let peer = Peer {
                name,
                stream: Some(arc_str.clone()),
                update_peers: false,
                peripheral,
                central,
                address: Some(addr),
            };

            // Start read loop
            let send_clone = sender.clone();

            task::spawn(async move {
                read_loop(send_clone, &arc_str).await;
            });
            peer
        } else {
            Peer {
                name,
                stream,
                update_peers: false,
                peripheral,
                central,
                address,
            }
        }
    }
    pub async fn send(& self, msg: &str) {
        debug!("SEND starts here <<<< {:?}", msg);
        if self.stream.is_some(){
            let s = self.stream.as_ref().unwrap();
            let stream = &s.clone();
            debug!("Send to peer {}: {}", self.name, msg);
            Peer::send_on_stream(stream, msg).await.expect("failed to send to Peer");
        } else if self.central.is_some() {
            debug!("SEND to bt");
            let sender = self.central.as_ref().unwrap();
            sender.send(msg).await;
            // self.central.as_ref().as_mut().expect("failed to get central").send(msg);
        } else {
            unimplemented!("cant send: {:?}" ,msg);
        }

    }


    pub async fn send_on_stream(mut stream: &TcpStream, message: &str) -> Result<bool, std::io::Error> {
        let mut bytes = Vec::new();
        let msg_length: u32 = message.len() as u32;
        bytes.append(&mut msg_length.to_be_bytes().to_vec());
        bytes.append(&mut message.as_bytes().to_vec());
        stream.write(&bytes).await.expect("Failed to write to stream");
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

async fn read_loop(sender: UnboundedSender<SocketEvent>, stream: &TcpStream) {
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