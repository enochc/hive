use std::rc::{Rc};
use std::sync::{Arc, Weak, Mutex};
use log::{debug, LevelFilter};
use log::warn;
use hive::hive::Hive;
#[cfg(feature = "gui")]
use hive::hive_gui::{HiveWindow, Gui};
use hive::init_logging;
use hive::property::Property;

#[cfg(feature = "gui")]
slint::include_modules!();

#[tokio::main]
async fn main(){
    init_logging(Some(LevelFilter::Debug));
    if !cfg!(feature = "gui") {
        warn!("Feature \"GUI\" is not enabled.")
    }
    let mut h = Hive::new("examples/motors.toml");
    h.get_mut_property(&Property::hash_id("m1")).expect("oops").on_next(|v|{
        println!("M1:: {}", v);
    });

    // let property = h.get_mut_property(&Property::hash_id("m1")).expect("sasdf");
    // let mp = Arc::new(Mutex::new(property.clone()));
    // let weak_m1 = Arc::downgrade(&mp).clone();
    let weak_m1 = h.get_weak_property("m1").clone();


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

    #[cfg(feature = "gui")]
    HiveWindow::launch(Some(h));


}
