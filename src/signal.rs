
use std::sync::{Arc, RwLock};
use async_std::task;
use async_std::task::JoinHandle;
use futures::future::join_all;
use std::sync::atomic::{AtomicUsize, Ordering};


#[derive(Default)]
pub struct Signal<T> {
    slots: RwLock<Vec<Arc<dyn Fn(T) + Send + Sync + 'static>>>,
    counter: AtomicUsize,
}

async fn send_emit<T>(func: Arc<dyn Fn(T) + Send + Sync + 'static>, val: T)
{
    func(val)
}

impl<T> Signal<T>{
    pub fn num_slots(self) ->usize {
        return self.slots.read().unwrap().len();
    }

    pub async fn emit(&mut self, val: T)
        where T:Sync + Clone + Send +'static,
    {
        let count = self.counter.load(Ordering::Relaxed);
        println!("EMITTING:: {}", count);
        let mut handles: Vec<JoinHandle<()>> = Vec::new();
        // Process each slot asynchronously
        for s in self.slots.read().unwrap().iter() {
            let s_clone = s.clone();
            let val_clone = val.clone();

            handles.push(task::spawn(  async move {
                send_emit(s_clone, val_clone).await;
            }));

        }

        join_all(handles).await;

    }

    pub fn connect(&self, slot: impl Fn(T) + Send + Sync + 'static) {
        self.counter.fetch_add(1, Ordering::SeqCst);
        self.slots.write().expect("Failed to get write lock on slots").push(Arc::new(slot));
    }
}
