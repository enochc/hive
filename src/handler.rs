use bytes::{BufMut, BytesMut};
use futures::channel::mpsc::UnboundedSender;
use futures::executor::block_on;
use futures::SinkExt;
#[allow(unused_imports)]
use log::{debug, error, info};

use crate::hive::{PEER_MESSAGE, PEER_MESSAGE_DIV};
use crate::peer::SocketEvent;
use crate::property::{Property, property_to_bytes, PropertyValue};

#[derive(Clone)]
pub struct Handler {
    pub (crate) sender: UnboundedSender<SocketEvent>,
    pub from_name: String,
}


impl Handler {
    pub fn set_str(&self, name:&str, value:&str){
        let _prop = Property::from_str(name, value);
    }

    // This is basically a set_property method, however, Hive itself does not deal directly with property values
    // It only propagates value change notifications, hence the name
    pub async fn set_property(&mut self, prop_name:&str, prop_value:Option<&PropertyValue>){
        // let p = Property::from_toml(prop_name, prop_value);
        let p = Property::from_toml(prop_name, Some(&prop_value.unwrap().toml()));
        self.send_property(p).await;
    }

    pub async fn send_property(&mut self, property:Property){

        let message = property_to_bytes(Some(&property), true)
            .unwrap();
        debug!("... send_property: {:?}, {:?}", message, self.sender.is_closed());
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg: message
        };

        self.sender.send(socket_event).await.expect("Failed to send property");

    }

    pub async fn delete_property(&mut self, name: &str){
        let p = Property::new(name, None);
        let message = property_to_bytes(Some(&p), true).unwrap();
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg: message
        };
        self.sender.send(socket_event).await.unwrap();
    }

    pub async fn send_to_peer(&mut self, peer_name:&str, msg:&str){
        let mut bytes = BytesMut::with_capacity(peer_name.len()+msg.len()+2);
        bytes.put_u8(PEER_MESSAGE);
        bytes.put_slice(peer_name.as_bytes());
        bytes.put_u8(PEER_MESSAGE_DIV.as_bytes()[0]);
        bytes.put_slice(msg.as_bytes());
        debug!(".... send to peer: {:?}", bytes);
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg: bytes.freeze()
        };
        self.sender.send(socket_event).await.expect("failed to send to peer");

    }

    pub fn hangup(&mut self) {
        block_on(self.sender.send(SocketEvent::Hangup{from:String::from("")}))
            .expect("failed to hangup");
    }

}

