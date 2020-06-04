use futures::channel::mpsc;
use futures::channel::mpsc::{UnboundedSender, UnboundedReceiver};
use futures::SinkExt;
use async_std::{
    io::BufReader,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
    task,
    sync::Arc,
};
use std::borrow::BorrowMut;

#[derive(Debug)]
pub enum SocketEvent {
    NewPeer {
        name: String,
        // stream: Arc<TcpStream>,
        stream: TcpStream,
    },
    Message {
        from: String,
        // to: Vec<String>,
        msg: String,
    },
}

pub struct Peer{
    pub name:String,
    pub stream: Arc<TcpStream>,
}

fn as_u32_be(array: &[u8; 4]) -> u32 {
    ((array[0] as u32) << 24) +
        ((array[1] as u32) << 16) +
        ((array[2] as u32) <<  8) +
        ((array[3] as u32) <<  0)
}

impl Peer {
   pub fn new(name:String, stream:TcpStream, sender: UnboundedSender<SocketEvent>) -> Peer {
        let arcStr = Arc::new(stream);
        println!("<<< New Peer: {:?}", &name);
        let peer = Peer{
            name,
            stream: arcStr.clone(),
        };

        // Start read loop
        let send_clone = sender.clone();
        let arcStr2 = arcStr.clone();
        task::spawn(async move{
            read_loop(send_clone, arcStr2).await;
        });
       // task::spawn(async move{
       //     write_loop(send_clone, arcStr2.clone()).await;
       // });
        
        return peer;
    }

    pub async fn send(& mut self, message: String) -> Result<bool, std::io::Error> {
        println!("<<< wrighting to peer: {:?}", message);
        let mut bytes = Vec::new();
        let msg_length: u32 = message.len() as u32;
        bytes.append(&mut msg_length.to_be_bytes().to_vec());
        bytes.append(&mut message.as_bytes().to_vec());
        // let mut stream = self.streem;
        let mut stream = &*self.stream;
        let n = stream.write(&bytes).await?;
        println!("<<< written to peer: {:?}", n);
        Result::Ok(true)
    }
}


async fn read_loop(sender: UnboundedSender<SocketEvent>, stream: Arc<TcpStream>){
    let mut reader = BufReader::new(&*stream);
    loop {
        let mut sender = sender.clone();
        let mut size_buff = [0; 4];
        // While next message zize is > 0 bites read messages
        while let Ok(read) = reader.read(&mut size_buff).await {
            if read > 0 {
                let message_size = as_u32_be(&size_buff);
                println!("SIZE: {:?}", message_size);
                let mut size_buff = vec![0u8; message_size as usize];
                match reader.read_exact(&mut size_buff).await {
                    Ok(_t) => {
                        let msg = String::from(std::str::from_utf8(&size_buff).unwrap());
                        let from = match stream.peer_addr() {
                            Ok(addr) => addr.to_string(),
                            _ => String::from("no peer address"),
                        };
                        println!("read: {:?}", msg);
                        let se = SocketEvent::Message {
                            from,
                            msg
                        };

                        sender.send(se).await;
                    },
                    _ => eprintln!("Failed to read message")
                }
            }
        }
    }
}