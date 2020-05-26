use std::thread::sleep;
use failure::_core::time::Duration;
use std::fs;
use hive::Hive;

fn main() {

    let foo: String = fs::read_to_string("examples/properties.toml").unwrap().parse().unwrap();
    let config: toml::Value = toml::from_str(&foo).unwrap();

    let mut h = Hive::parse_properties(&config);

    // the set functions require a mutable reference to the property
    let p = h.get_mut_property("thingvalue");
    match p {
        Some(p) => {
            p.on_changed.connect(|v|{
                println!("Inside signal: {:?}", v);
                sleep(Duration::from_secs(2))
            });

            p.on_changed.connect(|v|{
                println!("also Inside signal: {:?}", v);
            });

            p.set_str("What");
            p.set_str("now");
            p.set_int(6);
            p.set_int(6);
            p.set_bool(true);

            println!("Done: {:?}", p.get());
        },
        _ => {
            println!("No OPtion: \"thingvalue\"");
        }
    }






}
