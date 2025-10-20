use std::rc::{Rc};
use std::sync::{Arc, Weak, Mutex};
use log::warn;
use hive::hive::Hive;
#[cfg(feature = "gui")]
use hive::hive_gui::{HiveWindow, Gui};
use hive::property::Property;

#[cfg(feature = "gui")]
slint::include_modules!();

fn main(){
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


    h.get_mut_property(&Property::hash_id("all")).expect("oops").on_next(move |v| {
        match weak_m1.upgrade() {
            None => {println!("none")}
            Some(w) => {
                (*w.lock().unwrap()).set_value(v);
            }
        }
    });

    #[cfg(feature = "gui")]
    HiveWindow::launch(Some(h));
}
