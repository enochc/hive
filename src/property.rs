use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;
use std::sync::{Condvar, Mutex, RwLock};
use std::task::{Context, Poll};

use async_std::stream::Stream;
use async_std::sync::Arc;
use async_std::task::block_on;
use bytes::{BufMut, Bytes, BytesMut, Buf};
#[allow(unused_imports)]
use log::{debug, info};
use toml::Value;

use crate::hive::{PROPERTIES, PROPERTY};
use toml::value::Table;
use std::fmt::{Debug, Formatter, Display, write, Pointer};
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::time::Duration;
use ahash;
use ahash::AHasher;
use chrono::{Local, NaiveDateTime};
use futures::future::ok;
use log::{error, warn};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug)]
pub struct PropertyValue {
    pub val: Value
}

impl PropertyValue {
    pub fn toml(&self) -> Value {
        self.val.clone()
    }
}
impl Display for PropertyValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), fmt::Error> {
        write!(f, "{}",self.val)
    }
}

impl PartialEq for PropertyValue {
    fn eq(&self, other: &Self) -> bool {
        return self.val.eq(&other.val)
    }
}
impl From<Value> for PropertyValue {
    fn from(v: Value) -> Self {
        PropertyValue {val:v}
    }
}
impl From<&Value> for PropertyValue {
    fn from(v: &Value) -> Self {
        PropertyValue {val:v.clone()}
    }
}
impl From<&str> for PropertyValue {
    fn from(v: &str) -> Self {
        PropertyValue {val:Value::from(v)}
    }
}
impl From<bool> for PropertyValue {
    fn from(v: bool) -> Self {
        PropertyValue {val:Value::from(v)}
    }
}
impl From<u32> for PropertyValue {
    fn from(v: u32) -> Self {
        PropertyValue {val:Value::from(v)}
    }
}
impl From<i32> for PropertyValue {
    fn from(v: i32) -> Self {
        PropertyValue {val:Value::from(v)}
    }
}
impl From<f32> for PropertyValue {
    fn from(v: f32) -> Self {
        PropertyValue {val:Value::from(v)}
    }
}
impl From<f64> for PropertyValue {
    fn from(v: f64) -> Self {
        PropertyValue {val:Value::from(v)}
    }
}
impl From<i64> for PropertyValue {
    fn from(v: i64) -> Self {
        PropertyValue {val:Value::from(v)}
    }
}

// impl Into<PropertyType> for toml::Value{
//     fn into(self) -> PropertyType {
//         PropertyType{toml:self}
//     }
// }

pub(crate) const IS_BOOL: i8 = 0x19;
pub(crate) const IS_STR: i8 = 0x20;
pub(crate) const IS_SHORT: i8 = 0x21; // 8 bits
pub(crate) const IS_SMALL: i8 = 0x14; // 16 bits
pub(crate) const IS_LONG: i8 = 0x15; // 32 bits
pub(crate) const IS_INT: i8 = 0x16; // 64 bits
pub(crate) const IS_FLOAT: i8 = 0x17; // 64 bits
pub(crate) const IS_NONE: i8 = 0x18;

#[derive(Default, Clone)]
pub struct PropertyStream
{
    has_next: Arc<(Mutex<bool>, Condvar)>,
    pub value: Arc<RwLock<Option<PropertyValue>>>,

}

impl Stream for PropertyStream {
    type Item = PropertyValue;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        println!("^^^^^^ ssss");
        let (lock, cvar) = &*self.has_next;
        let is_ready = lock.lock().unwrap();

        let _unused = cvar.wait(is_ready).unwrap();
        let a = &*self.value.read().unwrap();
        debug!("poll next sending....");

        return Poll::Ready(a.clone());
    }
}

impl Debug for Property {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} value = {:?}, args = {:?}", self.name, *self.value.read().unwrap(), self.args)
    }
}

