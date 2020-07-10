use futures::channel::mpsc::UnboundedSender;
use futures::executor::block_on;
use futures::SinkExt;

use crate::hive::{PEER_MESSAGE_DIV};
use crate::peer::SocketEvent;
use crate::property::{Property, property_to_sock_str};

#[derive(Clone)]
pub struct Handler {
    pub (crate) sender: UnboundedSender<SocketEvent>
}

impl Handler {
    pub fn set_str(&self, name:&str, value:&str){
        let _prop = Property::from_str(name, value);
    }

    pub async fn send_property_string(&mut self, prop_name:&str, prop_value:&str){
        let p = Property::from_str(prop_name, prop_value);
        self.send_property(p).await;
    }

    pub async fn send_property(&mut self, property:Property){

        let message = property_to_sock_str(Some(&property), true)
            .unwrap();
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg: message.to_string()
        };

        self.sender.send(socket_event).await.expect("Failed to send property");
    }

    pub async fn delete_property(&mut self, name: &str){
        let p = Property::new(name, None);
        let message = property_to_sock_str(Some(&p), true).unwrap();
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg: message.to_string()
        };
        self.sender.send(socket_event).await.unwrap();
    }

    pub async fn send_to_peer(&mut self, peer_name:&str, msg:&str){
        let message = format!("|s|{}{}{}",peer_name,PEER_MESSAGE_DIV, msg);
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg: message.to_string()
        };
        self.sender.send(socket_event).await.unwrap();
    }

    pub fn hangup(&mut self) {
        block_on(self.sender.send(SocketEvent::Hangup{from:String::from("")}))
            .expect("failed to hangup");
    }

}

