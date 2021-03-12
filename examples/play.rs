
use async_std::stream::Stream;
use std::pin::Pin;
use async_std::task::{Context, Poll};
use async_std::sync::Arc;
use std::sync::{Condvar, Mutex};
use hive::property::PropertyValue;
use toml::Value;
use std::time::Duration;
use std::thread::sleep;


/// A stream which counts from one to five
#[derive(Clone)]
struct Counter {
    count: u32,
    ready: Arc<(Mutex<bool>, Condvar)>,
}


impl Counter {
    fn new() -> Counter {

        let ready = Arc::new((Mutex::new(false), Condvar::new()));
        let ready_clone = ready.clone();

        let counter = Counter {
            count: 0,
            ready,
        };

        async_std::task::spawn(async move {
            let (_lock, cvar) = &*ready_clone;
            println!("__ task spawn");
            loop {
                sleep(Duration::from_secs(1));
                cvar.notify_all();
            }
        });
        return counter;
    }
}


impl Stream for Counter {
    type Item = Value;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {

        self.count = self.count + 1;
        let (lock, cvar) = &*self.ready;
        let is_reeady = lock.lock().unwrap();

        let _ = cvar.wait(is_reeady).unwrap();

        if self.count < 6 {
            let op = PropertyValue::from(self.count);
            let ss = Some(op);
            return Poll::Ready(ss);
        }else {
            println!("done");
            return Poll::Ready(None);
        }

    }
}
fn main(){
    // And now we can use it!
    use async_std::stream::StreamExt;

    let mut counter = Counter::new();

    let mut counter_clone = counter.clone();
    async_std::task::spawn(async move {
        while let Some(x) = counter_clone.next().await {
            println!("me too: {}", x);
        }
        println!("also done");

    });

    async_std::task::block_on(async {
        while let Some(x) = counter.next().await {
            println!("{}", x);
        }
        println!("really done");

    });


}
