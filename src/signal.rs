use std::sync::{Arc, RwLock};

use async_std::task;
use async_std::task::JoinHandle;
use futures::future::join_all;

#[derive(Default)]
pub struct Signal<T> {
    slots: RwLock<Vec<Arc<dyn Fn(T) + Send + Sync + 'static>>>
}

async fn send_emit<T>(func: Arc<dyn Fn(T) + Send + Sync + 'static>, val: T) { func(val) }

impl<T> Signal<T> {

    pub fn new() -> Signal<T>{
        return Signal{ slots: Default::default() }
    }

    pub async fn emit(&mut self, val: T)
        where T: Sync + Clone + Send + 'static,
    {
        let mut handles: Vec<JoinHandle<()>> = Vec::new();
        // Spawn thread for each attached slot
        for s in self.slots.read().unwrap().iter() {
            let s_clone = s.clone();
            let val_clone = val.clone();

            handles.push(task::spawn(async move {
                send_emit(s_clone, val_clone).await;
            }));
        }

        join_all(handles).await;
    }

    pub fn connect(&self, slot: impl Fn(T) + Send + Sync + 'static) {
        self.slots.write().expect("Failed to get write lock on slots").push(Arc::new(slot));
    }
}
