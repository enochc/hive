use std::collections::HashMap;
use slint;
use slint::{CloseRequestResponse, Model, SharedString};
use std::env;
use std::ops::Index;

use crate::hive::Hive;
use crate::property::{Property};

use futures::{SinkExt, StreamExt};
use std::rc::Rc;
use log::kv::{Source, Value};
use toml::macros::IntoDeserializer;
use crossbeam_channel::{bounded, RecvError, select, Sender};
use async_std::task::block_on;

pub trait Gui {
    fn launch(hive: Option<Hive>);
}

pub struct HiveWindow {}

// impl HProperty {
//     fn new(name: &str, val: &str) -> HProperty {
//         return HProperty {
//             name: name.into(),
//             value: val.into(),
//         };
//     }
// }

slint::include_modules!();

// impl From<&Property> for HProperty {
//     fn from(value: &Property) -> Self {
//         HProperty {
//             name: value.get_name().into(),
//             value: match value.get_value() {
//                 None => "__empty".into(),
//                 Some(v) => v.to_string().into(),
//             },
//         }
//     }
// }

impl Gui for HiveWindow {
    fn launch(hive: Option<Hive>) {
        let (mut sender, mut receiver) = bounded::<Vec<f32>>(1); //channel::<Vec<HProperty>>();
        let window = MainWindow::new().unwrap();
        let window_weak = window.as_weak();

        // let mut hive_props: Option<HashMap<u64, Property>> = None;

        // let mut props = match hive {
        //     None => {
        //         vec![0f32, 0f32]
        //     }
        //     Some(ref h) => {
        //         hive_props = Some(h.properties.clone());
        //         h.properties.iter().map(|(p, v)| v.into()).collect()
        //     },
        // };
        //props.sort_by(|a, b| a.name.cmp(&b.name));
        // let props_model: Rc<VecModel<HProperty>> = Rc::new(VecModel::from(props));
        // let properties_rc = ModelRc::from(props_model);

        // window.set_properties(properties_rc);
        let mut done:bool = false;

        async_std::task::spawn(async move {
            loop {
                while !done {
                    match receiver.recv() {
                        Ok(property) => {
                            // println!("<<< property {:?}", property);
                            window_weak
                                .upgrade_in_event_loop(move |win| {
                                    win.set_motor1(property[0]);
                                    win.set_motor2(property[1]);
                                    win.set_motor3(property[2]);
                                    win.set_motor4(property[3]);
                                })
                                .expect("Window is gone.");
                        }
                        Err(_) => {}
                    }
                }
            }
        });

        window.window().on_close_requested(move || {
            println!("<< done");
            done = true;
            return CloseRequestResponse::HideWindow;
        });

        let rr = Rc::new(hive).clone();

        // let mut ssc: Sender<Vec<f32>> = sender.clone();

        window.on_prop_changed( move |prop:SharedString, val:f32|{
            match rr.as_ref() {
                None => {}
                Some(r) => {
                        let mut property = r.properties[&Property::hash_id(&prop)].clone();
                        property.set_value(val.into());
                }
            }
        });

        window.run().unwrap();
    }
}
