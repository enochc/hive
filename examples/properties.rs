use std::thread::sleep;

use config::Config;
use failure::_core::time::Duration;
use futures::executor::block_on;

use hive;
use hive::models::{Property, PropertyType};
use hive::signal;

fn main() {

    // let ran = block_on(hive::run(&config));
    //hive::run(&String::from("examples/properties.toml"));

    // let mut p: Property<i32> = Default::default();

    //  let mut p: Property<PropertyType> = Property{
    //     // value: Some(PropertyType::STRING(String::from("none"))),
    //      value: Some(PropertyType::new("none")),
    //     on_changed: Default::default(),
    // }; //Default::default();
    let mut p = Property::new("none");

    p.on_changed.connect(|v|{
        println!("Inside signal: {:?}", v);
        sleep(Duration::from_secs(2))
    });

    p.on_changed.connect(|v|{
        println!("also Inside signal: {:?}", v);
    });
    let v = PropertyType::STRING(String::from("two").into_boxed_str());


    p.set(v);
    // p.set("its".to_string());

    println!("Done: {:?}", p.get());
}
