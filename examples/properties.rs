
use hive::property::Property;
use async_std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::Duration;
use hive::property::SetProperty;
use futures::StreamExt;
use async_std;

fn main() {
    //test
    let mut p = Property::from_value("test",4.into());
    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::clone(&counter);

    p.on_next(move |v| {
        println!("Inside changed signal: {:?}", v);
        c2.fetch_add(1, Ordering::SeqCst);
    });

    p.set("What");
    p.set("now");
    p.set_value(6.into());
    p.set_value(6.into());
    println!("VAL:: {:?}", p.get_value());
    p.set_value(true.into());
    println!("VAL:: {:?}", p.get_value());

    let ret = counter.load(Ordering::Relaxed);
    assert_eq!(ret, 4);
    println!("Done: {:?} ran {:?} times", p.to_string(), ret);

}
