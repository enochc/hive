
use std::collections::HashMap;
use std::fs;
use std::thread;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
// use tokio::net::{TcpListener, TcpStream};
use async_std::{
    io::BufReader,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
    task,
    task::JoinHandle,
};
use tokio::prelude::*;
use toml;
use std::str::from_utf8;
use std::mem::transmute;

use crate::models::Property;
use serde::de::Error;
// use std::sync::mpsc::{Sender, Receiver};
// use std::sync::mpsc;
use futures::channel::mpsc;
use futures::executor::block_on;
use std::borrow::Borrow;
use std::thread::sleep;
use failure::_core::time::Duration;
use futures::channel::mpsc::UnboundedSender;
use futures::SinkExt;

// use async_std::task;
// use async_std::task::JoinHandle;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;

// #[derive(Send)]
pub struct Hive {
    pub properties: HashMap<String, Property>,
    // sender: Sender<u8>,
    // receiver: Receiver<u8>,
    connect_to: Option<Box<str>>,
    listen_port: Option<Box<str>>,
}


fn as_u32_be(array: &[u8; 4]) -> u32 {
    ((array[0] as u32) << 24) +
        ((array[1] as u32) << 16) +
        ((array[2] as u32) <<  8) +
        ((array[3] as u32) <<  0)
}
impl Hive {

    pub fn new(toml_path: &str) -> Hive{
        let foo: String = fs::read_to_string(toml_path).unwrap().parse().unwrap();
        let config: toml::Value = toml::from_str(&foo).unwrap();

        Hive::parse_properties(&config)
    }
    pub fn get_mut_property(&mut self, key: &str) -> Option<&mut Property> {
        println!("properties: {:?}", self.properties.keys());
        let op = self.properties.get_mut(key);

        return op
    }

    fn parse_properties(toml: &toml::Value) -> Hive {

        let mut props:HashMap::<String, Property> = HashMap::new();

        let properties = toml.get("Properties");
        if !properties.is_none() {
            match properties{
                Some(p_val) => {
                    let p_val = p_val.as_table().unwrap();
                    for key in p_val.keys() {
                        let val = p_val.get(key);
                        match val {
                            Some(v) if v.is_str() => {
                                props.insert(String::from(key), Property::from_str(v.as_str().unwrap()));
                                // props[key] = ;
                            },
                            Some(v) if v.is_integer() => {
                                props.insert(String::from(key), Property::from_int(v.as_integer().unwrap()));
                            },
                            Some(v) if v.is_bool() => {
                                props.insert(String::from(key), Property::from_bool(v.as_bool().unwrap()));
                            },
                            Some(v) if v.is_float() => {
                                props.insert(String::from(key), Property::from_float(v.as_float().unwrap()));
                            },
                            _ => {
                                println!("<<Failed to Set Property: {:?}", key)
                            }
                        };
                        println!("||{:?} == {:?}",key, val);
                    }
                },
                _ => {
                    println!("Failed to unwrap connect address");
                }
            }
        }

        let connect_port = match toml.get("connect") {
            Some(v) => {
                Some(String::from(v.as_str().unwrap()).into_boxed_str())
            },
            _ => None
        };
        let listen_port = match toml.get("listen") {
            Some(v) => {
                Some(String::from(v.as_str().unwrap()).into_boxed_str())
            },
            _ => None
        };

        // let (tx, rx) = mpsc::channel();

        Hive {
            properties: props,
            // sender: tx,
            // receiver: rx,
            connect_to: connect_port,
            listen_port
        }
    }

    /*
        Socket Communication:
            -- handshake / authentication
            -- transfer config /toml

            message [message size, message]
                message size = u32 unsigned 32 bite number, 4 bytes in length
     */


    async fn accept_loop(sender: Sender<String>, addr: impl ToSocketAddrs) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;