struct NAME(Option<String>);
impl Display for NAME {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            None => { write!(f, "unnamed") }
            Some(n) => { write!(f, "{}", n) }
        }
    }
}
impl Debug for NAME {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl From<&str> for NAME {
    fn from(value: &str) -> Self {
        NAME(Some(String::from(value)))
    }
}
impl Borrow<str> for NAME{
    fn borrow(&self) -> &str {
        match &self.0 {
            None => {""}
            Some(n) => {n}
        }
    }
}
impl Clone for NAME{
    fn clone(&self) -> Self {
        match &self.0 {
            None => {NAME(None)}
            Some(n) => {NAME(Some(String::from(n)))}
        }
    }
}
#[derive(Clone)]
pub struct Property
{
    pub name: NAME,
    pub id: u64,
    pub value: Arc<RwLock<Option<PropertyValue>>>,
    // on_changed was fun, kind of QT like signal/slot binding, but it's not necessary
    // without it the onChanged signal I can implement Clone if I feel so inclined
    // pub on_changed: Signal<Option<PropertyType>>,
    pub stream: PropertyStream,
    on_next_holder: Arc<dyn Fn(PropertyValue) + Send + Sync + 'static>,
    pub args: Option<Table>,
    pub backoff: u64,
    last_notify: Option<NaiveDateTime>
}


impl Property {

    pub fn with_backoff(&mut self, val: &u64)->&mut Property {
        self.backoff = val.clone();
        self
    }

    pub fn hash_id(val: &str) -> u64 {
        let mut hasher = AHasher::default(); // todo replace with 32 bit hash:: https://github.com/Cyan4973/xxHash
        hasher.write(val.as_bytes());
        let rr= hasher.finish();
        // println!("<< {}: {}",val,  rr);
        rr
    }
    pub fn get_value(&self) -> Option<Value> {
        let rr = &*self.value.read().unwrap();
        match rr {
            None => { None }
            Some(v) => {
                Some(v.clone().val)
            }
        }
    }
    pub fn is_none(&self) ->bool {
        return self.value.read().unwrap().is_none();
    }
    pub fn to_string(&self) -> String {
        let v = &*self.value.read().unwrap();
        return match v {
            Some(t) => format!("{}={}", self.name, t.val.to_string()),
            None => format!("{}=None", self.name),
        };
    }
    fn rest_get(&mut self) ->Result<bool>{
        if cfg!(feature = "rest"){
            let table = self.args.as_ref().unwrap();
            match table.get("rest_get"){
                Some(url) => {
                    println!("doing a thing: {:?}", url.as_str().unwrap());
                    return Ok(true);
                },
                _ =>{}
            }
        }

        return Ok(false)


    }

