use std::borrow::Borrow;
use std::collections::HashMap;
// use crate::signal::Signal;
use std::convert::TryFrom;
use std::fmt;
use std::pin::Pin;
use std::sync::{Condvar, Mutex, RwLock};
use std::task::{Context, Poll};

use async_std::stream::Stream;
use async_std::sync::Arc;
use async_std::task::block_on;
use bytes::{BufMut, Bytes, BytesMut};
#[allow(unused_imports)]
use log::{debug, info};
use toml::Value;

use crate::hive::{DELETE, PROPERTIES, PROPERTY};

pub type PropertyType = toml::Value;


#[derive(Default, Clone)]
pub struct PropertyStream
{
    has_next: Arc<(Mutex<bool>, Condvar)>,
    pub value: Arc<RwLock<Option<PropertyType>>>,

}

// impl Sink<PropertyType> for PropertyStream {
//     type Error = ();
//
//     fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
//         unimplemented!()
//     }
//
//     fn start_send(self: Pin<&mut Self>, item: PropertyType) -> Result<(), Self::Error> {
//         unimplemented!()
//     }
//
//     fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
//         unimplemented!()
//     }
//
//     fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
//         unimplemented!()
//     }
// }

impl Stream for PropertyStream {
    type Item = Value;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let (lock, cvar) = &*self.has_next;
        let is_reeady = lock.lock().unwrap();

        let _ = cvar.wait(is_reeady).unwrap();
        let a = &*self.value.read().unwrap();


        return Poll::Ready(a.clone());
    }
}

impl fmt::Debug for Property {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={:?}", self.name, self.value)
    }
}

// #[derive(Clone)]
pub struct Property
{
    name: Box<str>,
    pub value: Arc<RwLock<Option<PropertyType>>>,
    // on_changed was fun, kind of QT like signal/slot binding, but it's not necessary
    // without the onChanged signal I can implement Clone if I feel so inclined
    // pub on_changed: Signal<Option<PropertyType>>,
    pub stream: PropertyStream,
    on_next_holder: Arc<dyn Fn(PropertyType) + Send + Sync + 'static>,

}