        // let (broker_sender, broker_receiver) = mpsc::unbounded(); // 1
        //TODO: this: // let _broker_handle = task::spawn(broker_loop(broker_receiver));
        let mut incoming = listener.incoming();
        while let Some(stream) = incoming.next().await {
            let stream = stream?;
            println!("Accepting from: {}", stream.peer_addr()?);
            // spawn_and_log_error(connection_loop(broker_sender.clone(), stream));
            sender.clone().send(String::from("Accepted!!"));
        }
        Ok(())
    }

    pub async fn listen(sender: Sender<String>, port: u16) -> Result<bool> {
        // let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port as u16);
        // let mut listener = TcpListener::bind(addr).await?;
        //
        // // loop {
        //     let (mut socket, _) = listener.accept().await?;
        //     println!("<<< started socket");
        //     task::spawn(  async move {
        //         // In a loop, read data from the socket
        //         let mut msg_length: u32 = 0;
        //         loop {
        //             println!("<<< loop");
        //             /*
        //                 let n = match socket.read(&mut buf).await {
        //                 Ok(n) if n == 0 => { return; },
        //                 Ok(n) => { n },
        //                 Err(e) => {
        //                     eprintln!("failed to read from socket; err = {:?}", e);
        //                     return;
        //              */
        //             if msg_length == 0 {
        //                 let mut buff = [0;4];
        //                 let n = match  block_on(socket.read(&mut buff)){//.await {
        //                     Ok(n) if n == 0 => { return; },
        //                     Ok(n) => n ,
        //                     Err(e) => return
        //                 };
        //
        //                 msg_length = as_u32_be(&buff);
        //                 println!("MSG LENGTH: {:?}", msg_length);
        //             } else {
        //                 // let mut buf = [0; 1024];
        //                 let mut msg = String::new();
        //                 let n = block_on(socket.read_to_string(&mut msg)).unwrap();//.await.unwrap();
        //                 if n as u32 == msg_length {
        //                 //     println!("RECEIVED: {:?}", from_utf8(&buf[0..n]));
        //                     println!("RECEIVED: {:?} len: {:?}", msg, n);
        //                     &sender.send(msg.clone());
        //
        //                     msg_length = 0;
        //                 }
        //             }
        //         }
        //         println!("<<< break loop");
        //     });
        // // }
        Ok(true)
    }

    // #[tokio::main]
    pub async fn connect(address: &str) -> Result<bool> {
        if let mut socket = TcpStream::connect(address).await? {
            println!("Connected to the server!");
            let msg = "all That";
            let mut bytes = Vec::new();
            let msg_length: u32 = msg.len() as u32;
            bytes.append(&mut msg_length.to_be_bytes().to_vec());
            bytes.append(&mut msg.as_bytes().to_vec());
            let n = socket.write(&bytes).await?;
            println!("<<< written {:?}", n);
            sleep(Duration::from_secs(1));

        } else {
            println!("Couldn't connect to server...");
            // return std::io::Error::new("Nope");
        }
        Result::Ok(true)
    }

    // #[tokio::main]
    pub async fn run(&self) -> Result<bool> {

        if !self.connect_to.is_none() {
            println!("Connect To: {:?}", self.connect_to);
            let (tx,rx): (Sender<i32>, Receiver<i32>) = mpsc::unbounded();
            // let sender_clone = tx.clone();
            // let address = self.connect_to.clone();
            let address = self.connect_to.as_ref().unwrap().to_string().clone();
            task::spawn(async move{
                Hive::connect(&address).await;
                // sender_clone.send(2);
            }).await;
            //Wait here indefinately to connection
            // let done = rx.recv();
        }

        if !self.listen_port.is_none() {
            println!("Listening for connections on {:?}", self.listen_port);
            // let (tx,rx): (Sender<String>, Receiver<String>) = mpsc::unbounded();
            let (tx,rx) = mpsc::unbounded();
            // let sender_clone = tx.clone();
            let port = self.listen_port.as_ref().unwrap().to_string().clone();
            task::spawn( async move {
                Hive::accept_loop(tx.clone(),port).await;
                // match block_on(Hive::listen(sender_clone, port)){
                //     Ok(r) => print!("__{:?}__", r),
                //     Err(e) => println!("ERROR!! {:?}",e)
                // };//.await?;
                // sender_clone.send(1);
            }).await;
            //Wait here indefinately to listen
            // loop {
            //     let done = rx.recv();
            //     println!("<< DONE!! {:?}", done);
            // }


        }

        return Result::Ok(true)
    }
}