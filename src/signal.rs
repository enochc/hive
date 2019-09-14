use failure::Error;
use futures::future::{Future, ok};
use futures::future;
use futures::future::lazy;
use std::sync::{Arc, Mutex, RwLock};


#[derive(PartialEq, Clone, Debug)]
pub enum PropertyType {
    REAL(i64),
    INT(u32),
    SHORT(u8),
    BOOL(bool),
    STRING(Box<str>),
}

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


#[derive(Default)]
pub struct Property<PropertyType>
{
    value: Option<PropertyType>,
    pub on_changed: Signal<Option<PropertyType>>,
}


impl<PropertyType: Clone> Property<PropertyType> {
    pub fn set(&mut self, v: PropertyType)
        where PropertyType: std::fmt::Debug + PartialEq + Send + Clone + 'static,
    {

        let v_clone = v.clone();
        let op_v = Some(v);

        if !self.value.eq(&op_v) {
            self.value = op_v;
            self.on_changed.emit(Some(v_clone));
        } else {
            println!("do nothing ")
        }
    }

    pub fn get(&self) -> &Option<PropertyType> {
        &self.value
    }
}

