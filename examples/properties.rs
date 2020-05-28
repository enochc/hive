use std::thread::sleep;

use failure::_core::time::Duration;

use hive::models::Property;

fn main() {

    let mut p = Property::from_int(4);

    p.on_changed.connect(|v|{
        println!("Inside signal: {:?}", v);
        sleep(Duration::from_secs(2))
    });

    p.on_changed.connect(|v|{
        println!("also Inside signal: {:?}", v);
        // sleep(Duration::from_secs(2))
    });

    p.set_str("What");
    p.set_str("now");
    p.set_int(6);
    p.set_int(6);
    p.set_bool(true);

    println!("Done: {:?}", p.get());
}
