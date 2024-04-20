use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use async_std::task;
use async_std::task::JoinHandle;
// use tokio::task::JoinHandle;


pub struct BackOff {
    pub timeout:Duration,
    pub waiting: Arc<(Mutex<bool>, Condvar)>,
    handle: JoinHandle<()>
}

impl BackOff {
    pub async fn update<F>(mut self, f:F)->Arc<(Mutex<bool>, Condvar)>
    where F: FnOnce(), F: Send + 'static {
        println!("<<< update!");
        self.handle.cancel().await;
        println!("<<< canceled");
        let w2 = self.waiting.clone();
        self.handle = task::spawn(async move {
            println!("doing it 2 !!");
            let (x,y) = &*w2;
            let mut waiting = x.lock().expect("nope 1");
            if (*waiting) {
                // println!("waiting ...");
                let guard = y.wait(waiting).expect("nope 2");
                // println!("waiting 2 {}", guard);

                f();
            } else {
                println!("already done");
                y.notify_all();
            }
        });
        self.waiting.clone()
    }

    pub async fn new<F>(timeout: Duration, f:F) -> BackOff
    where F: FnOnce(), F: Send + 'static {
        let waiting: Arc<(Mutex<bool>, Condvar)> = Arc::new((Mutex::new(true), Condvar::new()));
        let w2 = waiting.clone();
        let w3 = waiting.clone();
        let w4 = waiting.clone();
        let t2 = timeout.clone();

        task::spawn(async move {
            task::sleep(t2).await;
            let ( x,y) = &*w3;
            *x.lock().unwrap() = false;
            let ( x,y) = &*w4;
            // let b = x.lock().unwrap();
            println!("notify all");
            y.notify_all();
            // println!("<< 2222");
        });

        let mut bb = BackOff {
            timeout,
            waiting,
            handle: task::spawn(async move {
                println!("doing it 1 !!");
                let (x,y) = &*w2;
                let guard = x.lock().unwrap();
                let w = y.wait(guard).unwrap();
                f();
            })
        };

        bb
    }
}