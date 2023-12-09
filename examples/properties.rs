
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
    let c1 = Arc::clone(&counter);
    let c2 = Arc::clone(&counter);

    // p.on_changed.connect(move |v|{
    // p.on_next(move |v| {
    let mut stream = p.stream.clone();
    async_std::task::spawn(async move {
        while let Some(z) = stream.next().await {
            println!("Inside signal: {:?}", x);
            sleep(Duration::from_millis(1000));
            c1.fetch_add(1, Ordering::SeqCst);
            println!(" DONE 1")
        };
    });

    // p.on_changed.connect(move |v|{
    // p.on_next(move |v| {
    //     print!("also Inside signal: {:?}", v);
    //     c2.fetch_add(1, Ordering::SeqCst);
    //     println!(" DONE 2")
    // });

    p.set("What");
    p.set("now");
    p.set_value(6.into());
    p.set_value(6.into());
    p.set_value(true.into());
    // wait a moment for the on_change handler to pick up the change
    sleep(Duration::from_millis(300));

    let ret = counter.load(Ordering::Relaxed);
    sleep(Duration::from_millis(1000));
    assert_eq!(ret, 8);
    println!("Done: {:?} ran {:?} times", p.to_string(), ret);

}
