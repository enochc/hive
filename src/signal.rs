use failure::{Error};
use futures::future::Future;

#[derive(PartialEq, Clone)]
pub enum PropertyType {
    REAL(i64),
    INT(u32),
    SHORT(u8),
    BOOL(bool),
    STRING(Box<str>),
}


#[derive(Default)]
pub struct Signal<T> {
    slots: Vec<Box<dyn Fn(T)>>
}

impl<T: Clone> Signal<T> {
    pub fn emit(&mut self, val: &T) {
        for s in &mut self.slots {
            s(val.clone());
        }
    }

    pub fn connect(&mut self, slot: impl Fn(T) + 'static) {
        self.slots.push(Box::new(slot));
    }

    pub fn new() -> Self {
        Signal::<T>{
            slots: Vec::<Box<dyn Fn(T)>>::new()
        }
    }
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
    pub on_changed: Signal<PropertyType>,
}

impl From<i32> for Property<i32> {
    fn from(v: i32) -> Self {
        Property{
            value: Default::default(),
            on_changed: Signal::<i32>::new(),
        }
    }
}

impl<PropertyType: Clone> Property<PropertyType> {
    pub fn set_value(&mut self, v: PropertyType)
    {
        match &self.value {
            Some(v) => println!("do nothing"),
            _ => {
                let v2 = v.clone();
                self.value = Some(v);
                self.on_changed.emit(&v2);

            }
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