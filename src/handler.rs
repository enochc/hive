use futures::channel::mpsc;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use crate::peer::{SocketEvent, Peer};
use crate::property::Property;
use crate::hive::PROPERTY;
use futures::SinkExt;
use futures::executor::block_on;

type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;

#[derive(Clone)]
pub struct Handler {
    pub (crate) sender: Sender<SocketEvent>
}

impl Handler {
    pub fn set_str(&self, name:&str, value:&str){
        let prop = Property::from_str(name, value);


    }
    // pub fn send_property(&mut self, msg:&str) {
    //     // comvert property into a string message for deconstruction
    //     block_on(self.sender.send(SocketEvent::Message {
    //         from: String::from("blah blah blah"),
    //         msg: String::from(msg),
    //     })).expect("failed to send property");
    // }

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
        self.sender.send(socket_event).await;
    }
}