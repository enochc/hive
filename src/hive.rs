// Much code "borrowed" from> https://book.async.rs/tutorial/implementing_a_client.html
use std::{collections::HashMap, fs};
use crate::signal::Signal;
use async_std::{
    net::{TcpListener, TcpStream, ToSocketAddrs},
    task,
    sync::Arc,
};
use futures::{SinkExt, StreamExt};
use futures::channel::mpsc;
use toml;

use crate::handler::{Handler, };
use crate::peer::{Peer, SocketEvent};
use crate::property::{Property, properties_to_sock_str};
// use usbd_serial::{SerialPort, USB_CLASS_CDC, UsbError};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;

use log::{debug, info, error};
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};



// #[derive(Debug)]
pub struct Hive {
    pub properties: HashMap<String, Property>,
    sender: Sender<SocketEvent>,
    receiver: Receiver<SocketEvent>,
    connect_to: Option<Box<str>>,
    listen_port: Option<Box<str>>,
    pub name: Box<str>,
    peers: Vec<Peer>,
    pub message_received: Signal<String>,
    pub connected: Arc<AtomicBool>,
    listening: Arc<AtomicBool>,
}


pub (crate) const PROPERTIES: &str = "|P|";
pub (crate) const PROPERTY: &str = "|p|";
pub (crate) const DELETE: &str = "|d|";
pub (crate) const PEER_MESSAGE: &str = "|s|";
pub (crate) const HEADER: &str = "|H|";
pub (crate) const PEER_MESSAGE_DIV: &str = "|=|";
pub (crate) const REQUEST_PEERS: &str = "<p|";
// pub (crate) const ACK:&str = "<<|";

#[cfg(feature="bluetooth")]
fn spawn_bluetooth_listener(listening:Arc<AtomicBool>)->Result<()>{
    std::thread::spawn(move||{
        let mut rt = tokio::runtime::Builder::new()
            .threaded_scheduler()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move{
            let perf = crate::bluetooth::advertise::Peripheral::new().await;
            perf.run(listening).await.expect("Failed to run peripheral");
        });
    });


    Ok(())

}

impl Hive {
    pub fn is_sever(&self) ->bool{
        self.listen_port.is_some()
    }

