
use std::{env, thread};
use std::ops::Index;
use slint;
use slint::{CloseRequestResponse, Model, ModelRc, VecModel, Weak, SharedString};
use crate::hive::Hive;
use crate::property::{Property, PropertyValue};
use std::rc::Rc;
use std::sync::{Arc, mpsc, RwLock, RwLockReadGuard};
use std::thread::sleep;
use std::time::Duration;
use async_std::sync::Mutex;
use toml::macros::IntoDeserializer;
use toml::Value;
use futures::channel::mpsc::{ channel, Sender};
use futures::{SinkExt, StreamExt};
// use mpsc::{channel, Sender};

pub trait Gui{
    fn launch( hive: Option<Hive>);
}
pub struct HiveWindow{}

impl HProperty {
    fn new(name:&str, val:&str)-> HProperty {
        return HProperty{name:name.into(), value:val.into()}
    }
}

slint::include_modules!();

impl From<&Property> for HProperty {
    fn from(value: &Property) -> Self {
        HProperty{
            name:value.get_name().into(),
            value: match value.get_value() {
                None => {"empty".into()}
                Some(v) => {v.to_string().into()}
            }}
    }
}

impl Gui for HiveWindow {
    fn launch(hive: Option<Hive>) {


        let (mut sender, mut receiver) = channel::<HProperty>(1);
        let window = MainWindow::new().unwrap();
        let window_weak = window.as_weak();


        let props = match hive {
            None => {
                vec![HProperty{name:"two".into(),value:"vals".into()}]
            }
            Some(h) => {
                h.properties.iter().map(|(p,v)|{
                    v.into()
                }).collect()

            }
        };
        let props_model: Rc<VecModel<HProperty>> = Rc::new(VecModel::from(props));
        let properties_rc = ModelRc::from(props_model);

        window.set_properties(properties_rc);
        let mut sender2 = sender.clone();

        async_std::task::spawn(async move {

            loop {
                while (!sender2.is_closed()) {
                    match receiver.next().await {
                        None => {}
                        Some(property) => {
                            println!("<<< property {:?}", property);

                            window_weak.upgrade_in_event_loop(|win|{
                                let props_model: Rc<VecModel<HProperty>> = Rc::new(VecModel::from(vec![property]));
                                    let properties_rc: ModelRc<HProperty> = ModelRc::from(props_model);
                                    win.set_properties(properties_rc)
                            }).unwrap()

                            // slint::invoke_from_event_loop(move || {
                            //     let props_model: Rc<VecModel<HProperty>> = Rc::new(VecModel::from(vec![property]));
                            //     let properties_rc: ModelRc<HProperty> = ModelRc::from(props_model);
                            //     handle_copy.unwrap().set_properties(properties_rc)
                            // }).expect("TODO: panic message");
                        }
                    }
                }

            }
        });

        window.window().on_close_requested(move||{
                println!("<< done");
                return CloseRequestResponse::HideWindow;
        });

        async_std::task::spawn(async move {
            let mut x = 0;
            while(x < 5) {
                async_std::task::sleep(Duration::from_secs(3)).await;
                sender.send(HProperty::new("one", &format!("two {}", x))).await.expect("TODO: panic message");
                println!("<<<<<<{}",x);
                x+=1;
            }

        });

        window.run().unwrap();
    }
}


slint::slint! {

}