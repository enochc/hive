// Much code "borrowed" from> https://book.async.rs/tutorial/implementing_a_client.html
use std::{collections::HashMap, fs};
use crate::signal::Signal;
use async_std::{
    net::{TcpListener, TcpStream, ToSocketAddrs},
    task,
};
use futures::{SinkExt, StreamExt};
use futures::channel::mpsc;
use toml;

use crate::handler::{Handler, };
use crate::peer::{Peer, SocketEvent};
use crate::property::{Property, properties_to_sock_str};

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
    pub name: Box<str>,
    peers: Vec<Peer>,
    pub message_received: Signal<String>
}


pub (crate) const PROPERTIES: &str = "|P|";
pub (crate) const PROPERTY: &str = "|p|";
pub (crate) const DELETE: &str = "|d|";
pub (crate) const PEER_MESSAGE: &str = "|s|";
pub (crate) const HEADER: &str = "|H|";
pub (crate) const PEER_MESSAGE_DIV: &str = "|=|";
pub (crate) const REQUEST_PEERS: &str = "<p|";
pub (crate) const ACK:&str = "<<|";



impl Hive {
    fn is_sever(&self) ->bool{
        self.listen_port.is_some()
    }

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
            name: String::from(name).into_boxed_str(),
            peers: Vec::new(),
            message_received: Default::default(),
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