    pub fn on_next<F>(&mut self, f: F) where
        F: Fn(PropertyValue) + Send + Sync + 'static {
        self.on_next_holder = Arc::new(f);
    }
    pub fn from_table(table: &Table) -> Option<Property> {
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

    pub fn from_value_name(name: &str, val:Value) -> Property
        where Value: Into<PropertyValue> {
        let id = Property::hash_id(&name);
        return Property::from_id(&id, Some(val.into()));
    }
    pub fn from_value(id: &u64, val:Value) -> Property
        where Value: Into<PropertyValue> {
        return Property::from_id(id, Some(val.into()));
    }
    pub fn from_name(name: &str, val: Option<PropertyValue>) -> Property {
        let arc_val = Arc::new(RwLock::new(val));
        return Property {
            name: NAME(Some(name.to_string())),
            id: Property::hash_id(&name),
            value: arc_val.clone(),
            stream: PropertyStream {
                value: arc_val,
                has_next: Arc::new((Mutex::new(false), Condvar::new())),
            },
            on_next_holder: Arc::new(|_| {}),
            args: None,
            backoff: 0,
            last_notify: None
        };
    }
    pub fn from_id(id: &u64, val: Option<PropertyValue>) -> Property {
        let arc_val = Arc::new(RwLock::new(val));
        return Property {
            name: NAME(None),
            id: *id,
            value: arc_val.clone(),
            stream: PropertyStream {
                value: arc_val,
                has_next: Arc::new((Mutex::new(false), Condvar::new())),
            },
            on_next_holder: Arc::new(|_| {}),
            args: None,
            backoff: 0,
            last_notify: None
        };
    }

    pub fn from_toml(name: &str, val: Option<&Value>) -> Property {
        let p = match val {
            Some(v) if v.is_str() => {
                Property::from_name(name, Some(v.into()))
            }
            Some(v) if v.is_integer() => {
                Property::from_name(name, Some(v.into()))
            }
            Some(v) if v.is_bool() => {
                Property::from_name(name, Some(v.into()))
            }
            Some(v) if v.is_float() => {
                Property::from_name(name, Some(v.into()))
            }
            Some(v) if v.is_table() => {
                /*
                If the value is also a table, we look for the "val" in the table to set as the default value.
                the rest of the table values, if any, are set as the "table" value on the Property
                */
                match v.as_table() {
                    Some(t) => {
                        let mut params = Table::new();
                        let mut val:Option<Value> = None;
                        for key in t.keys(){
                            match t.get(key){
                                Some(v) => {
                                    if key == "val"{
                                        val = Some(v.clone())
                                    } else {
                                        params.insert(key.clone(), v.clone());
                                    }
                                }
                                None => {}
                            }
                        }

                        let mut p = Property::from_name(name, Some(val.unwrap().into()));
                        if params.keys().len() >0 {
                            p.args = Some(params);
                        }
                        p.rest_get().expect("Failed to get api and point");
                        return p;
                    }
                    _ => {}
                };
                Property::from_name(name, None)

            }
            _ => {
                debug!("<<Failed to convert Property: {:?}: {:?}", name, val);
                Property::from_name(name, None)
            }
        };
        return p;
    }

    pub fn set_from_prop(&mut self, v: Property) -> bool {
        let other = &*v.value.read().unwrap();
        return self.set_value(other.as_ref().unwrap().clone());
    }

    pub fn set_value(&mut self, new_prop: PropertyValue) -> bool
        where PropertyValue: Debug + PartialEq + Sync + Send + Clone + 'static,
    {
        let does_eq = match &*self.value.read().unwrap() {
            None => { false }
            Some(pt) => { pt.eq(&new_prop) }
        };
        debug!("!does_eq: {}", !does_eq);

        return if !does_eq {
            *self.value.write().unwrap() = Some(new_prop.clone());

            debug!("emit change");
            let stream = self.stream.has_next.clone();

            let (sending, cvar) = &*stream;
            if self.backoff > 0 {
                match self.last_notify {
                    Some(last_time)=> {
                        // do the magic here!!!
                        let ss = *sending.lock().unwrap();
                        if(!ss) {
                            *sending.lock().unwrap() = true;
                            let duration = self.backoff.clone();
                            let stream_clone = stream.clone();
                            let hh = self.on_next_holder.clone();
                            let rr = new_prop.clone();

                            async_std::task::spawn(async move {
                                async_std::task::sleep(Duration::from_millis(duration)).await;
                                let (x, y) = &*stream_clone;
                                *x.lock().unwrap() = false;
                                y.notify_all();
                                (hh)(rr);
                            });
                        } else {
                            error!("!!! already sending ......");
                        }

                        info!("<<< hmmm {:?} {:?}", last_time, sending.lock())
                    }
                    _ => {
                        // not set, set it!!
                        cvar.notify_all();
                        self.last_notify = Some(Local::now().naive_utc());
                        debug!("notified!");
                    }
                }
            } else {
                // no backoff, just set it!
                cvar.notify_all();
                self.last_notify = Some(Local::now().naive_utc());

                (self.on_next_holder)(new_prop);
                debug!("notified!");
            }

            // let on_next = async {
            //     let (_, cvar) = &*stream;
            //     cvar.notify_all();
            // };
            // (self.on_next_holder)(new_prop);

            // block_on(async {
            //     on_next.await;
            // });

            true
        } else {
            debug!("value is the same, do nothing ");
            false
        };
    }
}

// I'm not currently using this anywhere
impl SetProperty<&str> for Property{
    fn set(&mut self, v: &str) {
        let p = PropertyValue::from(v);
        self.set_value(p);
    }
}

pub trait SetProperty<T> {
    fn set(&mut self, v:T);
}
/*
    |P|one=1\ntwo=2\nthree=3
 */
pub(crate) fn properties_to_bytes(properties: &HashMap<u64, Property>) -> Bytes {
    let mut bytes = BytesMut::new();
    bytes.put_u8(PROPERTIES);
    for p in properties {
        if p.1.value.read().unwrap().is_some() {
            bytes.put_slice(
                &property_to_bytes(p.1, false)
            );
        }
    }
    return bytes.freeze();
}

use num_traits::ToPrimitive;

use toml::Value::Datetime;
// use sha1::Digest;

pub(crate) fn property_to_bytes(property: &Property, inc_head: bool) -> Bytes {
    let mut bytes = BytesMut::new();
    if inc_head {
        bytes.put_u8(PROPERTY);
    }
    bytes.put_u64(property.id);

    match &*property.value.read().unwrap() {
        None => {
            bytes.put_i8(IS_NONE);
        }
        Some(p) => {
            match &p.val  {
                Value::String(str) => {
                    bytes.put_i8(IS_STR);
                    let str_length: u8 = str.len() as u8;
                    bytes.put_u8(str_length);
                    bytes.put_slice(str.as_bytes())
                }
                Value::Integer(int) => {
                    match int.to_i8() {
                        Some(i) => {
                            bytes.put_i8(IS_SHORT);
                            bytes.put_i8(i);
                        }
                        None => {
                            match int.to_i16() {
                                Some(i) => {
                                    bytes.put_i8(IS_SMALL);
                                    bytes.put_i16(i);
                                }
                                None => {
                                    match int.to_i32() {
                                        Some(i) => {
                                            bytes.put_i8(IS_LONG);
                                            bytes.put_i32(i);
                                        }
                                        None => {
                                            match int.to_i64() {
                                                Some(i) => {
                                                    bytes.put_i8(IS_INT);
                                                    bytes.put_i64(i);
                                                }
                                                None => {
                                                    unimplemented!("You really shouldn't be here {:?}", p)
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Value::Float(f) => {
                    bytes.put_i8(IS_FLOAT);
                    bytes.put_f64(*f);
                }
                Value::Boolean(b) => {
                    bytes.put_i8(IS_BOOL);
                    bytes.put_u8(if *b { 1 } else { 0 });
                }
                Value::Datetime(_) => {
                    unimplemented!("not implemented for {:?}", p);
                }
                Value::Array(_) => {
                    unimplemented!("not implemented for {:?}", p);
                }
                Value::Table(_) => {
                    unimplemented!("not implemented for {:?}", p);
                }
            }
        }
    }
    return bytes.freeze();
}

pub(crate) fn bytes_to_property(bytes:&mut Bytes) -> Option<Property> {
    let property_id = bytes.get_u64();
    debug!("id: {:?}", property_id);

    let value_type = bytes.get_i8();
    match value_type {
        IS_STR => {
            let str_length = bytes.get_u8() as usize;
            let value = String::from_utf8(bytes.slice(..str_length).to_vec());
            bytes.advance(str_length);
            debug!("<<<<<<<<<<<<<<<<<<<<<<<<< string {:?} = {:?}", str_length, value);
            let v = value.ok().unwrap();
            return Some(Property::from_value(&property_id, v.into()));
        },
        IS_BOOL => {
            let bool = bytes.get_u8() > 0;
            return Some(Property::from_value(&property_id, bool.into()));
        }
        IS_SHORT => {
            let short = bytes.get_i8();
            return Some(Property::from_value(&property_id, short.into()));
        }
        IS_SMALL => {
            let small = bytes.get_i16();
            return Some(Property::from_value(&property_id, (small as i32).into()));
        }
        IS_LONG => {
            let long = bytes.get_i32();
            return Some(Property::from_value(&property_id, long.into()));
        }
        IS_INT => {
            let int = bytes.get_i64();
            return Some(Property::from_value(&property_id, int.into()));
        }
        IS_FLOAT => {
            let float = bytes.get_f64();
            return Some(Property::from_value(&property_id, float.into()));
        }
        IS_NONE => {
            return Some(Property::from_id(&property_id, None));
        }
        _ => {
            unimplemented!(".... doh, finish me {:?}", value_type)
            // return None
        }
    }
}