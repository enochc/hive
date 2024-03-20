
use std::env;
use std::ops::Index;
use slint;
use slint::{CloseRequestResponse, Model, ModelRc, VecModel, Weak, SharedString};
use crate::hive::Hive;
use crate::property::{Property, PropertyValue};
use std::rc::Rc;
use std::sync::{Arc, RwLock, RwLockReadGuard};
use std::thread::sleep;
use std::time::Duration;
use async_std::sync::Mutex;
use toml::macros::IntoDeserializer;
use toml::Value;

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
        // let props_model: Rc<VecModel<HProperty>> = Rc::new(VecModel::from(props));
        let props_model = Arc::new(Mutex::new(VecModel::from(props)));
        let properties_rc = ModelRc::from(props_model.into());
        let window = MainWindow::new().unwrap();
        window.set_properties(properties_rc);
        window.window().on_close_requested(move||{
                println!("<< done");
                return CloseRequestResponse::HideWindow;
        });

        let ccc = props_model.clone();

        async_std::task::spawn(async move {
            let mut x = 0;
            while(x < 5) {
                async_std::task::sleep(Duration::from_secs(3)).await;
                let athing = ccc.lock().await;
                athing.push(HProperty::new("another one","something"));
                println!("<<<<<<{}",x);
                x+=1;
            }

        });

        window.run().unwrap();
    }
}


slint::slint! {

}