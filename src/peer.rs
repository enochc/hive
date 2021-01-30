
use log::{debug, info};
use async_std::{
    io::BufReader,
    net::TcpStream,
    prelude::*,
    task,
};
// use futures::{SinkExt, AsyncBufReadExt, AsyncReadExt};
use futures::{SinkExt, AsyncBufReadExt};
use futures::channel::mpsc::{UnboundedSender};
use futures::executor::block_on;

#[cfg(feature = "bluetooth")]
use crate::bluetooth::{central::Central};
#[cfg(feature = "bluetooth")]
use bluster::gatt::event::{Response};

use bytes::{BytesMut, BufMut, Bytes, Buf};
use crate::hive::{Sender, HI, HELLO, HIVE_PROTOCOL, HEADER_NAME};
use std::time::{Duration, SystemTime};
use async_std::sync::{Arc, RwLock};

use std::fmt::{Debug};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
#[cfg(feature = "websock")]
use crate::websocket::WebSock;



const ACK_DURATION:u64 = 30;

#[cfg(not(feature = "bluetooth"))]
#[derive(Debug)]
pub struct Response {}

#[cfg(not(feature = "websock"))]
#[derive(Debug)]
pub struct WebSock {}

#[cfg(not(feature = "bluetooth"))]
#[derive(Debug)]
pub struct Central {}


#[derive(Debug)]
pub enum SocketEvent {
    NewPeer {
        name: String,
        stream: Option<TcpStream>,
        peripheral: Option<Sender<Bytes>>,
        central: Option<Central>,
        address: String,
    },
    Message {
        from: String,
        msg: String,
    },
    SendBtProps {
        sender: futures::channel::oneshot::Sender<Response>,
    },
    Hangup {
        from: String,
    },
}

#[derive(Debug)]
pub struct Peer {
    name: Arc<RwLock<String>>,
    pub stream: Option<TcpStream>,
    pub update_peers: bool,
    pub peripheral: Option<Sender<Bytes>>,
    central: Option<Central>,
    pub address: String,
    last_received: Arc<RwLock<SystemTime>>,
    ack_check: Arc<AtomicBool>,
    pub web_sock: Option<WebSock>,
    event_sender: UnboundedSender<SocketEvent>,
}



fn as_u32_be(array: &[u8; 4]) -> u32 {
    ((array[0] as u32) << 24) +
        ((array[1] as u32) << 16) +
        ((array[2] as u32) << 8) +
        ((array[3] as u32) << 0)
}

impl Peer {
    pub fn is_bt_client(&self) -> bool{
        return self.peripheral.is_some();
    }
    pub fn to_string(&self) ->String {
        return format!("{:?},{:?}", self.get_name(), self.address)
    }

    pub async fn set_name(& self, name: &str) {

        info!("<<<< set name = {:?}", name);
        *self.name.write().await = String::from(name);

    }
    pub fn address(&self) -> String {
        return self.address.clone()
    }

    pub fn new(name: String,
               mut stream: Option<TcpStream>,
               peripheral: Option<Sender<Bytes>>,
               central:Option<Central>,
               sender: UnboundedSender<SocketEvent>,
               address: String,
               is_tcp_server:bool) -> Peer {

        return if stream.is_some() {
            debug!("<< 1");
            let arc_str = stream.as_ref().unwrap().clone();
            let addr = arc_str.peer_addr().unwrap().to_string();
            let mut peer = Peer {
                name: Arc::new(RwLock::new(name)),
                stream: Some(arc_str.clone()),
                update_peers: false,
                peripheral,
                central,
                address: addr,
                last_received: Arc::new(RwLock::new(std::time::SystemTime::now())),
                ack_check: Arc::new(AtomicBool::new(false)),
                event_sender: sender.clone(),
                web_sock: None,
            };
            debug!("<< 2 {:?}", peer);

            // Start read loop
            let send_clone = sender.clone();


            // todo some sort of handshake, is this we web socket? or a hive socket, or something else entirely
            if is_tcp_server {
                match stream.as_mut() {
                    Some( s) => {
                        block_on(async {
                            debug!("<< 3");
                            &peer.handshake(s, &send_clone).await.expect("Shake failed");
                            debug!("<< 4");
                        });
                    },
                    None => {}
                };
            };

            // WebSock runs it's own read loop
            if peer.web_sock.is_none() {
                task::spawn(async move {
                    read_loop(send_clone, &arc_str).await;
                });
            }

            peer
        } else {
            Peer {
                name: Arc::new(RwLock::new(name)),
                stream,
                update_peers: false,
                peripheral,
                central,
                address,
                last_received: Arc::new(RwLock::new(std::time::SystemTime::now())),
                ack_check: Arc::new(AtomicBool::new(false)),
                event_sender: sender,
                web_sock: None
            }
        }

    }

