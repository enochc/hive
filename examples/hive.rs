use std::thread::sleep;
use hive::hive::Hive;
use std::time::Duration;

fn main() {
    let mut h = Hive::new("examples/properties.toml");//Hive::parse_properties(&config);

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

            println!("Done: {:?}", p.to_string());
        },
        _ => {
            println!("No OPtion: \"thingvalue\"");
        }
    }

}
