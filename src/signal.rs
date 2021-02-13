use std::sync::{Arc};
use async_std::task;
use futures::future::join_all;

use std::sync::atomic::{AtomicUsize, Ordering};
use async_std::sync::Mutex;
use async_std::task::{block_on};
use log::{debug};


#[derive(Default)]
pub struct Signal<T>
    where T: Send, {
    slots: Mutex<Vec<Arc<dyn Fn(T) + Send + Sync + 'static>>>,
    counter: AtomicUsize,
}

async fn send_emit<T>(func: Arc<dyn Fn(T) + Send + Sync + 'static>, val: T)
{
    func(val);
}

impl<T> Signal<T>
    where T: Send, {
    pub async fn num_slots(self) -> usize {
        return self.slots.lock().await.len();
    }

    pub async fn emit(&mut self, val: T)
        where T: Sync + Clone + Send + 'static,
    {
        let count = self.counter.load(Ordering::Relaxed);
        debug!("EMITTING:: {}", count);
        let mut futures = vec![];

        for s in self.slots.lock().await.iter() {
            let s_clone = s.clone();
            let val_clone = val.clone();

            let h = task::spawn(async move{
                send_emit(s_clone, val_clone).await;
            });

            futures.push(h);
        }

        join_all(futures).await;
    }

    pub fn connect(&self, slot: impl Fn(T) + Send + Sync + 'static) {
        self.counter.fetch_add(1, Ordering::SeqCst);
        let mut slots = block_on(self.slots.lock());
        slots.push(Arc::new(slot));
    }
}