    async fn handshake(&mut self, stream:&mut TcpStream, sender: &UnboundedSender<SocketEvent>) -> Result<(), std::io::Error>{
        // let mut strea_clone = stream.clone();
        let mut reader = BufReader::new(stream.clone());

        let mut str = String::new();

        // let buff = [0u8;1024];
        // let mut buff = vec![0u8];
        // let r = ReadExt::read(&mut reader, &mut *buff).await;
        // let r = reader.read(&mut *buff).await;
        // let r  = AsyncBufReadExt::read(&mut reader, &mut str).await?;
        // info!("<<handshake read: {:?} = {:?}",r,  buff);


        AsyncBufReadExt::read_line(&mut reader, &mut str).await?;

        info!("<<< handshake:: {:?}", str);

        if str.starts_with(HIVE_PROTOCOL) {
            loop {
                let mut bm = BytesMut::new();
                str = "".to_string();
                AsyncBufReadExt::read_line(&mut reader, &mut str).await?;
                info!("<< next line:: {:?}", str);
                bm.put_slice(str.as_bytes());
                let my8 = bm.get_u8();
                match my8 {
                    HEADER_NAME =>{
                        // trim off the newline char
                        bm.truncate(bm.len()-1);
                        let name = String::from_utf8(bm.to_vec()).unwrap();
                        self.set_name(&name).await;
                        break;
                    },
                    _ => {
                        info!("<<<<< something else");
                        break;
                    }
                }
            }
        } else if str.starts_with("GET") {
            #[cfg(feature = "websock")]
                {
                    info!("<< do websocket");
                    let sock = WebSock::from_stream(reader, stream.clone(), sender.clone()).await?;
                    self.web_sock = Some(sock);
                }
        };
        info!("<<<<<<<<<<<<<<<<<, shook");
        Ok(())

    }

    pub fn get_name(&self)->String{
        let name = &*block_on(self.name.read());
        return String::from(name);
    }
    pub fn receive_hello(&self){
        debug!("<< RECEIVED HELLO {:?}", self.to_string());
        self.ack_check.store(false, Relaxed);
    }
    pub async fn ack(&self){
        debug!("<<<< ACK {:?}",self.to_string());
        // let mut ff = *self.last_received.write().await;
        *self.last_received.write().await = SystemTime::now();
    }
    // when  hi is received, we send a hello
    pub async fn send_hello(&self){
        debug!("<< SEND HELLO {:?}", self.to_string());
        self.send(HELLO).await;
    }

