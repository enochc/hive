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

//    pub fn new() -> Self {
//        Signal::<T>{
//            slots: Vec::<Box<dyn Fn(T)>>::new()
//        }
//    }
}


//pub trait Property{
//    fn get() -> PropertyType;
//    fn set(v:PropertyType);
//    fn emit();
//}
#[derive(Default)]
pub struct Property<PropertyType>
{
    pub value: Option<PropertyType>,
    pub on_changed: Signal<Option<PropertyType>>,
}

//impl From<i32> for Property<i32> {
//    fn from(v: i32) -> Self {
//        Property{
//            value: Default::default(),
//            on_changed: Signal::<i32>::new(),
//        }
//    }
//}

impl<PropertyType: Clone> Property<PropertyType> {
    pub fn set_value(&mut self, v: PropertyType)
        where PropertyType: std::fmt::Debug + PartialEq + Send + Clone + 'static,
    {
        println!("<<< Setting Value: {:?}", v);
        let v_clone = v.clone();
        let op_v = Some(v);

        if !self.value.eq(&op_v) {
            self.value = op_v;
            self.on_changed.emit(Some(v_clone));
        } else {
            println!("do nothing ")
        }
    }
}


//#[derive(Default)]
//pub struct Counter {
//    pub value: i32,
//    pub value_changed: Signal<i32>,
//}
//
//impl Counter {
//    pub fn set_value(&mut self, v: i32) {
//        if v != self.value {
//            self.value = v;
//            self.value_changed.emit(v);
//        }
//    }
//
//    pub fn value(&self) -> i32 {
//        self.value
//    }
//}

//fn main() {
//    let mut a = Counter::default();
//    let b = Arc::new(Mutex::new(Counter::default()));
//
//    a.value_changed.connect({let b = b.clone(); move |v| b.lock().set_value(v)});
//
//    a.set_value(7);
//    println!("{}", b.lock().value());
//}