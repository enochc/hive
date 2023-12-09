use hive::hive::Hive;

fn main() {
    let mut h = Hive::new("examples/properties.toml");//Hive::parse_properties(&config);

    // the set functions require a mutable reference to the property
    let p = h.get_mut_property("thingvalue");
    match p {
        Some(p) => {

            p.on_next(|v|{
                println!("also Inside signal: {:?}", v);
            });

            p.set_value("What".into());
            p.set_value("now".into());
            p.set_value(6.into());
            p.set_value(6.into());
            p.set_value(true.into());

            println!("Done: {:?}", p.to_string());
        },
        _ => {
            println!("No Option: \"thingvalue\"");
        }
    }

}
