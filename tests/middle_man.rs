use async_std::sync::Arc;
use futures::executor::block_on;
use hive::hive::Hive;
use hive::init_logging;
use hive::property::Property;
use log::LevelFilter;
#[allow(unused_imports)]
use log::{debug, info};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

fn panic_after<T, F>(d: Duration, f: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T,
    F: Send + 'static,
{
    let (done_tx, done_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let val = f();
        done_tx.send(()).expect("Unable to send completion signal");
        val
    });

    match done_rx.recv_timeout(d) {
        Ok(_) => handle.join().expect("Thread panicked"),
        Err(_) => panic!("Thread took too long"),
    }
}

#[test]
fn main() {
    panic_after(Duration::from_millis(3_000), || {
        init_logging(Some(LevelFilter::Debug));

        let counter = Arc::new(AtomicUsize::new(0));
        let counter1 = counter.clone();
        let counter2 = counter.clone();

        let ack: Arc<(Mutex<u32>, Condvar)> = Arc::new((Mutex::new(0), Condvar::new()));
        let ack_clone = ack.clone();
        let ack_clone2 = ack.clone();

        let props_str = r#"
    listen="3000"
    name = "Server"
    [Properties]
    something_else_entirely=1
    "#;
        let mut server_hive = Hive::new_from_str(props_str);
        let thing_key = &Property::hash_id("something_else_entirely");

        server_hive
            .get_mut_property(thing_key)
            .unwrap()
            .on_next(move |value| {
                info!(
                    "SERVER ----------------------- server thing changed: {:?}",
                    value
                );
                counter1.fetch_add(1, Ordering::SeqCst);
                let (lock, cvar) = &*ack_clone;
                let mut done = lock.lock().unwrap();
                *done += 1;
                cvar.notify_one();
            });

        server_hive.go(true);

        let props_str = r#"
    name = "MiddleMan"
    connect="3000"
    listen="3001"
    "#;
        let middle_man = Hive::new_from_str(props_str);
        let mut middle_hand = middle_man.go(true);

        let props_str = r#"
    connect="3001"
    name = "Client"
    something_else_entirely=1
    "#;
        let mut client_hive = Hive::new_from_str(props_str);

        client_hive
            .get_mut_property(thing_key)
            .unwrap()
            .on_next(move |value| {
                info!(
                    "CLIENT!! --------------------- client thing changed: {:?}",
                    value
                );
                counter2.fetch_add(1, Ordering::SeqCst);
                let (lock, cvar) = &*ack_clone2;
                let mut done = lock.lock().unwrap();
                *done += 1;
                cvar.notify_one();
            });

        let mut client_hand = client_hive.go(true);

        block_on(async {
            middle_hand
                .set_property("something_else_entirely", Some(&4.into()))
                .await;
            let (lock, cvar) = &*ack;
            let mut done = lock.lock().unwrap();

            while *done < 2 {
                info!(":::: hmmmm, {:?}", done);
                done = cvar.wait(done).unwrap();
            }
            assert_eq!(counter.load(Ordering::Relaxed), 2);

            client_hand
                .set_property("something_else_entirely", Some(&5.into()))
                .await;

            while *done < 4 {
                info!(":::: hmmmm, {:?}", done);
                done = cvar.wait(done).unwrap();
            }

            assert_eq!(counter.load(Ordering::Relaxed), 4);
        });
    });
}