    pub async fn wave(&self){
        let name_clone = self.name.clone();
        let addr_clone = self.address.clone();
        let mut perf_clone = self.peripheral.as_ref().unwrap().clone();
        let ack_check_clone = self.ack_check.clone();
        let mut sender_clone = self.event_sender.clone();
        let adr_clone = self.address.clone();
        let last_received_clone = self.last_received.clone();
        async_std::task::spawn(async move{
            'wave_loop:loop{
                debug!("<< << <<< << << << << send Hi {:?}, {}", name_clone.read().await, addr_clone);
                task::sleep(Duration::from_secs(ACK_DURATION)).await;
                let since_last_comm= SystemTime::now().duration_since(*last_received_clone.read().await);
                debug!("<<<<<<< SINCE {:?}", since_last_comm);
                if since_last_comm.unwrap() > Duration::from_secs(ACK_DURATION){
                    let mut bytes = BytesMut::from(HI);
                    bytes.put_slice(name_clone.read().await.as_bytes());

                    match perf_clone.send(bytes.into()).await {
                        Ok(_) => {}
                        Err(e) => {
                            debug!("<<<<<< Error sending:: {:?}",e);
                            break 'wave_loop;
                        }
                    }
                    ack_check_clone.store(true,Relaxed);
                    task::sleep(Duration::from_secs(5)).await; // sleep 5 seconds for reply
                    if ack_check_clone.load(Relaxed) {
                        // No hello
                        debug!("<<<<<<<<<<< KILL THIS PEAR IS DEAD:: {:?}",name_clone.read().await);
                        sender_clone.send(SocketEvent::Hangup {from:adr_clone.clone()}).await.expect("failed to send hangup");
                        break 'wave_loop;
                    }
                }

            }
            debug!("<< Done Waving <<< {:?}", name_clone.read().await)
        });

    }
    pub async fn send(& self, msg: &str) {
        debug!("SEND starts here {:?}", msg);
        if self.stream.is_some(){

            if self.web_sock.is_some(){

                self.web_sock.as_ref().unwrap().send_message(msg.as_bytes().into()).await;
            } else {
                let s = self.stream.as_ref().unwrap();
                let stream = &s.clone();
                debug!("Send to peer {}: {}", self.name.read().await, msg);
                Peer::send_on_stream(stream, msg).await.expect("failed to send to Peer");
            }
        } else if self.central.is_some() {
            #[cfg(feature = "bluetooth")]
                {
                    debug!("SEND to bt {:?} from {:?}", msg, self.name);
                    let mut buff = BytesMut::new();
                    buff.put_slice(msg.as_bytes());
                    let sender = self.central.as_ref().unwrap();
                    sender.send(buff).await;
                }
        }else if self.peripheral.is_some(){
            debug!("Send via bt peripheral");
            let mut buff = BytesMut::new();
            buff.put_slice(msg.as_bytes());
            let b = buff.freeze();
            self.peripheral.as_ref().unwrap()
                .send(b.clone()).await.expect("failed to send something somewhere");
            debug!("sent...");
        } else {
            unimplemented!("cant send: {:?}" ,msg);
        }
        // listen for ack
        // let lar = *self.last_ack_received.read().await;
        // let since_last_ack = std::time::SystemTime::now().duration_since(lar).unwrap();
        // debug!("<< since last ack received: {:?}",since_last_ack);
        // if since_last_ack > Duration::from_secs(ACH_DURATION+2) {
        //     debug!("<< LISTEN FOR ACK");
        //     // let waiting = *self.ack_check.read().await
        //     if !*self.ack_check.read().await {
        //         *self.ack_check.write().await = true;
        //
        //         let ack_check_clone = self.ack_check.clone();
        //         let name_clone = self.name.clone();
        //         task::spawn( async move {
        //             // let done = cvar.wait(waiting).unwrap();
        //             while *ack_check_clone.read().await {
        //                 sleep(Duration::from_secs(1));
        //                 debug!("<<<<< ...... waiting for ack from {:?}", name_clone);
        //             }
        //             debug!("<<<< DONE WAITING");
        //         });
        //     }
        //
        //     let mut lar = self.last_ack_received.write().await;//.//.//lock().await;
        //     *lar = std::time::SystemTime::now();
        // }


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

    debug!("<< start tcp socker read loop");
    let mut reader = BufReader::new(&*stream);
    let from = match stream.peer_addr() {
        Ok(addr) => addr.to_string(),
        _ => String::from("no peer address"),
    };
    let mut is_running = true;
    while is_running {
        let mut sender = sender.clone();
        let mut size_buff = [0; 4];
        // let r = AsyncReadExt::read(&mut reader, &mut size_buff).await;
        let r = reader.read(&mut size_buff).await;
        let from = String::from(&from);
        match r {
            Ok(read) => {
                if read == 0 {
                    // end connection, something bad happened, or the client just disconnected.
                    debug!("Read zero bytes");
                    sender.send(SocketEvent::Hangup { from }).await.expect("Failed to send Hangup");
                    is_running = false;
                } else {
                    let message_size = as_u32_be(&size_buff);
                    let mut size_buff = vec![0u8; message_size as usize];
                    let red = reader.read_exact(&mut size_buff).await;
                    match red {
                        Ok(_t) => {
                            let msg = String::from(std::str::from_utf8(&size_buff).unwrap());
                            debug!("Read message: {:?}", &msg);
                            let se = SocketEvent::Message {
                                from,
                                msg,
                            };
                            if !sender.is_closed() {
                                sender.send(se).await.expect("Failed to send message");
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