
use crate::hive::{PROPERTIES, PROPERTY};

use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::watch;
use tokio_stream::Stream;
use tokio_stream::wrappers::WatchStream;
use std::sync::Arc;
use bytes::{Buf, BufMut, Bytes, BytesMut};

#[allow(unused_imports)]
use log::{debug, info};
use toml::Value;

use toml::value::Table;
use std::fmt::{Debug, Formatter, Display, write, Pointer};
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use ahash;
use ahash::AHasher;

pub type PropertyType = toml::Value;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug)]
pub struct PropertyValue {
    pub val: Value,
}

impl PropertyValue {
    pub fn toml(&self) -> Value {
        self.val.clone()
    }
}

impl Display for PropertyValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), fmt::Error> {
        write!(f, "{}", self.val)
    }
}

impl PartialEq for PropertyValue {
    fn eq(&self, other: &Self) -> bool {
        return self.val.eq(&other.val);
    }
}

impl From<Value> for PropertyValue {
    fn from(v: Value) -> Self {
        PropertyValue { val: v }
    }
}

impl From<&Value> for PropertyValue {
    fn from(v: &Value) -> Self {
        PropertyValue { val: v.clone() }
    }
}

impl From<&str> for PropertyValue {
    fn from(v: &str) -> Self {
        PropertyValue { val: Value::from(v) }
    }
}

impl From<bool> for PropertyValue {
    fn from(v: bool) -> Self {
        PropertyValue { val: Value::from(v) }
    }
}

impl From<u32> for PropertyValue {
    fn from(v: u32) -> Self {
        PropertyValue { val: Value::from(v) }
    }
}

impl From<i32> for PropertyValue {
    fn from(v: i32) -> Self {
        PropertyValue { val: Value::from(v) }
    }
}

impl From<f32> for PropertyValue {
    fn from(v: f32) -> Self {
        PropertyValue { val: Value::from(v) }
    }
}

impl From<f64> for PropertyValue {
    fn from(v: f64) -> Self {
        PropertyValue { val: Value::from(v) }
    }
}

impl From<i64> for PropertyValue {
    fn from(v: i64) -> Self {
        PropertyValue { val: Value::from(v) }
    }
}

pub(crate) const IS_BOOL: i8 = 0x19;
pub(crate) const IS_STR: i8 = 0x20;
pub(crate) const IS_SHORT: i8 = 0x21;
// 8 bits
pub(crate) const IS_SMALL: i8 = 0x14;
// 16 bits
pub(crate) const IS_LONG: i8 = 0x15;
// 32 bits
pub(crate) const IS_INT: i8 = 0x16;
// 64 bits
pub(crate) const IS_FLOAT: i8 = 0x17;
// 64 bits
pub(crate) const IS_NONE: i8 = 0x18;

/// Reactive stream for observing property value changes.
///
/// Backed by a `tokio::sync::watch` channel — a single current value shared
/// between one writer and many readers. Each clone gets an independent stream
/// cursor that yields only values changed *after* the clone was created.
///
/// The watch channel also serves as the canonical storage for the property's
/// current value, eliminating the need for a separate `Arc<RwLock<...>>`.
pub struct PropertyStream {
    sender: Arc<watch::Sender<Option<PropertyValue>>>,
    stream: WatchStream<Option<PropertyValue>>,
}

impl Clone for PropertyStream {
    fn clone(&self) -> Self {
        // subscribe() creates a new independent receiver starting from the current value.
        // from_changes() skips the current value and only yields future updates,
        // matching the semantics of the previous broadcast-based implementation.
        PropertyStream {
            sender: self.sender.clone(),
            stream: WatchStream::from_changes(self.sender.subscribe()),
        }
    }
}

impl Default for PropertyStream {
    fn default() -> Self {
        let (sender, rx) = watch::channel(None);
        PropertyStream {
            sender: Arc::new(sender),
            stream: WatchStream::from_changes(rx),
        }
    }
}

