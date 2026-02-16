use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use log::{debug, info, LevelFilter};

use hive::hive::Hive;
use hive::init_logging;
use hive::property::{Property, PropertyValue};

/// Verify that the backoff (debounce) mechanism on Property suppresses
/// rapid `on_next` callbacks while still updating the watch channel.
///
/// Timeline:
///   t=0ms    set_value(6)  → on_next fires immediately (counter=1), timer starts
///   t=100ms  set_value(7)  → suppressed (stashed as pending)
///   t=200ms  set_value(8)  → suppressed (replaces pending with 8)
///   t=500ms  timer expires → on_next fires with latest value 8 (counter=2)
#[tokio::test(flavor = "multi_thread")]
async fn property_backoff_debounce() {
    init_logging(Some(LevelFilter::Debug));

    let props_str = r#"
    listen = "127.0.0.1:3000"
    name = "Server"
    [Properties]
    thingvalue = 1
    "#;
    let mut hive = Hive::new_from_str_unknown(props_str);

    let prop = hive.get_mut_property_by_name("thingvalue").unwrap();
    prop.set_backoff(&500);

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    // Use a channel to be notified when on_next fires.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PropertyValue>();

    prop.on_next(move |value| {
        info!("on_next fired: {:?}", value);
        counter_clone.fetch_add(1, Ordering::SeqCst);
        let _ = tx.send(value);
    });

    // First set — should fire on_next immediately and start the backoff timer.
    prop.set_value(6.into());
    let val = rx.recv().await.expect("expected first on_next");
    assert_eq!(val.val.as_integer().unwrap(), 6);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Rapid updates within the backoff window — on_next should be suppressed.
    tokio::time::sleep(Duration::from_millis(100)).await;
    prop.set_value(7.into());
    tokio::time::sleep(Duration::from_millis(100)).await;
    prop.set_value(8.into());

    // Counter should still be 1 — intermediate updates are absorbed.
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // But the watch channel value should always be current.
    assert_eq!(prop.get_value().unwrap().as_integer().unwrap(), 8);

    // Wait for the backoff timer to expire and deliver the pending value.
    let val = rx.recv().await.expect("expected second on_next after backoff");
    assert_eq!(val.val.as_integer().unwrap(), 8);
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    info!("backoff test passed");
}

/// Verify that when backoff is 0 (default), every change fires on_next immediately.
#[tokio::test(flavor = "multi_thread")]
async fn property_no_backoff() {
    init_logging(Some(LevelFilter::Debug));

    let mut prop = Property::from_name("test_prop", Some(PropertyValue::from(0)));

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PropertyValue>();

    prop.on_next(move |value| {
        counter_clone.fetch_add(1, Ordering::SeqCst);
        let _ = tx.send(value);
    });

    prop.set_value(1.into());
    let _ = rx.recv().await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    prop.set_value(2.into());
    let _ = rx.recv().await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    prop.set_value(3.into());
    let _ = rx.recv().await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}
