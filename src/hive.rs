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
    task,
    sync::Arc,
};
use failure::_core::time::Duration;
use futures::{SinkExt, StreamExt};
use futures::channel::mpsc;
// use tokio::prelude::*;
use toml;

use crate::models::Property;
use failure::_core::fmt::Debug;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;

#[derive(Debug)]
pub struct Hive {
    pub properties: HashMap<String, Property>,
    // sender: Sender<u8>,
    // receiver: Receiver<u8>,
    connect_to: Option<Box<str>>,
    listen_port: Option<Box<str>>,
    property_config: Option<toml::Value>,
    // peers: HashMap<String, Arc<TcpStream>>,
}


fn as_u32_be(array: &[u8; 4]) -> u32 {
    ((array[0] as u32) << 24) +
        ((array[1] as u32) << 16) +
        ((array[2] as u32) <<  8) +
        ((array[3] as u32) <<  0)
}

#[derive(Debug)]
enum SocketEvent {
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

impl Hive {
    // fn add_peer(&mut self, name:String, stream: Arc<TcpStream>){
    //     self.peers.insert(name, stream);
    // }

    pub fn new(toml_path: &str) -> Hive{
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

        let mut hive = Hive {
            properties: props,
            // sender: tx,
            // receiver: rx,
            connect_to,
            listen_port,
            property_config: None,
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
            println!("||{:?} == {:?}",key, val);
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
                    sender.clone().send(se).await;
                },
                Err(e) => eprintln!("No peer address: {:?}", e),
            }
        }
        Ok(())
    }
    async fn read_loop(sender: Sender<SocketEvent>, stream: Arc<TcpStream>){
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



    #[allow(irrefutable_let_patterns)]
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

    pub async fn run(& self) -> Result<bool> {

        // I'm a client
        if !self.connect_to.is_none() {
            println!("Connect To: {:?}", self.connect_to);
            // let (tx,rx): (Sender<i32>, Receiver<i32>) = mpsc::unbounded();
            let address = self.connect_to.as_ref().unwrap().to_string().clone();
            task::spawn(async move{
                match Hive::connect(&address).await {
                    Err(e) => eprintln!("Error connecting {:?}",e),
                    _ => (),
                }
            }).await;

        }

        // I'm a server
        if !self.listen_port.is_none() {
            let mut peers: HashMap<String, Arc<TcpStream>> = HashMap::new();
            let port = self.listen_port.as_ref().unwrap().to_string().clone();

            println!("Listening for connections on {:?}", self.listen_port);
            let (tx,mut rx) = mpsc::unbounded();
            let tx_clone = tx.clone();
            // let mut peers = &self.peers;
            // receive SocketEvent loop
            task::spawn(async move{
                println!("running listener");
                while let Some(event) = rx.next().await {
                    match event {
                        SocketEvent::NewPeer{name, stream} => {
                            let stream = Arc::new(stream);
                            println!("<<< New Peer: {:?}", name);
                            // self.add_peer(name, Arc::clone(&stream));
                            // peers.insert(name, Arc::clone(&stream));
                            // sync properties to client (transfer config)

                            // Start read loop
                            let tx_clone = tx_clone.clone();
                            task::spawn(async move{
                                Hive::read_loop(tx_clone, Arc::clone(&stream)).await;
                            });
                        },
                        SocketEvent::Message{from, msg} => {
                             println!("<<<< New Message: {:?}", msg)
                        },
                    }
                }
                println!("done listener");
            });
            // send message loop

            // listen for connections loop
            let p = port.clone();
            task::spawn( async move {
                match Hive::accept_loop(tx.clone(),p).await {
                    Err(e) => eprintln!("Failed accept loop: {:?}",e),
                    _ => (),
                }

            }).await;

        }
        return Result::Ok(true)
    }
}