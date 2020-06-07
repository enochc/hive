// use futures::channel::mpsc;
use futures::channel::mpsc::{UnboundedSender, UnboundedReceiver};
use futures::{SinkExt};
use async_std::{
    io::BufReader,
    net::{TcpListener, TcpStream},
    prelude::*,
    task,
    sync::Arc,
};
use futures::executor::block_on;

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
   pub fn new(name:String,
              stream:TcpStream,
              sender: UnboundedSender<SocketEvent>,
              _receiver: Option<UnboundedReceiver<SocketEvent>>) -> Peer {
        let arc_str = Arc::new(stream);
        println!("<<< New Peer: {:?}", &name);
        let peer = Peer{
            name,
            stream: arc_str.clone(),
        };

        // Start read loop
        let send_clone = sender.clone();
        let arc_str2 = arc_str.clone();
        task::spawn(async move{
            read_loop(send_clone, arc_str2).await;
        });

       //write loop
       // let arc_str3 = arc_str.clone();
       // task::spawn(async move{
       //     write_loop(receiver, arc_str3).await;
       // });
        
        return peer;
    }
    pub async fn send(& self, msg: &str){
        let stream = self.stream.clone();
        println!("<<< send to streem 1: {:?}, {:?}", stream.peer_addr().unwrap().to_string(), msg);
        block_on(send(stream, msg)).expect("failed to send msg on stream");
    }

}
async fn send(stream: Arc<TcpStream>, message: &str) -> Result<bool, std::io::Error> {
    let peer_name = stream.peer_addr().unwrap().to_string();
    println!("<<< send to streem 2: {:?}, {:?}", peer_name, message);
    let mut bytes = Vec::new();
    let msg_length: u32 = message.len() as u32;
    bytes.append(&mut msg_length.to_be_bytes().to_vec());
    bytes.append(&mut message.as_bytes().to_vec());
    // let mut stream = self.streem;
    let mut stream = &*stream;
    let n = stream.write(&bytes).await?;
    println!("<<< written to peer: {:?}, {:?}", peer_name, n);
    Result::Ok(true)
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
                        println!("sending from {:?}: {:?}", from, msg);
                        let se = SocketEvent::Message {
                            from,
                            msg
                        };

                        sender.send(se).await.expect("Failed to send message");
                    },
                    _ => eprintln!("Failed to read message")
                }
            }
        }
    }
}