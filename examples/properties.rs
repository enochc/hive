use std::thread::sleep;

use config::Config;
use failure::_core::time::Duration;
use futures::executor::block_on;

use hive;
use hive::models::{Property, PropertyType};
use hive::signal;

fn main() {

    // let h = Block_on(hive::run(&String::from("examples/properties.toml")));

    let mut p = Property::from_int(4);

    p.on_changed.connect(|v|{
        println!("Inside signal: {:?}", v);
        sleep(Duration::from_secs(2))
    });

    p.on_changed.connect(|v|{
        println!("also Inside signal: {:?}", v);
    });

    p.setStr("What");
    p.setStr("now");
    p.setInt(6);
    p.setBool(true);

    println!("Done: {:?}", p.get());
}
