use std::fmt::Debug;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, SystemTime};

use async_std::{
    io::BufReader,
    net::TcpStream,
    prelude::*,
    task,
};
use async_std::sync::{Arc, RwLock};
#[cfg(feature = "bluetooth")]
use bluster::gatt::event::Response;
use bytes::{Buf, BufMut, Bytes, BytesMut};
// use futures::{AsyncBufReadExt, SinkExt, AsyncWriteExt};
use futures::{AsyncBufReadExt, SinkExt};
use futures::channel::mpsc::UnboundedSender;
use futures::executor::block_on;
#[allow(unused_imports)]
use log::{debug, info, trace, warn};

#[cfg(feature = "bluetooth")]
use crate::bluetooth::{central::Central};
use crate::hive::{HEADER_NAME, HIVE_PROTOCOL, PING, PONG, Result, Sender, HEADER};
#[cfg(feature = "websock")]
use crate::websocket::WebSock;

const ACK_DURATION: u64 = 30;

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
        ptype: PeerType,
    },
    Message {
        from: String,
        msg: Bytes,
    },
    SendBtProps {
        sender: futures::channel::oneshot::Sender<Response>,
    },
    Hangup {
        from: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeerType {
    TcpServer = 0,
    TcpClient = 1,
    // this isn't determined until the handshake
    WebSockClient = 2,
    BluetoothCentral = 3,
    BluetoothPeripheral = 4,
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
    hive_name: String,
    peer_type: PeerType,
}


fn as_u32_be(array: &[u8; 4]) -> u32 {
    ((array[0] as u32) << 24) +
        ((array[1] as u32) << 16) +
        ((array[2] as u32) << 8) +
        ((array[3] as u32) << 0)
}

impl Peer {
    pub fn is_bt_client(&self) -> bool {
        return self.peripheral.is_some();
    }
    pub fn to_string(&self) -> String {
        return format!("{:?},{:?}", self.get_name(), self.address);
    }

    pub async fn set_name(&self, name: &str) {
        debug!("{:?} set name = {:?}", self.get_id_name(), name);
        *self.name.write().await = String::from(name);
        debug!("set: {:?}", self.get_id_name());
    }
    pub fn address(&self) -> String {
        return self.address.clone();
    }

    pub async fn new(
        name: String,
        mut stream: Option<TcpStream>,
        peripheral: Option<Sender<Bytes>>,
        central: Option<Central>, sender: UnboundedSender<SocketEvent>,
        address: String,
        hive_name: String,
        peer_type: PeerType,
    ) -> Peer {

        return if stream.is_some() {
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
                hive_name,
                peer_type,
            };

            let send_clone = sender.clone();

            match stream.as_mut() {
                Some(s) => {
                    let msg = format!("Shake failed for {}", peer.get_id_name());
                    &peer.handshake(s, &sender).await.expect(&msg);
                }
                None => {}
            };

            // WebSock runs it's own read loop
            if peer.web_sock.is_none() {
                let name_id = String::from(&*peer.get_id_name());
                debug!("start tcp socket read loop for({})", name_id);

                task::spawn(async move {
                    read_loop(send_clone, &arc_str, name_id).await;
                });
            }




            return peer;
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
                web_sock: None,
                hive_name,
                peer_type,
            }
        };

    }

    async fn handshake(&mut self, stream: &mut TcpStream, sender: &UnboundedSender<SocketEvent>) -> Result<()> {

        match self.peer_type {
            PeerType::TcpServer => {
                debug!("<<<< SEND  CLIENT HANDSHAKE");
                let mut bm = BytesMut::new();
                bm.put_slice(format!("{}\n", HIVE_PROTOCOL).as_bytes());
                bm.put_u8(HEADER_NAME);
                bm.put_slice(format!("{}\n", self.hive_name).as_bytes());
                stream.write(bm.as_ref()).await.expect("write failed");
                stream.flush().await.expect("flush failed");
            },
            PeerType::TcpClient => {
                let mut reader = BufReader::new(stream.clone());
                debug!("{:?} handshake....", self.get_id_name());
                let mut buff = Vec::new();
                let eol_byte = "\n".as_bytes()[0];
                AsyncBufReadExt::read_until(&mut reader, eol_byte, &mut buff).await?;
                let mut str = String::from_utf8(buff).expect("failed to parse utf8");

                if str.starts_with(HIVE_PROTOCOL) {
                    loop {
                        let mut bm = BytesMut::new();
                        str = "".to_string();
                        AsyncBufReadExt::read_line(&mut reader, &mut str).await.expect("failed to read line post header");
                        debug!("next line:: {:?}", str);
                        bm.put_slice(str.as_bytes());
                        let my8 = bm.get_u8();
                        match my8 {
                            HEADER_NAME => {
                                // trim off the newline char
                                bm.truncate(bm.len() - 1);
                                let name = String::from_utf8(bm.to_vec()).expect("Can't parse name");
                                self.set_name(&name).await;
                                break;
                            }
                            _ => {
                                warn!("something else");
                                break;
                            }
                        }
                    }
                } else if str.starts_with("GET") {
                    #[cfg(feature = "websock")]
                        {
                            debug!("do websocket");
                            let sock = WebSock::from_stream(
                                reader,
                                stream.clone(),
                                sender.clone()).await?;
                            self.web_sock = Some(sock);
                        }
                };
                // send headers
                let name_bytes = self.hive_name.as_bytes();
                // let mut bytes = BytesMut::with_capacity(name_bytes.len() + 3);
                let mut bytes = BytesMut::with_capacity(name_bytes.len() + 2);
                bytes.put_u8(HEADER);
                // bytes.put_u8(PEER_REQUESTS);
                bytes.put_u8(HEADER_NAME);
                bytes.put_slice(name_bytes);
                info!(".... send headers: {:?}", bytes);
                self.send(bytes.freeze()).await?;

            },
            _ => {
                unimplemented!("finish this!!");
            }
        }

        debug!("shook");
        Ok(())
    }

    pub fn get_name(&self) -> String {
        let name = &*block_on(self.name.read());
        return String::from(name);
    }
    pub fn get_id_name(&self) -> String {
        let stream  =self.stream.as_ref().unwrap();
        let mm = format!("{:?}/{:?}", stream.local_addr().unwrap(), stream.peer_addr().unwrap());
        return format!("{}/{}, {:?}",self.hive_name, self.get_name(), mm);
    }
    pub fn receive_pong(&self) {
        debug!("RECEIVED PING {:?}", self.to_string());
        self.ack_check.store(false, Relaxed);
    }
    pub async fn ack(&self) {
        debug!("ACK {:?}", self.to_string());
        *self.last_received.write().await = SystemTime::now();
    }
    // when  hi is received, we send a hello
    pub async fn send_pong(&self) -> Result<()> {
        debug!("SEND PONG {:?}", self.to_string());
        let byte = Bytes::from_static(&[PONG]);
        self.send(byte).await
    }

    /** TODO, clean up, I'm not actually using this anyware, or refactor to a Ping/Pong */
    pub async fn _wave(&self) {
        let name_clone = self.name.clone();
        let addr_clone = self.address.clone();
        let mut perf_clone = self.peripheral.as_ref().unwrap().clone();
        let ack_check_clone = self.ack_check.clone();
        let mut sender_clone = self.event_sender.clone();
        let adr_clone = self.address.clone();
        let last_received_clone = self.last_received.clone();
        async_std::task::spawn(async move {
            'wave_loop: loop {
                debug!("send Hi {:?}, {}", name_clone.read().await, addr_clone);
                task::sleep(Duration::from_secs(ACK_DURATION)).await;
                let since_last_comm = SystemTime::now().duration_since(*last_received_clone.read().await);
                if since_last_comm.unwrap() > Duration::from_secs(ACK_DURATION) {
                    let nc = &*name_clone.read().await;
                    let mut bytes = BytesMut::with_capacity(nc.len() + 1);
                    bytes.put_u8(PING);
                    bytes.put_slice(nc.as_bytes());

                    match perf_clone.send(bytes.into()).await {
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("Error sending:: {:?}", e);
                            break 'wave_loop;
                        }
                    }
                    ack_check_clone.store(true, Relaxed);
                    task::sleep(Duration::from_secs(5)).await; // sleep 5 seconds for reply
                    if ack_check_clone.load(Relaxed) {
                        // No hello
                        debug!("KILL THIS PEER IS DEAD:: {:?}", name_clone.read().await);
                        sender_clone.send(SocketEvent::Hangup { from: adr_clone.clone() }).await.expect("failed to send hangup");
                        break 'wave_loop;
                    }
                }
            }
            trace!("Done Waving <<< {:?}", name_clone.read().await);
        });
    }

    pub async fn send(&self, msg: Bytes) -> Result<()> {
        debug!("SEND starts here {:?}", msg);
        if self.stream.is_some() {
            if self.web_sock.is_some() {
                info!("Sending message to web client at {:?} = {:?}", self.address, msg);
                self.web_sock.as_ref().unwrap().send_message(msg).await?;
            } else {
                debug!(">>>>>>>>>>>>>>>>  {:?} Send to peer {}: {:?}",self.get_id_name(), self.name.read().await, msg);
                self.send_on_stream(msg).await?;
            }
        } else if self.central.is_some() {
            #[cfg(feature = "bluetooth")]
                {
                    debug!("SEND to bluetooth {:?} from {:?}", msg, self.name);
                    let mut buff = BytesMut::new();
                    buff.put_slice(msg.as_ref());
                    let sender = self.central.as_ref().unwrap();
                    sender.send(buff).await;
                }
        } else if self.peripheral.is_some() {
            debug!("Send via bt peripheral");
            let mut buff = BytesMut::with_capacity(msg.len());//BytesMut::from(msg.to_vec());
            buff.put_slice(msg.as_ref());
            let b = buff.freeze();
            self.peripheral.as_ref().unwrap()
                .send(b.clone()).await.expect("failed to send something somewhere");
            debug!("sent...");
        } else {
            unimplemented!("cant send: {:?}", msg);
        }

        Ok(())
    }


    async fn send_on_stream(&self, message: Bytes) -> Result<bool> {
        let mut bytes = Vec::new();
        let msg_length: u32 = message.len() as u32;
        bytes.append(&mut msg_length.to_be_bytes().to_vec());
        bytes.append(&mut message.to_vec());
        self.stream.as_ref().unwrap().write(&bytes).await.expect("Failed to write to stream");
        self.stream.as_ref().unwrap().flush().await.expect("flush failed");
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


async fn read_loop(sender: UnboundedSender<SocketEvent>, stream: &TcpStream, peer_id_string:String) {
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
        debug!("{:?} waiting for read", peer_id_string);
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
                            // let msg = String::from(std::str::from_utf8(&size_buff).unwrap());
                            let msg = Bytes::from(size_buff);
                            debug!("<<<<<<<<<<<<<<<<< {:?} Read message: {:?}", peer_id_string, &msg);
                            let debug = msg.clone();
                            let se = SocketEvent::Message {
                                from,
                                msg,
                            };
                            if !sender.is_closed() {
                                sender.send(se).await.expect("Failed to send message");
                                debug!("{:?} send message {:?}", peer_id_string, debug);
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
    debug!("<< Peer run done");
}