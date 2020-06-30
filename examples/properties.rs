use std::thread::sleep;

use failure::_core::time::Duration;

use hive::property::Property;
use async_std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};


fn main() {

    let mut p = Property::from_int("test",4);
    let counter = Arc::new(AtomicUsize::new(0));
    let c1 = Arc::clone(&counter);
    let c2 = Arc::clone(&counter);

    p.on_changed.connect(move |v|{
        println!("Inside signal: {:?}", v);
        sleep(Duration::from_millis(500));
        c1.fetch_add(1, Ordering::SeqCst);

    });

    p.on_changed.connect(move |v|{
        println!("also Inside signal: {:?}", v);
        c2.fetch_add(1, Ordering::SeqCst);
    });

    p.set_str("What");
    p.set_str("now");
    p.set_int(6);
    p.set_int(6);
    p.set_bool(true);

    let ret = counter.load(Ordering::Relaxed);
    assert_eq!(ret, 8);
    println!("Done: {:?} ran {:?} times", p.get(), ret);

}
