use failure::Error;
use futures::future::{Future, ok};
use futures::future;
use futures::future::lazy;
use std::sync::{Arc, Mutex, RwLock};
use crate::models::{Property, PropertyType};


#[derive(Default)]
pub struct Signal<T> {
    slots: Arc<RwLock<Vec<Arc<dyn Fn(T) + Send + Sync + 'static>>>>
}

impl<T> Signal<T> {
    pub fn emit(&mut self, val: T)
    where T: Clone + Send + 'static,
    {
        let slots_clone = self.slots.clone();
        tokio::run(lazy(move || {
            for s in slots_clone.read().unwrap().iter() {
                let s_clone = s.clone();
                let v1 = val.clone();
                tokio::spawn(lazy(move || {
                    s_clone(v1.clone());
                    ok(())
                }));
            }
            ok(())
        }));
    }

    pub fn connect(&mut self, slot: impl Fn(T) + Send + Sync + 'static) {
        self.slots.write().expect("Failed to get write lock on slots").push(Arc::new(slot));
    }
}
