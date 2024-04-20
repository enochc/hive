use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicBool, AtomicI8, Ordering};
use std::thread;
use std::time::Duration;
use async_std::task;
use futures::executor::block_on;
use log::debug;
use slint::private_unstable_api::re_exports::Size;
use hive::backoff::BackOff;
use hive::hive::Hive;





fn main() {
    block_on(done2());
    // block_on(doit());
}

async fn done2(){
    let b = BackOff::new(Duration::from_millis(1_000),||{
        println!("<<< **** 1");
    }).await;
    // task::sleep(Duration::from_millis(100)).await;
    b.update(||{
        println!("<<< **** 2");
    }).await;
    let www = b.update(||{
        println!("<<< **** 2");
    }).await;
    let (x,y) = &*www;
    let some = x.lock().unwrap();
    let ss = y.wait(some).unwrap();
    println!("<<< done that!! {}", *ss);
}
async fn done() {
    let b1 = Arc::new(Mutex::new(true));
    let b2 = b1.clone();
    let b3  = b2.clone();

    println!("1 {}", b1.lock().expect("1"));
    println!("2 {}", b2.lock().expect("2"));
    println!("3 {}", b3.lock().expect("3"));

    // this Locks
    // let mut c = b1.lock().unwrap();
    // *c = false;

    // This doesn't update the mutexes
    // let mut c = *b1.lock().unwrap();
    // c = false;

    // this works!!!
    *b1.lock().unwrap() = false;


    println!("1 {}", b1.lock().expect("1"));
    println!("2 {}", b2.lock().expect("2"));
    println!("3 {}", b3.lock().expect("3"));



}
async fn doit(){
    let ss = Arc::new((Mutex::new("yes"), Condvar::new()));
    let is_setup = Arc::new((Mutex::new(false), Condvar::new()));

    let mut counter = Arc::new(AtomicI8::new(0));

    for x in 0..5 {
        let sc = Arc::clone(&ss);
        let mut cc = Arc::clone(&counter);
        let mut is_setup2 = Arc::clone(&is_setup);
        let xc = x.clone();
        thread::spawn(move || {
            loop {
                println!("spawn {}", xc);
                let (loc, cvar) = &*sc;

                cc.fetch_add(1, Ordering::Acquire);
                if(cc.load(Ordering::Relaxed)==4) {
                    let (x, y) = &*is_setup2;
                    y.notify_one();
                }

                let things = loc.lock().unwrap();

                let gggg = cvar.wait(things).unwrap();
                println!("<<< {}:: {}", x, *gggg);

            }
        });
        println!("spawned {}", x);
    }
    let (x, y) = &*is_setup;
    let guardd = y.wait(x.lock().unwrap()).expect("nopeers");


    let (lock, cvar) = &*ss;
    cvar.notify_all();

    let mut started = lock.lock().unwrap();

    let s = counter.load(Ordering::Relaxed);
    println!("<<< counted:: {}",s);
    *&counter.load( Ordering::Acquire);
    *started = "no";

    task::sleep(Duration::from_millis(1_000)).await;
    cvar.notify_all();
    counter.store(0, Ordering::Relaxed);
    println!("<<< counted:: {}",counter.load(Ordering::Relaxed));
}