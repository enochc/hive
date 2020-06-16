use std::thread::sleep;

use failure::_core::time::Duration;

use hive::property::Property;

fn main() {

    let mut p = Property::from_int("test",4);
    let mut z = 0;

    p.on_changed.connect(|v|{
        println!("Inside signal: {:?}", v);
        sleep(Duration::from_secs(2));
        z+=1;
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