impl Property {
    pub fn to_string(&self) -> String {
        let v = &*self.value.read().unwrap();
        return match v {
            Some(t) => format!("{}={}", self.name, t.to_string()),
            None => format!("{}=None", self.name),
        };
    }
    pub fn on_next<F>(&mut self, f: F) where
        F: Fn(PropertyType) + Send + Sync + 'static {
        self.on_next_holder = Arc::new(f);
    }
    pub fn from_table(table: &toml::value::Table) -> Option<Property> {
        if table.keys().len() != 1 {
            // return None
        }
        // for key in table.keys(){
        let key = table.keys().nth(0).unwrap();
        let val = table.get(key);
        let p = Property::from_toml(key.as_str(), val);
        return Some(p);
    }

    pub fn get_name(&self) -> &str {
        self.name.borrow()
    }
    pub fn new(name: &str, val: Option<PropertyType>) -> Property {
        let arc_val = Arc::new(RwLock::new(val));
        return Property {
            name: Box::from(name),
            value: arc_val.clone(),
            stream: PropertyStream {
                value: arc_val,
                has_next: Arc::new((Mutex::new(false), Condvar::new())),
            },
            // on_next_holder: Arc::new(Box::new(|_| {})),
            on_next_holder: Arc::new(|_| {}),
        };
    }
    pub fn from_str(name: &str, val: &str) -> Property {
        Property::new(name, Some(PropertyType::from(val)))
    }
    pub fn from_bool(name: &str, val: bool) -> Property {
        Property::new(name, Some(PropertyType::from(val)))
    }
    pub fn from_float(name: &str, val: f64) -> Property {
        Property::new(name, Some(PropertyType::from(val)))
    }
    pub fn from_toml(name: &str, val: Option<&toml::Value>) -> Property {
        //Property::new(name, Some(PropertyType::from(val.to_string())))
        let p = match val {
            Some(v) if v.is_str() => {
                Property::from_str(name, v.as_str().unwrap())
            }
            Some(v) if v.is_integer() => {
                Property::from_int(name, v.as_integer().unwrap())
            }
            Some(v) if v.is_bool() => {
                Property::from_bool(name, v.as_bool().unwrap())
            }
            Some(v) if v.is_float() => {
                Property::from_float(name, v.as_float().unwrap())
            }
            _ => {
                println!("<<Failed to convert Property: {:?}", name);
                Property::new(name, None)
            }
        };
        return p;
    }
    pub fn from_int(name: &str, val: i64) -> Property {
        let small_int = u32::try_from(val);
        return match small_int {
            Ok(si) => {
                Property::new(name, Some(PropertyType::from(si)))
            }
            _ => {
                Property::new(name, Some(PropertyType::from(val)))
            }
        };
    }
    pub fn set_str(&mut self, s: &str) {
        let p = PropertyType::from(s);
        self.set(p);
    }
    pub fn set_bool(&mut self, b: bool) {
        let p = PropertyType::from(b);
        self.set(p);
    }
    pub fn set_int(&mut self, s: u32) {
        let p = PropertyType::from(s);
        self.set(p);
    }
    pub fn set_float(&mut self, s: f64) {
        let p = PropertyType::from(s);
        self.set(p);
    }

    pub fn set_from_prop(&mut self, v: Property) -> bool {
        let other = &*v.value.read().unwrap();
        return self.set(other.as_ref().unwrap().clone());
    }

    pub fn set(&mut self, new_prop: PropertyType) -> bool
        where PropertyType: std::fmt::Debug + PartialEq + Sync + Send + Clone + 'static,
    {
        let does_eq = match &*self.value.read().unwrap() {
            None => { false }
            Some(pt) => { pt.eq(&new_prop) }
        };

        return if !does_eq {
            let mut rr = self.value.write().unwrap();
            *rr = Some(new_prop.clone());
            debug!("emit change");
            let stream = self.stream.has_next.clone();
            // let on_change = self.on_changed.emit(Some(new_prop));

            let on_next = async {
                let (_, cvar) = &*stream;
                cvar.notify_all();
            };
            (self.on_next_holder)(new_prop);

            block_on(async {
                // futures::join!(on_change, on_next);
                on_next.await;
            });
            true
        } else {
            debug!("value is the same, do nothing ");
            false
        };
    }
}
/*
    |P|one=1\ntwo=2\nthree=3
 */
pub(crate) fn properties_to_bytes(properties: &HashMap<String, Property>) -> Bytes {
    let mut bytes = BytesMut::new();
    bytes.put_u8(PROPERTIES);
    for p in properties {
        if p.1.value.read().unwrap().is_some() {
            bytes.put_slice(
                &property_to_bytes(Some(p.1), false).unwrap()
            );
            bytes.put_u8(b'\n');
        }
    }
    return bytes.freeze();
}
/*
    |p|one=1
 */
pub(crate) fn property_to_bytes(property: Option<&Property>, inc_head: bool) -> Option<Bytes> {
    return match property {
        Some(p) if p.value.read().unwrap().is_some() => {
            let prop_str = p.to_string();
            let bytes = if inc_head {
                let mut b = BytesMut::with_capacity(prop_str.len() + 1);
                b.put_u8(PROPERTY);
                b.put_slice(prop_str.as_bytes());
                b.freeze()
            } else {
                Bytes::from(prop_str)
            };
            Some(bytes)
        }
        Some(p) => {
            let p_name = p.get_name();
            let mut bytes = BytesMut::with_capacity(p_name.len() + 1);
            bytes.put_u8(DELETE);
            bytes.put_slice(p_name.as_bytes());
            Some(bytes.freeze())
        }
        _ => None
    };


}