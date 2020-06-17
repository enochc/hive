
use futures::channel::mpsc::UnboundedSender;
use crate::peer::{SocketEvent};
use crate::property::Property;
use crate::hive::PROPERTY;
use futures::SinkExt;
use futures::executor::block_on;


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

        let mut message = PROPERTY.to_string();
        message.push_str(property.to_string().as_str());
        let socket_event = SocketEvent::Message {
            from: String::from(""),
            msg: message.to_string()
        };
        self.sender.send(socket_event).await.expect("Failed to send property");
    }

    pub fn hangup(&mut self) {
        block_on(self.sender.send(SocketEvent::Hangup{from:String::from("")}))
            .expect("failed to hangup");
    }

}

pub(crate) fn property_to_sock_str(property:Option<&Property>) -> Option<String> {
    match property {
        Some(p) =>{
            let mut message = PROPERTY.to_string();
            message.push_str(p.to_string().as_str());
            return Some(message);
        },
        _ => None
    }

}