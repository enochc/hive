use std::collections::HashMap;
use slint;
use slint::{CloseRequestResponse, Model, ModelRc, SharedString, VecModel, Weak};
use std::env;
use std::ops::Index;

use crate::hive::Hive;
use crate::property::{Property, SetProperty};

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
    fn launch(hive: Option<Hive>) {
        let (mut sender, mut receiver) = bounded(1); //channel::<Vec<HProperty>>();
        let window = MainWindow::new().unwrap();
        let window_weak = window.as_weak();

        let mut hive_props: Option<HashMap<u64, Property>> = None;

        let mut props = match hive {
            None => {
                vec![HProperty {
                    name: "two".into(),
                    value: "vals".into(),
                }]
            }
            Some(ref h) => {
                hive_props = Some(h.properties.clone());
                h.properties.iter().map(|(p, v)| v.into()).collect()
            },
        };
        props.sort_by(|a, b| a.name.cmp(&b.name));
        let props_model: Rc<VecModel<HProperty>> = Rc::new(VecModel::from(props));
        let properties_rc = ModelRc::from(props_model);

        window.set_properties(properties_rc);
        // let mut sender2 = sender.clone();
        let mut done:bool = false;

        async_std::task::spawn(async move {
            loop {
                while !done {
                    match receiver.recv() {//next().await {
                        // None => {}
                        // Some(property) => {
                        //     println!("<<< property {:?}", property);
                        //
                        //     window_weak
                        //         .upgrade_in_event_loop(|win| {
                        //             let props_model: Rc<VecModel<HProperty>> =
                        //                 Rc::new(VecModel::from(property));
                        //             let properties_rc: ModelRc<HProperty> =
                        //                 ModelRc::from(props_model);
                        //             win.set_properties(properties_rc)
                        //         })
                        //         .expect("Window is gone.");
                        //
                        //     // slint::invoke_from_event_loop(move || {
                        //     //     let props_model: Rc<VecModel<HProperty>> = Rc::new(VecModel::from(vec![property]));
                        //     //     let properties_rc: ModelRc<HProperty> = ModelRc::from(props_model);
                        //     //     handle_copy.unwrap().set_properties(properties_rc)
                        //     // }).expect("TODO: panic message");
                        // }
                        Ok(property) => {
                            println!("<<< property {:?}", property);
                                window_weak
                                    .upgrade_in_event_loop(|win| {
                                        let props_model: Rc<VecModel<HProperty>> =
                                            Rc::new(VecModel::from(property));
                                        let properties_rc: ModelRc<HProperty> =
                                            ModelRc::from(props_model);
                                        win.set_properties(properties_rc)
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

        let mut ssc: Sender<Vec<HProperty>> = sender.clone();

        window.on_prop_changed( move |prop:SharedString, val:f32|{
            match rr.as_ref() {
                None => {}
                Some(r) => {
                    // if prop.eq("all") {
                    //     println!("all");
                    //     ssc.send(vec! {
                    //         HProperty::new("all",&val.to_string()),
                    //         HProperty::new("m1",&val.to_string()),
                    //         HProperty::new("m2",&val.to_string()),
                    //         HProperty::new("m3",&val.to_string()),
                    //         HProperty::new("m4",&val.to_string()),
                    //     }).expect("TODO: panic message");
                    // } else {
                        let mut property = r.properties[&Property::hash_id(&prop)].clone();
                        property.set_value(val.into());
                    // }
                }
            }
        });

        window.run().unwrap();
    }
}
