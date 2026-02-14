use std::sync::Arc;
use hive::hive::Hive;
use hive::init_logging;
use hive::property::Property;
use hive::LevelFilter;
#[allow(unused_imports)]
use log::{debug, info};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use hive::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 5)]
async fn middle_man_test() {
    let result = tokio::time::timeout(Duration::from_millis(3_000), async {
        init_logging(Some(LevelFilter::Debug));
        info!("blah blah blah");
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
        let mut server_hive = Hive::new_from_str_unknown(props_str);
        let thing_key = &Property::hash_id("something_else_entirely");

        server_hive.get_mut_property(thing_key).unwrap().on_next(move |value| {
            info!("SERVER ----------------------- server thing changed: {:?}", value);
            counter1.fetch_add(1, Ordering::SeqCst);
            let (lock, cvar) = &*ack_clone;
            let mut done = lock.lock().unwrap();
            *done += 1;
            cvar.notify_one();
        });
        let cancellation_token = CancellationToken::new();
        server_hive.go(true, cancellation_token.clone());

        let props_str = r#"
    name = "MiddleMan"
    connect="3000"
    listen="3001"
    "#;
        let middle_man = Hive::new_from_str_unknown(props_str);
        let mut middle_hand = middle_man.go(true, cancellation_token.clone());

        let props_str = r#"
    connect="3001"
    name = "Client"
    something_else_entirely=1
    "#;
        let mut client_hive = Hive::new_from_str_unknown(props_str);

        client_hive.get_mut_property(thing_key).unwrap().on_next(move |value| {
            info!("CLIENT!! --------------------- client thing changed: {:?}", value);
            counter2.fetch_add(1, Ordering::SeqCst);
            let (lock, cvar) = &*ack_clone2;
            let mut done = lock.lock().unwrap();
            *done += 1;
            cvar.notify_one();
        });

        let mut client_hand = client_hive.go(true, cancellation_token);

        middle_hand
            .set_property("something_else_entirely", Some(&4.into()))
            .await;
        {
            let (lock, cvar) = &*ack;
            let mut done = lock.lock().unwrap();

            while *done < 2 {
                info!("::::1 hmmmm, {:?}", done);
                done = cvar.wait(done).unwrap();
            }
        }
        assert_eq!(counter.load(Ordering::Relaxed), 2);

        client_hand
            .set_property("something_else_entirely", Some(&5.into()))
            .await;

        {
            let (lock, cvar) = &*ack;
            let mut done = lock.lock().unwrap();
            while *done < 4 {
                info!("::::2 hmmmm, {:?}", done);
                done = cvar.wait(done).unwrap();
            }
        }

        assert_eq!(counter.load(Ordering::Relaxed), 4);
        info!("AND THAN SOME");
    }).await;
    info!("<<<< {:?}", result);

    if result.is_err() {
        panic!("Test timed out after 3 seconds");
    }
}
