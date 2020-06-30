
use futures::channel::mpsc::UnboundedSender;
use crate::peer::{SocketEvent};
use crate::property::Property;
use crate::hive::{PROPERTY, PROPERTIES};
use futures::SinkExt;
use futures::executor::block_on;
use std::collections::HashMap;


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

pub(crate) fn properties_to_sock_str(properties: &HashMap<String, Property>) -> String {
    let mut message = PROPERTIES.to_string();
    for p in properties {
        message.push_str(
            property_to_sock_str(Some(p.1), false).unwrap().as_str()
        );
        message.push('\n');
    }
    return String::from(message);
}

pub(crate) fn property_to_sock_str(property:Option<&Property>, inc_head:bool) -> Option<String> {
    match property {
        Some(p) =>{
            let mut message = if inc_head{
                PROPERTY.to_string() } else{String::from("")
            };
            message.push_str(p.to_string().as_str());
            return Some(message);
        },
        _ => None
    }

}