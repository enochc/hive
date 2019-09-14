use hive;
use hive::signal;
use config::Config;
use std::thread::sleep;
use failure::_core::time::Duration;

fn main() {

    let mut config = Config::new();
    config.merge(config::File::with_name("hive.yaml"));

    hive::run(&config);

    let mut p: signal::Property<i32> = Default::default();

    p.on_changed.connect(|v|{
        println!("Inside signal: {:?}", v);
        sleep(Duration::from_secs(2))
    });

    p.on_changed.connect(|v|{
        println!("also Inside signal: {:?}", v);
    });

    p.set(3);
    p.set(4);

    println!("Done: {:?}", p.get());
}