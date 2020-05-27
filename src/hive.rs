
use std::collections::HashMap;
use std::fs;
use std::thread;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::*;
use toml;
use std::str::from_utf8;

use crate::models::Property;
use serde::de::Error;
use std::sync::mpsc::{Sender, Receiver};
use std::sync::mpsc;
use futures::executor::block_on;

pub struct Hive {
    pub properties: HashMap<String, Property>,
    sender: Sender<u8>,
    receiver: Receiver<u8>,
    connect_to: Option<Box<str>>,
    listen_port: u16,
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

        // toml.get("connect").unwrap().as_bool().unwrap();
        let connect_port = match toml.get("connect") {
            Some(v) => {
                Some(String::from(v.as_str().unwrap()).into_boxed_str())
            },
            _ => None
        };
        let listen_port = match toml.get("listen") {
            Some(port) => port.as_integer().unwrap() as u16,
            _ => 0
        };

        let (tx, rx) = mpsc::channel();

        Hive {
            // properties:Default::default(),
            properties: props,
            sender: tx,
            receiver: rx,
            connect_to: connect_port,
            listen_port
        }
    }

    /*
        Socket Communication:
            -- handshake / authentication
            -- transfer config /toml
     */


    #[tokio::main]
    pub async fn listen(port: u16) -> Result<bool, std::io::Error> {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port as u16);
        let mut listener = TcpListener::bind(addr).await?;

        loop {
            let (mut socket, _) = listener.accept().await?;
            tokio::spawn( async move {
                // BUFFER SIZE IS IMPORTANT!!
                let mut buf = [0; 1024];
                // In a loop, read data from the socket
                loop {
                    let n = match socket.read(&mut buf).await {
                        // socket closed
                        Ok(n) if n == 0 => { return; },
                        Ok(n) => { n },
                        Err(e) => {
                            eprintln!("failed to read from socket; err = {:?}", e);
                            return;
                        }
                    };
                    let msg = from_utf8(&buf).unwrap();
                    if(msg.len()>0){
                        println!("RECEIVED: {:?}", from_utf8(&buf[0..n]));
                    }
                }
                println!("Done listening");
            });
        }
        return Result::Ok(true)
    }

    #[tokio::main]
    pub async fn run(&self) -> Result<bool, std::io::Error> {

        if !self.connect_to.is_none() {
            let addr = &self.connect_to.as_ref().unwrap().to_string();
            if let mut socket = TcpStream::connect(addr).await? {
                println!("Connected to the server!");
                socket.write("all That".as_bytes()).await?;

            } else {
                println!("Couldn't connect to server...");
            }
        }

        if self.listen_port.gt(&0) {
            println!("Listening for connections on {:?}", self.listen_port);
            let (tx,rx): (Sender<i32>, Receiver<i32>) = mpsc::channel();
            let sender_clone = tx.clone();
            let port = self.listen_port.clone();
            thread::spawn( move ||{
                Hive::listen(port);//.await?;
                sender_clone.send(1);
            });
            //Wait here indefinately to listen
            let done = rx.recv();

        }

        return Result::Ok(true)
    }
}