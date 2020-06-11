// Much code "borrowed" from> https://book.async.rs/tutorial/implementing_a_client.html
use std::{
    collections::HashMap,
    fs,
};

use async_std::{
    net::{TcpListener, TcpStream, ToSocketAddrs},
    task,
};
use futures::{SinkExt, StreamExt};
use futures::channel::mpsc;
use futures::executor::block_on;
use toml;

use crate::handler::{Handler, property_to_sock_str};
use crate::peer::{Peer, SocketEvent};
use crate::property::Property;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;

// #[derive(Debug)]
pub struct Hive {
    pub properties: HashMap<String, Property>,
    sender: Sender<SocketEvent>,
    receiver: Receiver<SocketEvent>,
    connect_to: Option<Box<str>>,
    listen_port: Option<Box<str>>,
    property_config: Option<toml::Value>,
    pub name: Box<str>,
    peers: Vec<Peer>,
}


pub (crate) const PROPERTIES:&str = "|P|";
pub (crate) const PROPERTY:&str = "|p|";

// static mut PEERS: Vec<Peer> = Vec::new();
//
// unsafe fn add_peer(peer:Peer) -> bool{
//     for p in &PEERS {
//         if p.name == peer.name {
//             return false;
//         }
//     }
//     PEERS.push(peer);
//     true
// }



impl Hive {

    // fn add_peer(&mut self, name:String, stream: Arc<TcpStream>){
    //     self.peers.insert(name, stream);
    // }

    pub fn new_from_str(name: &str, properties: &str) -> Hive{
        let config: toml::Value = toml::from_str(properties).unwrap();
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

        let (send_chan, receive_chan) = mpsc::unbounded();
        let mut hive = Hive {
            properties: props,
            sender: send_chan,
            receiver: receive_chan,
            connect_to,
            listen_port,
            property_config: None,
            name: String::from(name).into_boxed_str(),
            peers: Vec::new(),
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

    pub fn get_handler(&self) -> Handler {
        return Handler {
            sender: self.sender.clone(),
        }
    }

    pub fn new(name: &str, toml_path: &str) -> Hive{
        let foo: String = fs::read_to_string(toml_path).unwrap().parse().unwrap();
        Hive::new_from_str(name, foo.as_str())
    }

    pub fn get_mut_property(&mut self, key: &str) -> Option<&mut Property> {
        if !self.properties.contains_key(key){
            let p = Property::new(key, None);
            self.set_property(p);
        }

        let op = self.properties.get_mut(key);
        return op
    }
    fn has_property(&self, key:&str)->bool{
        return self.properties.contains_key(key)
    }

    fn set_property(&mut self, property: Property ){
        let name = property.get_name().clone();
        if self.has_property(name) {
            /*
            This calls emit on the existing property
             */
            self.get_mut_property(name).unwrap().set_from_prop(property);
        }else {

            // TODO when added for first time, no change event is emitted, EMIT CHANGE!!
            //  De couple add_property and get_mut_property
            self.properties.insert(String::from(name), property);
        }
    }

    fn parse_property(&mut self, key:&str, property: Option<&toml::Value>) {
        let p = Property::from_toml(key, property);
        self.set_property(p);
    }

    fn parse_properties(&mut self, properties: &toml::Value) {
        let p_val = properties.as_table().unwrap();
        self.property_config = Some(properties.clone());
        for key in p_val.keys() {
            let val = p_val.get(key);
            self.parse_property(key,val)
        }
    }

    /*
        Socket Communication:
            -- handshake / authentication
            -- transfer config /toml

            message [message size, message]
                message size = u32 unsigned 32 bite number, 4 bytes in length
     */

    async fn accept_loop(mut sender: Sender<SocketEvent>, addr: impl ToSocketAddrs) -> Result<()> {
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
                    sender.send(se).await.expect("failed to send message");
                },
                Err(e) => eprintln!("No peer address: {:?}", e),
            }
        }

        Ok(())
    }

    pub async fn run(& mut self) -> Result<bool> {

        // I'm a client
        if !self.connect_to.is_none() {
            println!("Connect To: {:?}", self.connect_to);
            let address = self.connect_to.as_ref().unwrap().to_string().clone();
            let send_chan = self.sender.clone();
            task::spawn(async move{
                let stream = TcpStream::connect(address).await;
                match stream {
                    Ok( s) => {
                        match s.peer_addr() {
                            Ok(peer) => {
                                let name = peer.to_string();

                                let se = SocketEvent::NewPeer {
                                    name,
                                    stream: s,
                                };
                                send_chan.clone().send(se).await.expect("failed to send pear");
                                // p.send("Hi from client").await;
                            },
                            Err(e) => eprintln!("No peer address: {:?}", e),
                        }
                    },
                    _ => {eprintln!("Nope")}
                };

            });
            // listen for messages from server
            while let Some(event) = self.receiver.next().await {
                match event {
                    // Clients should never receive a new peer message
                    SocketEvent::NewPeer{name, stream} => {
                        let p = Peer::new(
                            name,
                            stream,
                            self.sender.clone());
                        self.peers.push(p);
                    },
                    SocketEvent::Message{from, msg} => {
                        self.got_message(from.as_str(), msg);
                    },
                }
            }
        }

        // I'm a server
        if !self.listen_port.is_none() {
            let port = self.listen_port.as_ref().unwrap().to_string().clone();
            println!("{:?} Listening for connections on {:?}",self.name, self.listen_port);
            let send_chan = self.sender.clone();
            // listen for connections loop
            let p = port.clone();
            task::spawn( async move {
                match Hive::accept_loop(send_chan.clone(), p).await {
                    Err(e) => eprintln!("Failed accept loop: {:?}",e),
                    _ => (),
                }
            });
            
            while let Some(event) = self.receiver.next().await {
                match event {
                    SocketEvent::NewPeer{name, stream} => {
                        let pname = name;
                        let p = Peer::new(
                            pname,
                            stream,
                            self.sender.clone());
                        self.send_properties(&p).await;
                        self.peers.push(p);

                    },
                    SocketEvent::Message{from, msg} => {
                        self.got_message(from.as_str(), msg);
                    },
                }
            }

        }
        return Result::Ok(true)
    }

    async fn send_properties(&self, peer:&Peer){
        let str = toml::to_string(self.property_config.as_ref().unwrap()).unwrap();
        let mut message = String::from(PROPERTIES);
        message.push_str(&str);
        peer.send(message.as_str()).await;
    }

    async fn broadcast(&self, msg: Option<String>, except:&str){
        /*
        Dont broadcast to the same Peer that the original change came from
        to prevent an infinite re-broadcast loop
         */
        match msg {
            Some(m) => {
                for p in &self.peers {
                    // println!("{:?}",p.name);
                    if p.name != except {
                        p.send(m.as_str()).await;
                    }
                }
            },
            _ => {}
        }

    }

    fn got_message(&mut self, from:&str, msg:String){
        let (msg_type,message) = msg.split_at(3);
        match msg_type{
            PROPERTIES => {
                let value:toml::Value = toml::from_str(message).unwrap();
                self.parse_properties(&value);
            },
            PROPERTY => {
                let p_toml: toml::Value = toml::from_str(message).unwrap();
                let property = Property::from_table(p_toml.as_table().unwrap());
                let broadcast_message = property_to_sock_str(property.as_ref());
                self.set_property(property.unwrap());

                block_on(self.broadcast(broadcast_message, from ));

            }
            _ => println!("got message {:?}", msg)
        }
    }
}
