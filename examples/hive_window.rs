use std::rc::{Rc};
use std::sync::{Arc, Weak, Mutex};
use std::thread;
use log::{debug, LevelFilter};
use hive::hive::Hive;
use hive::hive_gui::{HiveWindow, Gui};
use hive::init_logging;
use hive::property::Property;
slint::include_modules!();

fn main(){
    init_logging(Some(LevelFilter::Debug));


    let mut h = Hive::new("examples/listen_3000.toml");

    let p = h.get_mut_property_by_name("lightValue");
    match p {
        None => {}
        Some(prop) => {
            // let p2 = prop.clone();
            prop.on_next(move |v|{
                debug!("<<< CHANGED {:?}", v);
            });
        }
    }


    HiveWindow::launch(Some(h));


}
