// Much code "borrowed" from> https://book.async.rs/tutorial/implementing_a_client.html
use std::{
    collections::HashMap,
    fs,
    // mem::transmute,
    // net::{IpAddr, Ipv4Addr, SocketAddr},
    // str::from_utf8,
    // thread,
    thread::sleep,
};

use async_std::{
    io::BufReader,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
    sync::Arc,
    task,
};
use failure::_core::fmt::Debug;
use failure::_core::time::Duration;
use futures::{SinkExt, StreamExt};
use futures::channel::mpsc;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use spmc;
// use tokio::prelude::*;
use toml;

use crate::peer::{SocketEvent, Peer};
use crate::property::Property;
use std::error::Error;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;

#[derive(Debug)]
pub struct Hive {
    pub properties: HashMap<String, Property>,
    sender: Sender<String>,
    receiver: Receiver<String>,
    connect_to: Option<Box<str>>,
    listen_port: Option<Box<str>>,
    property_config: Option<toml::Value>,
    pub name: Box<str>,
    // peers: HashMap<String, Arc<TcpStream>>,
}

static mut peers: Vec<Peer> = Vec::new();

unsafe fn addPeer(peer:Peer) -> bool{
    for p in &peers {
        if p.name == peer.name {
            return false;
        }
    }
    peers.push(peer);
    true
}


impl Hive {
    // fn add_peer(&mut self, name:String, stream: Arc<TcpStream>){
    //     self.peers.insert(name, stream);
    // }

    pub fn new(name: &str, toml_path: &str) -> Hive{
        let foo: String = fs::read_to_string(toml_path).unwrap().parse().unwrap();
        let config: toml::Value = toml::from_str(&foo).unwrap();

        // Hive::parse_properties(&config)
        let connect_to = match config.get("connect") {
            Some(v) => {
                Some(String::from(v.as_str().unwrap()).into_boxed_str())
            },
            _ => None
        };
        let listen_port = match config.get("listen") {
            Some(v) => {
                Some(String::from(v.as_str().unwrap()).into_boxed_str())
            },
            _ => None
        };

        let mut props:HashMap::<String, Property> = HashMap::new();

        let (tx,mut rx) = mpsc::unbounded();
        let mut hive = Hive {
            properties: props,
            sender: tx,
            receiver: rx,
            connect_to,
            listen_port,
            property_config: None,
            name: String::from(name).into_boxed_str(),
            // peers: HashMap::new()
        };

        let properties = config.get("Properties");
        if !properties.is_none() {
            match properties {
                Some(p) => hive.parse_properties(p),
                _ => ()
            }
        };
        return hive;
    }

    pub fn get_mut_property(&mut self, key: &str) -> Option<&mut Property> {
        println!("properties: {:?}", self.properties.keys());
        let op = self.properties.get_mut(key);

        return op
    }



    fn parse_properties(&mut self, properties: &toml::Value) {
        let p_val = properties.as_table().unwrap();
        self.property_config = Some(properties.clone());
        let mut props = &mut self.properties;
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
            // println!("||{:?} == {:?}",key, val);
        }
    }

    /*
        Socket Communication:
            -- handshake / authentication
            -- transfer config /toml

            message [message size, message]
                message size = u32 unsigned 32 bite number, 4 bytes in length
     */




    async fn accept_loop(sender: Sender<SocketEvent>, addr: impl ToSocketAddrs) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        let mut incoming = listener.incoming();
        while let Some(stream) = incoming.next().await {
            let stream = stream?;
            println!("Accepting from: {}", stream.peer_addr()?);
            match stream.peer_addr() {
                Ok(peer) => {
                    let se = SocketEvent::NewPeer {
                        name: peer.to_string(),
                        stream,
                    };
                    println!("********* SOCKET EVENT: {:?}", se);
                    sender.clone().send(se).await;
                },
                Err(e) => eprintln!("No peer address: {:?}", e),
            }
        }

        print!("**************************************** HANGUP");
        Ok(())
    }

    pub async fn run(&mut self) -> Result<bool> {

        // I'm a client
        if !self.connect_to.is_none() {
            println!("Connect To: {:?}", self.connect_to);
            let address = self.connect_to.as_ref().unwrap().to_string().clone();
            let (mut send_chan,mut receive_chan) = mpsc::unbounded();
            let hive_name = self.name.clone();
            task::spawn(async move{
                if let mut stream = TcpStream::connect(address).await {
                    match stream {
                        Ok(mut s) => {
                            match s.peer_addr() {
                                Ok(peer) => {
                                    let name = format!("SERVER PEER {:?}", peer.to_string());
                                    let mut p = Peer::new(name, s, send_chan, None);
                                    p.send("Hi from client").await;
                                },
                                Err(e) => eprintln!("No peer address: {:?}", e),
                            }
                        },
                        _ => {eprintln!("Nope")}
                    }
                }
            });
            // listen for messages from server
            while let Some(event) = receive_chan.next().await {
                match event {
                    SocketEvent::NewPeer{name, stream} => {},
                    SocketEvent::Message{from, msg} => {
                        self.gotMessage(msg);
                    },
                }
            }
        }

        // I'm a server
        if !self.listen_port.is_none() {
            let port = self.listen_port.as_ref().unwrap().to_string().clone();

            println!("{:?} Listening for connections on {:?}",self.name, self.listen_port);
            let (send_chan,mut receive_chan) = mpsc::unbounded();
            let send_chan_clone = send_chan.clone();

            // listen for connections loop
            let p = port.clone();
            task::spawn( async move {
                match Hive::accept_loop(send_chan.clone(), p).await {
                    Err(e) => eprintln!("Failed accept loop: {:?}",e),
                    _ => (),
                }

            });


            let hive_name = self.name.clone();

            while let Some(event) = receive_chan.next().await {
                let send_chan = send_chan_clone.clone();
                match event {
                    SocketEvent::NewPeer{name, stream} => {
                        let pname = format!("CLIENT PEER {:?}", name);
                        let mut p = Peer::new(pname, stream, send_chan, None);
                        self.sendProperties(&p).await;

                    },
                    SocketEvent::Message{from, msg} => {
                        println!("{:?} <<<< New Message: {:?}",&hive_name, msg);
                        self.gotMessage(msg);
                    },
                }
            }

        }
        return Result::Ok(true)
    }
    async fn sendProperties(&self, peer:&Peer){
        let str = toml::to_string(self.property_config.as_ref().unwrap()).unwrap();
        let mut message = String::from("|P|");
        message.push_str(&str);
        peer.send(message.as_str()).await;
    }
    fn gotMessage(&mut self, msg:String){
        println!("process message: {:?} = {:?}",self.name,  msg);
        match &msg[0..3]{
            "|P|" => {
                // got properties
                let (_,rest) = msg.split_at(3);
                let value:toml::Value = toml::from_str(rest).unwrap();
                self.parse_properties(&value);

            },
            _ => println!("got message {:?}", msg)
        }
    }
}
