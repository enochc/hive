
use std::sync::{Arc, RwLock};
use async_std::task;
use async_std::task::JoinHandle;
use futures::future::join_all;
use std::sync::atomic::{AtomicUsize, Ordering};
use futures::stream::FuturesUnordered;
use futures::StreamExt;


#[derive(Default)]
pub struct Signal<T> {
    slots: RwLock<Vec<Arc<dyn Fn(T) + Send + Sync + 'static>>>,
    counter: AtomicUsize,
}

async fn send_emit<T>(func: Arc<dyn Fn(T) + Send + Sync + 'static>, val: T)->bool
{
    func(val);
    true
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
        let mut handles: Vec<JoinHandle<bool>> = Vec::new();
        // let mut handles = FuturesUnordered::new();
        // Process each slot asynchronously
        for s in self.slots.read().unwrap().iter() {
            println!("one");
            let s_clone = s.clone();
            println!("a");
            let val_clone = val.clone();
            println!("b");
            handles.push(task::spawn(  async move {
                println!("c");
                send_emit(s_clone, val_clone).await
            }));

        }
        println!("tow");
        join_all(handles).await;
        // handles.for_each(|_| async { () });
        // let c = handles.next().await
        // while let Some(thing) = handles.next().await {
        //     println!("did something {:?}", thing);
        // }
        println!("three");

    }

    pub fn connect(&self, slot: impl Fn(T) + Send + Sync + 'static) {
        self.counter.fetch_add(1, Ordering::SeqCst);
        self.slots.write().expect("Failed to get write lock on slots").push(Arc::new(slot));
    }
}
