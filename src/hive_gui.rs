use std::collections::HashMap;
use slint;
use slint::{CloseRequestResponse, Model, ModelRc, SharedString, VecModel};
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

impl HProperty {
    fn new(name: &str, val: &str) -> HProperty {
        return HProperty {
            name: name.into(),
            value: val.into(),
        };
    }
}

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

        let mut hive_props: Option<HashMap<u64, Property>> = None;

        let mut props = match hive {
            None => {
                vec![0f32, 0f32]
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




        window.run().unwrap();
    }
}