impl PropertyStream {
    fn new(initial: Option<PropertyValue>) -> Self {
        let (sender, rx) = watch::channel(initial);
        PropertyStream {
            sender: Arc::new(sender),
            stream: WatchStream::from_changes(rx),
        }
    }
}

impl Stream for PropertyStream {
    type Item = PropertyValue;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // WatchStream yields Option<PropertyValue>.
        // We filter out None (property cleared/deleted) and only surface real values.
        // When the watch sender is dropped, the stream closes naturally.
        loop {
            match Pin::new(&mut self.stream).poll_next(cx) {
                Poll::Ready(Some(Some(val))) => return Poll::Ready(Some(val)),
                Poll::Ready(Some(None)) => continue, // Value cleared, keep waiting
                Poll::Ready(None) => return Poll::Ready(None), // Sender dropped, stream ends
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl Debug for Property {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let val = self.stream.sender.borrow();
        write!(
            f,
            "{} value = {:?}, args = {:?}",
            self.name,
            &*val,
            self.args
        )
    }
}

pub struct NAME(Option<String>);

impl Display for NAME {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            None => {
                write!(f, "unnamed")
            }
            Some(n) => {
                write!(f, "{}", n)
            }
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
impl std::borrow::Borrow<str> for NAME {
    fn borrow(&self) -> &str {
        match &self.0 {
            None => "",
            Some(n) => n,
        }
    }
}
impl Clone for NAME {
    fn clone(&self) -> Self {
        match &self.0 {
            None => NAME(None),
            Some(n) => NAME(Some(String::from(n))),
        }
    }
}

#[derive(Clone)]
pub struct Property {
    pub name: NAME,
    pub id: u64,
    pub stream: PropertyStream,
    on_next_holder: Arc<dyn Fn(PropertyValue) + Send + Sync + 'static>,
    pub args: Option<Table>,
    /// Debounce duration in milliseconds for `on_next` callbacks.
    /// When > 0, the first value change fires `on_next` immediately,
    /// then subsequent changes within the window are absorbed.
    /// When the timer expires, the latest suppressed value (if any)
    /// is delivered via `on_next`.
    ///
    /// The watch-channel stream is always updated immediately
    /// regardless of backoff, so `get_value()` always returns the
    /// current value and stream subscribers see every change.
    pub backoff: u64,
    /// Shared flag: true while a backoff timer task is sleeping.
    backoff_active: Arc<AtomicBool>,
    /// Latest value suppressed during an active backoff window.
    /// Taken and delivered by the timer task when it wakes.
    backoff_pending: Arc<std::sync::Mutex<Option<PropertyValue>>>,
}

impl Property {
    pub fn with_backoff(&mut self, val: &u64) -> & Property {
        self.backoff = val.clone();
        self
    }

    pub fn set_backoff(&mut self, val: &u64) {
        self.backoff = val.clone();
    }

    pub fn hash_id(val: &str) -> u64 {
        let now: SystemTime = SystemTime::now();
        let mut hasher = AHasher::default(); // todo replace with 32 bit hash:: https://github.com/Cyan4973/xxHash
        hasher.write(val.as_bytes());

        hasher.finish()
    }

    /// Returns a clone of the current toml Value, if set.
    pub fn get_value(&self) -> Option<Value> {
        let current = self.stream.sender.borrow();
        current.as_ref().map(|v| v.val.clone())
    }

    /// Returns a clone of the current PropertyValue, if set.
    pub fn current_value(&self) -> Option<PropertyValue> {
        self.stream.sender.borrow().clone()

    }

    pub fn is_none(&self) -> bool {
        self.stream.sender.borrow().is_none()
    }

    pub fn to_string(&self) -> String {
        let v = self.stream.sender.borrow();
        match &*v {
            Some(t) => format!("{}={}", self.name, t.val.to_string()),
            None => format!("{}=None", self.name),
        }
    }

    fn rest_get(&mut self) -> Result<bool> {
        if cfg!(feature = "rest") {
            let table = self.args.as_ref().unwrap();
            match table.get("rest_get") {
                Some(url) => {
                    println!("doing a thing: {:?}", url.as_str().unwrap());
                    return Ok(true);
                }
                _ => {}
            }
        }

        Ok(false)
    }

    pub fn on_next<F>(&mut self, f: F)
    where
        F: Fn(PropertyValue) + Send + Sync + 'static,
    {
        self.on_next_holder = Arc::new(f);
    }
    pub fn from_table(table: &Table) -> Option<Property> {
        if table.keys().len() != 1 {
            // return None
        }
        let key = table.keys().nth(0).unwrap();
        let val = table.get(key);
        let p = Property::from_toml(key.as_str(), val);
        Some(p)
    }

    pub fn get_name(&self) -> &str {
        match &self.name.0 {
            None => "",
            Some(n) => n.as_str(),
        }
    }

    pub fn from_value(id: &u64, val: Value) -> Property
    where
        Value: Into<PropertyValue>,
    {
        Property::from_id(id, Some(val.into()))
    }

    pub fn from_name(name: &str, val: Option<PropertyValue>) -> Property {
        Property {
            name: NAME(Some(name.to_string())),
            id: Property::hash_id(&name),
            stream: PropertyStream::new(val),
            on_next_holder: Arc::new(|_| {}),
            args: None,
            backoff: 0,
            backoff_active: Arc::new(AtomicBool::new(false)),
            backoff_pending: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub fn from_id(id: &u64, val: Option<PropertyValue>) -> Property {
        Property {
            name: NAME(None),
            id: *id,
            stream: PropertyStream::new(val),
            on_next_holder: Arc::new(|_| {}),
            args: None,
            backoff: 0,
            backoff_active: Arc::new(AtomicBool::new(false)),
            backoff_pending: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub fn from_toml(name: &str, val: Option<&Value>) -> Property {
        let p = match val {
            Some(v) if v.is_str() => Property::from_name(name, Some(v.into())),
            Some(v) if v.is_integer() => Property::from_name(name, Some(v.into())),
            Some(v) if v.is_bool() => Property::from_name(name, Some(v.into())),
            Some(v) if v.is_float() => Property::from_name(name, Some(v.into())),
            Some(v) if v.is_table() => {
                /*
                If the value is also a table, we look for the "val" in the table to set as the default value.
                the rest of the table values, if any, are set as the "table" value on the Property
                */
                match v.as_table() {
                    Some(t) => {
                        let mut params = Table::new();
                        let mut val: Option<Value> = None;
                        for key in t.keys() {
                            match t.get(key) {
                                Some(v) => {
                                    if key == "val" {
                                        val = Some(v.clone())
                                    } else {
                                        params.insert(key.clone(), v.clone());
                                    }
                                }
                                None => {}
                            }
                        }

                        let mut p = Property::from_name(name, Some(val.unwrap().into()));
                        if params.keys().len() > 0 {
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
        p
    }

    pub fn set_from_prop(&mut self, v: Property) -> bool {
        let other = v.stream.sender.borrow();
        match &*other {
            Some(pv) => self.set_value(pv.clone()),
            None => false,
        }
    }

    pub fn set_value(&mut self, new_prop: PropertyValue) -> bool
    where
        PropertyValue: Debug + PartialEq + Sync + Send + Clone + 'static,
    {
        let does_eq = {
            let current = self.stream.sender.borrow();
            match &*current {
                None => false,
                Some(pt) => pt.eq(&new_prop),
            }
        }; // borrow dropped before we modify

        if !does_eq {
            debug!("emit change (backoff={})", self.backoff);

            // Always update the watch channel immediately so get_value()
            // and stream subscribers reflect the latest state.
            self.stream.sender.send_modify(|val| *val = Some(new_prop.clone()));

            if self.backoff > 0 {
                // Debounce: gate the on_next callback with a timer.
                if !self.backoff_active.swap(true, Ordering::SeqCst) {
                    // No timer running — fire on_next immediately, then start the backoff window.
                    (self.on_next_holder)(new_prop);

                    let active = self.backoff_active.clone();
                    let pending = self.backoff_pending.clone();
                    let on_next = self.on_next_holder.clone();
                    let duration = self.backoff;

                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(duration)).await;
                        active.store(false, Ordering::SeqCst);
                        // Deliver the latest suppressed value, if any.
                        let val = pending.lock().unwrap().take();
                        if let Some(v) = val {
                            (on_next)(v);
                        }
                    });
                } else {
                    // Timer is running — stash the latest value; the timer
                    // task will deliver it when the backoff window closes.
                    debug!("backoff active, suppressing on_next");
                    *self.backoff_pending.lock().unwrap() = Some(new_prop);
                }
            } else {
                (self.on_next_holder)(new_prop);
            }

            true
        } else {
            debug!("value is the same, do nothing ");
            false
        }
    }

}
// I'm not currently using this anywhere
impl SetProperty<&str> for Property {
    fn set(&mut self, v: &str) {
        let p = PropertyValue::from(v);
        self.set_value(p);
    }
}

pub trait SetProperty<T> {
    fn set(&mut self, v: T);
}
/*
   |P|one=1\ntwo=2\nthree=3
*/
pub(crate) fn properties_to_bytes(properties: &HashMap<u64, Property>) -> Bytes {
    let mut bytes = BytesMut::new();
    bytes.put_u8(PROPERTIES);
    for p in properties {
        if p.1.stream.sender.borrow().is_some() {
            bytes.put_slice(&property_to_bytes(p.1, false));
        }
    }
    bytes.freeze()
}

use num_traits::ToPrimitive;

/// Encode just the type-tagged value portion of the Hive wire format.
///
/// Writes `[type_tag][value_bytes]` without the leading `u64` property ID
/// or the `PROPERTY` header byte.  Used by the MQTT peer where the property
/// identity is carried by the MQTT topic.
pub fn property_value_to_bytes(property: &Property) -> Bytes {
    let mut bytes = BytesMut::new();
    let val = property.stream.sender.borrow();
    match &*val {
        None => {
            bytes.put_i8(IS_NONE);
        }
        Some(p) => {
            if p.val.is_bool() {
                bytes.put_i8(IS_BOOL);
                bytes.put_u8(if p.val.as_bool().unwrap() { 1 } else { 0 });
            } else if p.val.is_str() {
                bytes.put_i8(IS_STR);
                let str_val = p.val.as_str().unwrap();
                let str_length: u8 = str_val.len() as u8;
                bytes.put_u8(str_length);
                bytes.put_slice(str_val.as_bytes())
            } else if p.val.is_integer() {
                let int = p.val.as_integer().unwrap();
                match int.to_i8() {
                    Some(i) => {
                        bytes.put_i8(IS_SHORT);
                        bytes.put_i8(i);
                    }
                    None => match int.to_i16() {
                        Some(i) => {
                            bytes.put_i8(IS_SMALL);
                            bytes.put_i16(i);
                        }
                        None => match int.to_i32() {
                            Some(i) => {
                                bytes.put_i8(IS_LONG);
                                bytes.put_i32(i);
                            }
                            None => match int.to_i64() {
                                Some(i) => {
                                    bytes.put_i8(IS_INT);
                                    bytes.put_i64(i);
                                }
                                None => {
                                    unimplemented!("You really shouldn't be here {:?}", p)
                                }
                            },
                        },
                    },
                }
            } else if p.val.is_float() {
                bytes.put_i8(IS_FLOAT);
                bytes.put_f64(p.val.as_float().unwrap());
            } else {
                unimplemented!("not implemented for {:?}", p);
            }
        }
    }
    bytes.freeze()
}

pub fn property_to_bytes(property: &Property, inc_head: bool) -> Bytes {
    let mut bytes = BytesMut::new();
    if inc_head {
        bytes.put_u8(PROPERTY);
    }
    bytes.put_u64(property.id);

    let val = property.stream.sender.borrow();
    match &*val {
        None => {
            bytes.put_i8(IS_NONE);
        }
        Some(p) => {
            if p.val.is_bool() {
                bytes.put_i8(IS_BOOL);
                bytes.put_u8(if p.val.as_bool().unwrap() { 1 } else { 0 });
            } else if p.val.is_str() {
                bytes.put_i8(IS_STR);
                let str_val = p.val.as_str().unwrap();
                let str_length: u8 = str_val.len() as u8;
                bytes.put_u8(str_length);
                bytes.put_slice(str_val.as_bytes())
            } else if p.val.is_integer() {
                let int = p.val.as_integer().unwrap();
                match int.to_i8() {
                    Some(i) => {
                        bytes.put_i8(IS_SHORT);
                        bytes.put_i8(i);
                    }
                    None => match int.to_i16() {
                        Some(i) => {
                            bytes.put_i8(IS_SMALL);
                            bytes.put_i16(i);
                        }
                        None => match int.to_i32() {
                            Some(i) => {
                                bytes.put_i8(IS_LONG);
                                bytes.put_i32(i);
                            }
                            None => match int.to_i64() {
                                Some(i) => {
                                    bytes.put_i8(IS_INT);
                                    bytes.put_i64(i);
                                }
                                None => {
                                    unimplemented!("You really shouldn't be here {:?}", p)
                                }
                            },
                        },
                    },
                }
            } else if p.val.is_float() {
                bytes.put_i8(IS_FLOAT);
                bytes.put_f64(p.val.as_float().unwrap());
            } else {
                unimplemented!("not implemented for {:?}", p);
            }
        }
    }
    bytes.freeze()
}

pub(crate) fn bytes_to_property(bytes: &mut Bytes) -> Option<Property> {
    let property_id = bytes.get_u64();
    debug!("id: {:?}", property_id);

    let value = bytes_to_property_value(bytes);
    match value {
        Some(pv) => Some(Property::from_value(&property_id, pv.val)),
        None => Some(Property::from_id(&property_id, None)),
    }
}

/// Decode just the type-tagged value portion of the Hive wire format.
///
/// This reads `[type_tag][value_bytes]` without the leading `u64` property
/// ID.  Used by the MQTT peer where the property identity is carried by the
/// MQTT topic rather than an in-band hash.
pub fn bytes_to_property_value(bytes: &mut Bytes) -> Option<PropertyValue> {
    let value_type = bytes.get_i8();
    match value_type {
        IS_STR => {
            let str_length = bytes.get_u8() as usize;
            let value = String::from_utf8(bytes.slice(..str_length).to_vec());
            bytes.advance(str_length);
            let v = value.ok().unwrap();
            Some(PropertyValue::from(v.as_str()))
        }
        IS_BOOL => {
            let b = bytes.get_u8() > 0;
            Some(PropertyValue::from(b))
        }
        IS_SHORT => {
            let short = bytes.get_i8();
            Some(PropertyValue::from(short as i32))
        }
        IS_SMALL => {
            let small = bytes.get_i16();
            Some(PropertyValue::from(small as i32))
        }
        IS_LONG => {
            let long = bytes.get_i32();
            Some(PropertyValue::from(long))
        }
        IS_INT => {
            let int = bytes.get_i64();
            Some(PropertyValue::from(int))
        }
        IS_FLOAT => {
            let float = bytes.get_f64();
            Some(PropertyValue::from(float))
        }
        IS_NONE => None,
        _ => {
            unimplemented!("bytes_to_property_value: unhandled type tag {:?}", value_type)
        }
    }
}