    pub fn is_connected(&self)->bool{
        return self.connected.load(Ordering::Relaxed);
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
            connected: Arc::new(AtomicBool::new(false)),
            listening: Arc::new(AtomicBool::new(false)),
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
            info!("Accepting from: {}", stream.peer_addr()?);
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

    // pub async fn connect_USB(){
    //     init_logging();
    //
    //     let usb_bus = device_specific_usb::UsbBus::new(...);
    //     let mut serial = SerialPort::new(&usb_bus);
    //
    //     let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x16c0, 0x27dd))
    //         .product("Serial port")
    //         .device_class(USB_CLASS_CDC)
    //         .build();
    //
    //     loop {
    //         if !usb_dev.poll(&mut [&mut serial]) {
    //             continue;
    //         }
    //
    //         let mut buf = [0u8; 64];
    //
    //         match serial.read(&mut buf[..]) {
    //             Ok(count) => {
    //                 // count bytes were read to &buf[..count]
    //                 prin
    //             },
    //             Err(UsbError::WouldBlock) => {
    //                 // Err(err) => // An error occurred
    //             },// No data received
    //             _ => {error!("Something else happened!");}
    //
    //         };
    //
    //         match serial.write(&[0x3a, 0x29]) {
    //             Ok(count) => {
    //                 // count bytes were written
    //             },
    //             Err(err) => {
    //                 error!("Usb error occurred");
    //             }// No data could be written (buffers full)
    //                 // Err(e) => // An error occurred
    //         };
    //     }
    // }


    pub fn get_listener(&self) ->Arc<AtomicBool>{
        return self.listening.clone();
    }

    // tokio required by bluetooth bluster libs


    pub async fn run(& mut self){//} -> Result<()> {
        self.listening.store(true, Ordering::Relaxed);
        // I'm a client
        if !self.connect_to.is_none() {
            info!("Connect To: {:?}", self.connect_to);
            let address = self.connect_to.as_ref().unwrap().to_string().clone();
            let send_chan = self.sender.clone();
            let (tx, mut rx) = mpsc::unbounded();
            let mut connected = false;
            /* loop indefinately to connect to remote server */
            while !connected{
                let addr = address.clone();
                let send_chan_clone = send_chan.clone();
                let mut tx_clone = tx.clone();
                task::spawn(async move{
                    let stream = TcpStream::connect(&addr).await;
                    match stream {
                        Ok( s) => {
                            match s.peer_addr() {
                                Ok(_addr) => {
                                    let se = SocketEvent::NewPeer {
                                        name:String::from(""),
                                        stream: s,
                                    };
                                    send_chan_clone.clone().send(se).await.expect("failed to send peer");
                                    tx_clone.send(true).await.expect("Failed to send connected signal");
                                },
                                Err(e) => error!("No peer address: {:?}", e),
                            }
                        },
                        Err(e) => {
                            error!("Nope:: {:?}",e);
                            tx_clone.send(false).await.expect("Failed to send connect failed signal");

                        }
                    };

                });
                // listen for messages from server
                connected = rx.next().await.unwrap();
                if connected {
                    self.connected.store(true, Ordering::Relaxed);
                    self.receive_events(false).await;
                } else {
                    debug!("failed to connect, retry in a moment");
                    task::sleep(Duration::from_secs(10)).await;
                }
            }

            debug!("CLIENT DONE");
        }

        // I'm a server
        if !self.listen_port.is_none() {
            let port = self.listen_port.as_ref().unwrap().to_string().clone();
            info!("{:?} Listening for connections on {:?}",self.name, self.listen_port);
            let send_chan = self.sender.clone();
            // listen for connections loop
            let p = port.clone();
            task::spawn( async move {
                match Hive::accept_loop(send_chan.clone(), p).await {
                    Err(e) => error!("Failed accept loop: {:?}",e),
                    _ => (),
                }
            });

            #[cfg(feature="bluetooth")]
                {

                    info!("!! this is bluetooth");
                    let listening = self.listening.clone();


                    self.connected.store(true, Ordering::Relaxed);
                    // async_std::task::spawn(async move{
                    //
                    // });
                    // let mut rt = tokio::runtime::Runtime::new().unwrap();


                    spawn_bluetooth_listener(listening).expect("Failed to spawn bluetooth");

                    self.receive_events(true).await;

                }
            #[cfg(not(feature="bluetooth"))]
                {
                    self.connected.store(true, Ordering::Relaxed);
                    self.receive_events(true).await;
                }

            debug!("SERVER DONE");
        }
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
                    wait for the peer response with it's header info so we
                    have the actual peer name that it's given us to response with
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
                            self.notify_peers_change().await;
                            break;
                        }
                    }
                }
            }
        }
    }

    fn peer_string(& self)->String {
        let mut peers_string = String::new();
        debug!("name {:?}", &self.name);

        // Add self to peers list
        let myadr = self.listen_port.as_ref().expect("No port").clone();
        let myname = String::from(self.name.as_ref());
        peers_string.push_str(&format!("{}|{}", myname, myadr));

        for x in 0..self.peers.len(){
            peers_string.push_str(",");
            let p = &self.peers[x];
            let adr = p.address();
            let name = p.name.clone();
            peers_string.push_str(&format!("{}|{}", name, adr))
        };
        return peers_string;
    }

    async fn notify_peers_change(&mut self){
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
            _=> error!("No peer {}", peer_name)
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

        if self.is_sever() {
            self.notify_peers_change().await;
        }

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
        debug!("GOT MESSAGE: {}: {:?}", self.name, msg);
        if msg.len() <3{
            debug!("<<< Whats this? {}", msg);
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
                    let c = message.split(PEER_MESSAGE_DIV);
                    let vec: Vec<&str> = c.collect();
                    if vec.len() == 1 {
                        // the PEER_MESSAGE_DIV separates the peer name from the message
                        // without the DIV, there is only a message and its for me
                        debug!("the message was forwarded for me: {:?}", message);
                        self.message_received.emit(String::from(message)).await;
                        return;
                    }
                    let pear_name = vec[0];
                    if self.name.to_string() == pear_name.to_string() {
                        debug!("Message is for me: {:?}", vec[1]);
                        self.message_received.emit(String::from(vec[1])).await;
                    } else {
                        self.send_to_peer(vec[1], pear_name).await;
                    }
                },
                REQUEST_PEERS => {
                    let peer_str = format!("{}{}", REQUEST_PEERS, self.peer_string());
                    for p in self.peers.as_mut_slice() {
                        if p.address() == from {
                            p.send(&peer_str).await;
                            p.update_peers = true;
                            break;
                        }
                    }
                }
                _ => debug!("got unknown message {:?},{:?}", msg_type, msg)
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

