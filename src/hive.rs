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
use std::borrow::BorrowMut;

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

static mut PEERS: Vec<Peer> = Vec::new();

unsafe fn add_peer(peer:Peer) -> bool{
    for p in &PEERS {
        if p.name == peer.name {
            return false;
        }
    }
    PEERS.push(peer);
    true
}


impl Hive {
    // fn add_peer(&mut self, name:String, stream: Arc<TcpStream>){
    //     self.peers.insert(name, stream);
    // }

    pub fn new_from_str(name: &str, properties: &str) -> Hive{
        let config: toml::Value = toml::from_str(properties).unwrap();
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

        let props:HashMap::<String, Property> = HashMap::new();

        let (tx, rx) = mpsc::unbounded();
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

    pub fn new(name: &str, toml_path: &str) -> Hive{
        let foo: String = fs::read_to_string(toml_path).unwrap().parse().unwrap();
        Hive::new_from_str(name, foo.as_str())
    }

    pub fn get_mut_property(&mut self, key: &str) -> Option<&mut Property> {
        if !self.properties.contains_key(key){
            let mut p = Property::new();
            self.add_property(key, p);
        }

        let op = self.properties.get_mut(key);
        return op
    }
    fn has_property(&self, key:&str)->bool{
        return self.properties.contains_key(key)
    }

    fn add_property(&mut self, p_name: &str, property:Property ){
        if self.has_property(p_name) {
            self.get_mut_property(p_name).unwrap().set_from_prop(&property);
        }else {
            self.properties.borrow_mut().insert(String::from(p_name), property);
        }
    }

    fn parse_properties(&mut self, properties: &toml::Value) {
        let p_val = properties.as_table().unwrap();
        self.property_config = Some(properties.clone());
        // let props = &mut self.properties;
        for key in p_val.keys() {
            let val = p_val.get(key);
            match val {
                Some(v) if v.is_str() => {
                    self.add_property(key, Property::from_str(v.as_str().unwrap()))
                },
                Some(v) if v.is_integer() => {
                    self.add_property(key, Property::from_int(v.as_integer().unwrap()))
                },
                Some(v) if v.is_bool() => {
                    self.add_property(key, Property::from_bool(v.as_bool().unwrap()));
                },
                Some(v) if v.is_float() => {
                    self.add_property(key, Property::from_float(v.as_float().unwrap()));
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
                    sender.clone().send(se).await.expect("failed to send message");
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
            let (send_chan,mut receive_chan) = mpsc::unbounded();
            // let hive_name = self.name.clone();
            task::spawn(async move{
                if let stream = TcpStream::connect(address).await {
                    match stream {
                        Ok( s) => {
                            match s.peer_addr() {
                                Ok(peer) => {
                                    let name = format!("SERVER PEER {:?}", peer.to_string());
                                    let p = Peer::new(name, s, send_chan, None);
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
                        self.got_message(msg);
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
                        let p = Peer::new(pname, stream, send_chan, None);
                        self.send_properties(&p).await;

                    },
                    SocketEvent::Message{from, msg} => {
                        println!("{:?} <<<< New Message: {:?}",&hive_name, msg);
                        self.got_message(msg);
                    },
                }
            }

        }
        return Result::Ok(true)
    }
    fn properties_str(&self) -> String {
        format!("{:?}", self.properties)
    }

    async fn send_properties(&self, peer:&Peer){
        let str = toml::to_string(self.property_config.as_ref().unwrap()).unwrap();
        let mut message = String::from("|P|");
        message.push_str(&str);
        peer.send(message.as_str()).await;
    }
    fn got_message(&mut self, msg:String){
        println!("process message: {:?} = {:?}",self.name,  msg);
        match &msg[0..3]{
            "|P|" => {
                // got properties
                let (_,rest) = msg.split_at(3);
                let value:toml::Value = toml::from_str(rest).unwrap();
                self.parse_properties(&value);
                println!("properties: {:?}", self.properties_str());

            },
            _ => println!("got message {:?}", msg)
        }
    }
}
