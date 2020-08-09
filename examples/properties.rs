
use failure::_core::time::Duration;

use hive::property::Property;
use async_std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;


fn main() {

    let mut p = Property::from_int("test",4);
    let counter = Arc::new(AtomicUsize::new(0));
    let c1 = Arc::clone(&counter);
    let c2 = Arc::clone(&counter);

    p.on_changed.connect(move |v|{
        print!("Inside signal: {:?}", v);
        //sleep(Duration::from_millis(1000));
        c1.fetch_add(1, Ordering::SeqCst);
        println!(" DONE")

    });

    p.on_changed.connect(move |v|{
        print!("also Inside signal: {:?}", v);
        c2.fetch_add(1, Ordering::SeqCst);
        println!(" DONE")
    });

    p.set_str("What");
    p.set_str("now");
    p.set_int(6);
    p.set_int(6);
    p.set_bool(true);

    let ret = counter.load(Ordering::Relaxed);
    sleep(Duration::from_millis(1000));
    assert_eq!(ret, 8);
    println!("Done: {:?} ran {:?} times", p.get(), ret);

}