    // servers run this to accept client connections
    async fn accept_loop(mut sender: Sender<SocketEvent>, addr: impl ToSocketAddrs) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        let mut incoming = listener.incoming();
        while let Some(stream) = incoming.next().await {
            let stream = stream?;
            println!("Accepting from: {}", stream.peer_addr()?);
            match stream.peer_addr() {
                Ok(addr) => {
                    let se = SocketEvent::NewPeer {
                        name: addr.to_string(),
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
                            Ok(_addr) => {
                                let se = SocketEvent::NewPeer {
                                    name:String::from(""),
                                    stream: s,
                                };
                                send_chan.clone().send(se).await.expect("failed to send peer");
                            },
                            Err(e) => eprintln!("No peer address: {:?}", e),
                        }
                    },
                    _ => {eprintln!("Nope")}
                };

            });
            // listen for messages from server
            self.receive_events(false).await;
            println!("CLIENT DONE");
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
            self.receive_events(true).await;
            println!("SERVER DONE");

        }
        return Result::Ok(true)
    }

    async fn receive_events(&mut self, is_server:bool){
        while !self.sender.is_closed() {
            match self.receiver.next().await.unwrap() {
                SocketEvent::NewPeer { name, stream } => {
                    let p = Peer::new(
                        name,
                        stream,
                        self.sender.clone());
                    if is_server {
                        self.send_properties(&p).await;
                    }
                    // Send the header with my "peer name"
                    self.send_header(&p).await;
                    self.peers.push(p);

                    /*
                     //self.emit_peers().await;
                    It makes sense to call this here, but it's better if we
                    wait for the peer repsponse with it's header info so we
                    wave the actual peer name that it's given us to response with
                    so instead its called from the "set_headers_from_peer" method
                     */
                },
                SocketEvent::Message { from, msg } => {
                    self.got_message(from.as_str(), msg).await;
                },
                SocketEvent::Hangup {from} => {
                    for x in 0..self.peers.len(){
                        if self.peers[x].address() == from {
                            self.peers.remove(x);
                            self.emit_peers().await;
                            break;
                        }
                    }
                }
            }
        }
    }

    fn peer_string(& self)->String {
        // name:address,name:address
        let mut peers_string = String::new();
        let myadr = self.listen_port.as_ref().expect("No port").clone();
        let myname = String::from(self.name.as_ref());
        peers_string.push_str(&format!("{}|{}", myname, myadr));

        for x in 0..self.peers.len(){
            {peers_string.push_str(",")};
            let p = &self.peers[x];
            let adr = p.address();
            let name = p.name.clone();
            peers_string.push_str(&format!("{}|{}", name, adr))
        };
        return peers_string;
    }

    async fn emit_peers(&mut self) {
        let peer_str= format!("{}{}", REQUEST_PEERS, self.peer_string());
        for p in &self.peers {
            if p.update_peers {
                let stream = p.stream.clone();
                let msg = peer_str.clone();
                task::spawn(  async move{
                    Peer::send_on_stream(stream, &msg).await.unwrap();
                });
            }
        }
    }

    async fn send_to_peer(&self, msg:&str, peer_name:&str){
        let p = self.get_peer_by_name(peer_name);
        match p {
            Some(peer) => {
                let msg = format!("{}{}",PEER_MESSAGE ,msg);
                peer.send(msg.as_str()).await;
            },
            _=> eprintln!("No peer {}", peer_name)
        }
    }

    fn get_peer_by_name(&self, name:&str)->Option<&Peer>{
        for peer in &self.peers {
            if peer.name == String::from(name) {
                return Some(peer)
            }
        }
        return None
    }

    async fn send_properties(&self, peer:&Peer){
        let str = properties_to_sock_str(&self.properties);
        peer.send(str.as_str()).await;
    }

    /*
    Currently this only responds to new connected peers with the peer name
    TODO we could send other init options here such as a property filter etc
     */
    async fn send_header(&self, peer:&Peer){
        let mut str = HEADER.to_string();
        str.push_str(format!("NAME={}",self.name).as_ref());
        peer.send(str.as_ref()).await;
    }

    async fn set_headers_from_peer(&mut self, head:&str, from:&str){
        for p in self.peers.as_mut_slice() {
            if p.address() == from {
                let vec: Vec<&str> = head.split("NAME=").collect();
                let name = vec[1];
                p.set_name(name);
            }
        }
        self.emit_peers().await;
    }

    async fn broadcast(&self, msg: Option<String>, except:&str){
        /*
        Dont broadcast to the same Peer that the original change came from
        to prevent an infinite re-broadcast loop
         */
        match msg {
            Some(m) => {
                for p in &self.peers {
                    if p.address() != except {
                        p.send(m.as_str()).await;
                    }
                }
            },
            _ => {}
        }

    }

    async fn got_message(&mut self, from:&str, msg:String){
        println!("GOT MESSAGE: {}: {:?}", self.name, msg);
        if msg.len() <3{
            println!("<<< Whats this? {}", msg);
        }else {
            let (msg_type, message) = msg.split_at(3);
            match msg_type {
                PROPERTIES => {
                    let value: toml::Value = toml::from_str(message).unwrap();
                    self.parse_properties(&value);
                },
                PROPERTY => {
                    let p_toml: toml::Value = toml::from_str(message).unwrap();
                    let property = Property::from_table(p_toml.as_table().unwrap());
                    // let broadcast_message = property_to_sock_str(property.as_ref(), true);
                    self.set_property(property.unwrap());

                    self.broadcast(Some(msg), from).await;
                },
                DELETE => {
                    let p = self.properties.remove(&message.to_owned().clone());
                    // this is unnecessary, but fun
                    drop(p);
                    self.broadcast(Some(msg), from).await;
                },
                HEADER => {
                    self.set_headers_from_peer(msg.as_ref(), from).await;
                },
                PEER_MESSAGE => {
                    //|s|127.0.0.1:27733|=|message
                    let c = message.split(PEER_MESSAGE_DIV);
                    let vec: Vec<&str> = c.collect();
                    let pear_name = vec[0];

                    // TODO in future scenario where a hive can be a server and client
                    //  is this need to be considered?
                    if self.is_sever() {
                        self.send_to_peer(vec[1], pear_name).await;
                    }

                    if self.name.to_string() == pear_name.to_string() {
                        self.message_received.emit(String::from(vec[1])).await;
                    }
                },
                REQUEST_PEERS => {
                    let peer_str = format!("{}{}", REQUEST_PEERS, self.peer_string());
                    for p in self.peers.as_mut_slice() {
                        if p.address() == from {
                            p.send(&peer_str).await;
                            p.update_peers = true;
                        }
                    }
                }
                _ => println!("got unknown message {:?},{:?}", msg_type, msg)
            }
        }
    }
    // fn get_peer_by_address(&self, addr:&str)->Option<&Peer>{
    //     for peer in &self.peers {
    //         if peer.address() == String::from(addr) {
    //             return Some(peer)
    //         }
    //     }
    //     return None
    // }
}

