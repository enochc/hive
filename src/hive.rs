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
};
use failure::_core::time::Duration;
use futures::{SinkExt, StreamExt};
use futures::channel::mpsc;
// use tokio::prelude::*;
use toml;

use crate::models::Property;

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

        let mut incoming = listener.incoming();

        while let Some(stream) = incoming.next().await {
            let stream = stream?;
            println!("Accepting from: {}", stream.peer_addr()?);
            let sender = sender.clone();
            task::spawn(async move {

                let mut reader = BufReader::new(&stream);
                // Accept new client connections
                loop {
                    let mut sender = sender.clone();
                    let mut size_buff = [0;4];
                    // While next message zize is > 0 bites read messages
                    while let Ok(read) = reader.read(&mut size_buff).await {
                        if read > 0 {
                            let message_size = as_u32_be(&size_buff);
                            println!("SIZE: {:?}", message_size);
                            let mut size_buff = vec![0u8; message_size as usize];
                            match reader.read_exact(&mut size_buff).await {
                                Ok(_t) => {
                                    let msg = String::from(std::str::from_utf8(&size_buff).unwrap());
                                    println!("read: {:?}", msg);
                                    sender.send(msg).await;
                                },
                                _ => eprintln!("Failed to read message")
                            }
                        }
                    }
                }
            });
        }
        Ok(())
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

    pub async fn run(&self) -> Result<bool> {

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
            println!("Listening for connections on {:?}", self.listen_port);
            let (tx,mut rx) = mpsc::unbounded();
            // receive message loop
            task::spawn(async move{
                println!("runniong listener");
                while let Some(msg) = rx.next().await {
                    println!("<<< MESSAGE RECEIVED: {:?}", msg);
                }
                println!("done listener");
            });
            // send message loop

            // listen for connections loop
            let port = self.listen_port.as_ref().unwrap().to_string().clone();
            task::spawn( async move {
                match Hive::accept_loop(tx.clone(),port).await {
                    Err(e) => eprintln!("Failed accept loop: {:?}",e),
                    _ => (),
                }

            }).await;

        }
        return Result::Ok(true)
    }
}