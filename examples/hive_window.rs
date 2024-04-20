use std::rc::{Rc};
use std::sync::{Arc, Weak, Mutex};
use std::thread;
use log::LevelFilter;
use hive::hive::Hive;
use hive::hive_gui::{HiveWindow, Gui};
use hive::init_logging;
use hive::property::Property;
slint::include_modules!();

fn main(){

    // let h1 = thread::spawn(||{
    //     let mut h = Hive::new("examples/listen_3000.toml");
    //     HiveWindow::launch(Some(h));
    // });

    // let h2 = Hive::new("examples/connect_3000.toml");
    // HiveWindow::launch(Some(h2));

    init_logging(Some(LevelFilter::Debug));

//     let motors_toml = r#"
// connect = "127.0.0.1:3000"
// name = "client"
//     "#;

    let mut h = Hive::new("examples/listen_3000.toml");
    // let mut h = Hive::new_from_str(motors_toml);

    // h.get_mut_property(&Property::hash_id("m1")).expect("oops").on_next(|v|{
    //     println!("M1:: {}", v);
    // });



    // let property = h.get_mut_property(&Property::hash_id("m1")).expect("sasdf");
    // let mp = Arc::new(Mutex::new(property.clone()));
    // let weak_m1 = Arc::downgrade(&mp).clone();

    // let weak_m1 = h.get_weak_property("m1").clone();
    //
    //
    // h.get_mut_property(&Property::hash_id("all")).expect("oops").on_next(move |v| {
    //     match weak_m1.upgrade() {
    //         None => {println!("none")}
    //         Some(w) => {
    //             (*w.lock().unwrap()).set_value(v);
    //         }
    //     }
    // });


    HiveWindow::launch(Some(h));


}
