use std::collections::HashMap;
use slint;
use slint::{CloseRequestResponse, Model, ModelRc, SharedString, VecModel};
use std::env;
use std::ops::Index;

use crate::hive::Hive;
use crate::property::{Property};
use std::sync::{Arc, mpsc, RwLock, RwLockReadGuard};
use futures::{SinkExt, StreamExt};
use std::rc::Rc;
use std::time::Duration;
use log::kv::{Source, Value};
use futures::channel::mpsc::{channel, Sender as ASender};

use toml::macros::IntoDeserializer;
use crossbeam_channel::{bounded, RecvError, select, Sender};
use async_std::task::block_on;
use log::debug;
use crate::handler::Handler;

pub trait Gui {
    fn launch(hive: Option<Hive>);
    // fn launch_with<F>(hive: Option<Hive>, with_window:F) where F: Fn(HiveWindow);
}

pub struct HiveWindow {}

impl HProperty {
    fn new(name: &str, val: &str) -> HProperty {
        return HProperty {
            name: name.into(),
            value: val.into(),
        };
    }
}

slint::include_modules!();

impl From<&Property> for HProperty {
    fn from(value: &Property) -> Self {
        HProperty {
            name: value.get_name().into(),
            value: match value.get_value() {
                None => "__empty".into(),
                Some(v) => v.to_string().into(),
            },
        }
    }
}

impl Gui for HiveWindow {
    fn launch(mut hive: Option<Hive>) {
        let (mut sender, mut receiver) = channel::<HProperty>(1);
        let window = MainWindow::new().unwrap();
        let window_weak = window.as_weak();

        let rc_hive = Rc::new(hive);

        // let prop = match &*rc_hive {
        //     None => {
        //         None
        //     }
        //     Some(mut h) => {
        //         let p = h.get_mut_property_by_name("lightValue");
        //         return *p
        //     }
        // };

        let props = match rc_hive.as_ref() { //match hive {
            None => {
                vec![HProperty { name: "two".into(), value: "vals".into() }]
            }
            Some(h) => {
                // h.get_mut_property_by_name("lightValue");
                h.properties.iter().map(|(p, v)| {
                    let mut pp = v.clone();
                    pp.set_backoff(&1000);
                    (&pp).into()
                    // v.into()
                    // pp
                }).collect()
            }
        };

        let props_model: Rc<VecModel<HProperty>> = Rc::new(VecModel::from(props));
        let properties_rc = ModelRc::from(props_model);

        window.set_properties(properties_rc);
        let mut sender2 = sender.clone();

        tokio::task::spawn(async move {
            loop {
                while (!sender2.is_closed()) {
                    match receiver.next().await {
                        None => {}
                        Some(property) => {
                            println!("<<< property {:?}", property);

                            let handle_copy = window_weak.clone();
                            slint::invoke_from_event_loop(move || {
                                let props_model: Rc<VecModel<HProperty>> = Rc::new(VecModel::from(vec![property]));
                                let properties_rc: ModelRc<HProperty> = ModelRc::from(props_model);
                                handle_copy.unwrap().set_properties(properties_rc)
                            }).expect("TODO: panic message");
                        }
                    }
                }
            }
        });

        window.window().on_close_requested(move || {
            println!("<< done");
            return CloseRequestResponse::HideWindow;
        });

        let r2 = rc_hive.clone();

        window.on_prop_changed(move |prop: SharedString, val: f32| {
            debug!("<<< prop changed: {:?}, {}", prop, val);
            match &*r2  {
                None => {}
                Some(r) => {
                    let mut property = r.properties[&Property::hash_id(&prop)].clone();
                    property.set_value(val.into());
                }
            }
        });

        window.run().unwrap();
        println!("<< WTF>>>");
    }
}
