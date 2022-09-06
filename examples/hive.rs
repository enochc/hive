use std::thread::sleep;
use hive::hive::Hive;
use std::time::Duration;
use hive::property::{PropertyValue, SetProperty};

//OBSOLETE!!

fn main() {
    let mut h = Hive::new("examples/properties.toml");//Hive::parse_properties(&config);

    // the set functions require a mutable reference to the property
    let p = h.get_mut_property("thingvalue");
    match p {
        Some(p) => {
            // p.on_changed.connect(|v|{
            p.on_next(|v|{
                println!("Inside signal: {:?}", v);
                sleep(Duration::from_millis(300))
            });

            p.set_value(&"What".into());
            p.set("now");
            p.set_value(&6.into());
            p.set(&PropertyValue::from(6));
            p.set(7);
            p.set(true);

            println!("Done: {:?}", p.to_string());
        },
        _ => {
            println!("No Option: \"thingvalue\"");
        }
    }

}
