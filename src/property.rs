
use crate::hive::{NAMES, PROPERTIES, PROPERTY};

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
use crate::logging::{debug, info};
use toml::Value;

use toml::value::Table;
use std::fmt::{Debug, Formatter, Display, write, Pointer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

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
/// String longer than 255 bytes: `u32` big-endian length, then the bytes.
/// Strings up to 255 bytes keep using `IS_STR` with a `u8` length, so
/// peers running an older build still interoperate for those.
pub(crate) const IS_LONG_STR: i8 = 0x22;

/// Error decoding a value or property off the wire. Always the sender's
/// fault (truncated frame, unknown tag, bad UTF-8), never ours, so callers
/// log it and drop the message rather than panic.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodeError(pub String);

impl Display for DecodeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "decode error: {}", self.0)
    }
}

impl std::error::Error for DecodeError {}

fn need(bytes: &Bytes, n: usize, what: &str) -> std::result::Result<(), DecodeError> {
    if bytes.remaining() < n {
        Err(DecodeError(format!("{} needs {} bytes, {} left", what, n, bytes.remaining())))
    } else {
        Ok(())
    }
}

/// Reactive stream for observing property value changes.
///
/// Backed by a `tokio::sync::watch` channel — a single current value shared
/// between one writer and many readers. Each clone gets an independent stream
/// cursor that yields only values changed *after* the clone was created.
///
/// The watch channel also serves as the canonical storage for the property's
/// current value, eliminating the need for a separate `Arc<RwLock<...>>`.
///
/// The `PropertyStream` inside a `Property` holds no receiver until it is
/// first polled; only clones (and polled originals) subscribe. While nobody
/// is subscribed `Property::set_value` stores the value without waking
/// anyone, which skips the watch channel's notify (eight mutexes) on every
/// update, the largest per-message cost before (todo.md 5.6).
pub struct PropertyStream {
    sender: Arc<watch::Sender<Option<PropertyValue>>>,
    stream: Option<WatchStream<Option<PropertyValue>>>,
}

impl Clone for PropertyStream {
    fn clone(&self) -> Self {
        // subscribe() creates a new independent receiver starting from the current value.
        // from_changes() skips the current value and only yields future updates,
        // matching the semantics of the previous broadcast-based implementation.
        PropertyStream {
            sender: self.sender.clone(),
            stream: Some(WatchStream::from_changes(self.sender.subscribe())),
        }
    }
}

impl Default for PropertyStream {
    fn default() -> Self {
        Self::new(None)
    }
}

impl PropertyStream {
    fn new(initial: Option<PropertyValue>) -> Self {
        PropertyStream {
            sender: Arc::new(watch::channel(initial).0),
            stream: None,
        }
    }
}

impl Stream for PropertyStream {
    type Item = PropertyValue;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // WatchStream yields Option<PropertyValue>.
        // We filter out None (property cleared/deleted) and only surface real values.
        // When the watch sender is dropped, the stream closes naturally.
        // A `Property`'s own stream subscribes when first polled, so it
        // yields changes from then on.
        let this = &mut *self;
        let stream = this
            .stream
            .get_or_insert_with(|| WatchStream::from_changes(this.sender.subscribe()));
        loop {
            match Pin::new(&mut *stream).poll_next(cx) {
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
        // Property IDs travel over the wire and must match across separate
        // processes (server and client), possibly on different CPU
        // architectures (a Mac-built GUI talking to an ARM-built client,
        // for instance), so this needs to be identical everywhere.
        //
        // Used to use ahash::RandomState::with_seeds for this. Fixed seeds
        // do solve ahash's *per-process* random seeding (AHasher::default()
        // reseeds randomly each run, which silently breaks cross-process
        // matching in a way same-process tests never catch, both sides
        // just happen to share that process's random seed). But ahash can
        // also take a different internal code path per target depending on
        // whether hardware AES instructions are available, which changes
        // its output even with identical seeds and identical input. That's
        // exactly what happened here: confirmed 2026-08-02 on real
        // hardware, a Mac-built GUI and an ARM-built motor_client computed
        // completely different ids for the same property names, silently
        // breaking on_next for every property, because the receiving side
        // never recognized an incoming update as one it already had a
        // callback registered for. Fixed seeds alone don't protect against
        // that, a genuinely portable algorithm does. FNV-1a has no hardware-
        // dependent code paths at all, pure integer arithmetic, so it's
        // guaranteed to produce the same output on any target.
        const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

        let mut hash = FNV_OFFSET_BASIS;
        for byte in val.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash
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
        // Compare and store under one lock. The watch channel is updated
        // straight away so get_value() and stream subscribers reflect the
        // latest state; an unchanged value notifies nobody. With no stream
        // subscribed the closure stores the value but reports "unchanged",
        // so the channel skips waking receivers that don't exist. A stream
        // cloned at the same moment as this update may not see it as new,
        // the same either-way outcome as cloning just after it.
        let mut changed = false;
        let sender = &self.stream.sender;
        sender.send_if_modified(|current| {
            if current.as_ref() == Some(&new_prop) {
                false
            } else {
                *current = Some(new_prop.clone());
                changed = true;
                sender.receiver_count() > 0
            }
        });

        if changed {
            debug!("emit change (backoff={})", self.backoff);

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
/// Hasher for keys that are already well-mixed 64-bit hashes (property ids
/// are FNV-1a of the name), so hashing them again with SipHash is wasted
/// work on every lookup.
#[derive(Default, Clone, Copy)]
pub struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        // Only u64 keys are expected; fold anything else in FNV style so
        // it still works if someone hashes another type with it.
        for b in bytes {
            self.0 = (self.0 ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
}

/// Property id to property. Same API as a plain `HashMap<u64, Property>`.
pub type PropertyMap = HashMap<u64, Property, std::hash::BuildHasherDefault<IdHasher>>;

pub(crate) fn properties_to_bytes(properties: &PropertyMap) -> Bytes {
    let mut bytes = BytesMut::new();
    bytes.put_u8(PROPERTIES);
    for p in properties {
        if p.1.stream.sender.borrow().is_some() {
            bytes.put_slice(&property_to_bytes(p.1, false));
        }
    }
    bytes.freeze()
}


/// Encode just the type-tagged value portion of the Hive wire format.
///
/// Writes `[type_tag][value_bytes]` without the leading `u64` property ID
/// or the `PROPERTY` header byte.  Used by the MQTT peer where the property
/// identity is carried by the MQTT topic.
pub fn property_value_to_bytes(property: &Property) -> Bytes {
    let val = property.stream.sender.borrow();
    let v = val.as_ref().map(|pv| &pv.val);
    let mut bytes = BytesMut::with_capacity(value_len_hint(v));
    put_value(&mut bytes, v);
    bytes.freeze()
}

pub fn property_to_bytes(property: &Property, inc_head: bool) -> Bytes {
    let val = property.stream.sender.borrow();
    encode_property(property.id, val.as_ref().map(|pv| &pv.val), inc_head)
}

/// Encode `[PROPERTY]?[u64 id][type_tag][value]` into one buffer sized up
/// front, without needing a `Property`. `None` encodes `IS_NONE`, which
/// receivers treat as a delete.
pub(crate) fn encode_property(id: u64, value: Option<&Value>, inc_head: bool) -> Bytes {
    let mut bytes = BytesMut::with_capacity(inc_head as usize + 8 + value_len_hint(value));
    if inc_head {
        bytes.put_u8(PROPERTY);
    }
    bytes.put_u64(id);
    put_value(&mut bytes, value);
    bytes.freeze()
}

fn value_len_hint(value: Option<&Value>) -> usize {
    match value {
        Some(Value::String(s)) => 5 + s.len(),
        _ => 9,
    }
}

/// Write `[type_tag][value]`, the smallest integer encoding that fits.
fn put_value(bytes: &mut BytesMut, value: Option<&Value>) {
    match value {
        None => bytes.put_i8(IS_NONE),
        Some(Value::Boolean(b)) => {
            bytes.put_i8(IS_BOOL);
            bytes.put_u8(*b as u8);
        }
        Some(Value::String(s)) => {
            match u8::try_from(s.len()) {
                Ok(len) => {
                    bytes.put_i8(IS_STR);
                    bytes.put_u8(len);
                }
                Err(_) => {
                    bytes.put_i8(IS_LONG_STR);
                    bytes.put_u32(s.len() as u32);
                }
            }
            bytes.put_slice(s.as_bytes());
        }
        Some(Value::Integer(int)) => {
            let int = *int;
            if let Ok(i) = i8::try_from(int) {
                bytes.put_i8(IS_SHORT);
                bytes.put_i8(i);
            } else if let Ok(i) = i16::try_from(int) {
                bytes.put_i8(IS_SMALL);
                bytes.put_i16(i);
            } else if let Ok(i) = i32::try_from(int) {
                bytes.put_i8(IS_LONG);
                bytes.put_i32(i);
            } else {
                bytes.put_i8(IS_INT);
                bytes.put_i64(int);
            }
        }
        Some(Value::Float(f)) => {
            bytes.put_i8(IS_FLOAT);
            bytes.put_f64(*f);
        }
        Some(other) => unimplemented!("no wire encoding for {:?}", other),
    }
}

/// Encode a NAMES message, `[NAMES]` then `[u64 id][u16 length][name]` for
/// each pair: what the properties with those ids are called. A name too
/// long for the length field is left out.
pub(crate) fn encode_names<'a>(names: impl IntoIterator<Item = (u64, &'a str)>) -> Bytes {
    let mut bytes = BytesMut::new();
    bytes.put_u8(NAMES);
    for (id, name) in names {
        if let Ok(len) = u16::try_from(name.len()) {
            bytes.put_u64(id);
            bytes.put_u16(len);
            bytes.put_slice(name.as_bytes());
        }
    }
    bytes.freeze()
}

/// Decode the body of a NAMES message. An id is the hash of its name, so a
/// pair that doesn't match is refused: a peer can't give a property a name
/// that isn't its own.
pub(crate) fn decode_names(bytes: &mut Bytes) -> std::result::Result<Vec<(u64, String)>, DecodeError> {
    let mut names = Vec::new();
    while bytes.has_remaining() {
        need(bytes, 10, "name id and length")?;
        let id = bytes.get_u64();
        let len = bytes.get_u16() as usize;
        need(bytes, len, "name")?;
        let raw = bytes.split_to(len);
        let name = std::str::from_utf8(&raw).map_err(|e| DecodeError(format!("property name not UTF-8: {}", e)))?;
        if Property::hash_id(name) != id {
            return Err(DecodeError(format!("{:?} is not the name of property {:#x}", name, id)));
        }
        names.push((id, name.to_string()));
    }
    Ok(names)
}

/// Decode `[u64 id][type_tag][value]` without building a `Property`.
pub(crate) fn decode_property(
    bytes: &mut Bytes,
) -> std::result::Result<(u64, Option<PropertyValue>), DecodeError> {
    need(bytes, 8, "property id")?;
    let property_id = bytes.get_u64();
    let value = try_bytes_to_property_value(bytes)?;
    Ok((property_id, value))
}

/// Decode just the type-tagged value portion of the Hive wire format.
///
/// Panics on malformed input; kept for API compatibility. Anything reading
/// bytes that came off a network should use `try_bytes_to_property_value`.
pub fn bytes_to_property_value(bytes: &mut Bytes) -> Option<PropertyValue> {
    try_bytes_to_property_value(bytes).expect("malformed property value")
}

/// Decode just the type-tagged value portion of the Hive wire format.
///
/// This reads `[type_tag][value_bytes]` without the leading `u64` property
/// ID.  Used by the MQTT peer where the property identity is carried by the
/// MQTT topic rather than an in-band hash. `Ok(None)` is a real `IS_NONE`
/// (property cleared); malformed input is an `Err`, never a panic.
pub fn try_bytes_to_property_value(
    bytes: &mut Bytes,
) -> std::result::Result<Option<PropertyValue>, DecodeError> {
    need(bytes, 1, "type tag")?;
    let value_type = bytes.get_i8();
    let v = match value_type {
        IS_STR | IS_LONG_STR => {
            let str_length = if value_type == IS_STR {
                need(bytes, 1, "string length")?;
                bytes.get_u8() as usize
            } else {
                need(bytes, 4, "long string length")?;
                bytes.get_u32() as usize
            };
            need(bytes, str_length, "string body")?;
            let raw = bytes.split_to(str_length);
            let s = std::str::from_utf8(&raw).map_err(|e| DecodeError(format!("string not UTF-8: {}", e)))?;
            PropertyValue::from(s)
        }
        IS_BOOL => {
            need(bytes, 1, "bool")?;
            PropertyValue::from(bytes.get_u8() > 0)
        }
        IS_SHORT => {
            need(bytes, 1, "i8")?;
            PropertyValue::from(bytes.get_i8() as i32)
        }
        IS_SMALL => {
            need(bytes, 2, "i16")?;
            PropertyValue::from(bytes.get_i16() as i32)
        }
        IS_LONG => {
            need(bytes, 4, "i32")?;
            PropertyValue::from(bytes.get_i32())
        }
        IS_INT => {
            need(bytes, 8, "i64")?;
            PropertyValue::from(bytes.get_i64())
        }
        IS_FLOAT => {
            need(bytes, 8, "f64")?;
            PropertyValue::from(bytes.get_f64())
        }
        IS_NONE => return Ok(None),
        other => return Err(DecodeError(format!("unknown type tag {:#04x}", other))),
    };
    Ok(Some(v))
}

/// Plain-text form of a value, for MQTT `payload = "text"`: `72`, `1.5`,
/// `true`, or the string itself. Floats always keep a decimal point (`2.0`,
/// not `2`) so a receiver inferring types reads them back as floats.
/// `None` is an empty payload, which a receiving hive ignores unless the
/// property is a string (see `text_to_value`).
pub fn value_to_text(value: Option<&Value>) -> Bytes {
    match value {
        None => Bytes::new(),
        Some(Value::String(s)) => Bytes::copy_from_slice(s.as_bytes()),
        Some(Value::Integer(i)) => Bytes::from(i.to_string()),
        Some(Value::Float(f)) => Bytes::from(format!("{:?}", f)),
        Some(Value::Boolean(b)) => Bytes::from_static(if *b { b"true" } else { b"false" }),
        Some(other) => Bytes::from(other.to_string()),
    }
}

/// Parse a plain-text payload into a value. `hint` is the property's
/// current value, if any: the text is read as that type when it can be
/// (so a string property holding "72" stays a string, and a float property
/// reads "72" as 72.0), otherwise the type is inferred as bool, integer,
/// float, then string. Booleans also accept on/off, yes/no and 1/0 when
/// the property is already a bool (Home Assistant style). Surrounding
/// whitespace is ignored for everything but strings. Non-UTF-8 is an error.
///
/// An empty (or all-whitespace) payload is `Ok(None)`, meaning "ignore it",
/// unless the property is a string, where it is the empty string. Empty
/// payloads are routine in MQTT (clearing a retained message), and reading
/// one as `""` would turn a number or bool property into a string.
pub fn text_to_value(text: &[u8], hint: Option<&Value>) -> std::result::Result<Option<Value>, DecodeError> {
    let raw = std::str::from_utf8(text).map_err(|e| DecodeError(format!("text payload not UTF-8: {}", e)))?;
    let t = raw.trim();
    if let Some(Value::String(_)) = hint {
        return Ok(Some(Value::String(raw.to_string())));
    }
    if t.is_empty() {
        return Ok(None);
    }
    Ok(Some(infer_text(raw, t, hint)))
}

fn infer_text(raw: &str, t: &str, hint: Option<&Value>) -> Value {
    let as_bool = |loose: bool| -> Option<bool> {
        match t.to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            "on" | "yes" | "1" if loose => Some(true),
            "off" | "no" | "0" if loose => Some(false),
            _ => None,
        }
    };
    // f64 parsing accepts "inf"/"nan"; only treat text with a digit as a
    // number so labels like "info" or "nan" stay strings.
    let as_float = || t.bytes().any(|b| b.is_ascii_digit()).then(|| t.parse::<f64>().ok()).flatten();

    match hint {
        Some(Value::Float(_)) => {
            if let Some(f) = as_float() {
                return Value::Float(f);
            }
        }
        Some(Value::Integer(_)) => {
            if let Ok(i) = t.parse::<i64>() {
                return Value::Integer(i);
            }
        }
        Some(Value::Boolean(_)) => {
            if let Some(b) = as_bool(true) {
                return Value::Boolean(b);
            }
        }
        _ => {}
    }
    if let Some(b) = as_bool(false) {
        return Value::Boolean(b);
    }
    if let Ok(i) = t.parse::<i64>() {
        return Value::Integer(i);
    }
    if let Some(f) = as_float() {
        return Value::Float(f);
    }
    Value::String(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_values_round_trip_by_inference() {
        for v in [Value::Integer(72), Value::Integer(-5), Value::Float(2.0), Value::Float(1.5),
                  Value::Boolean(true), Value::String("hello world".into())] {
            let text = value_to_text(Some(&v));
            assert_eq!(text_to_value(&text, None).unwrap(), Some(v.clone()), "{:?} via {:?}", v, text);
        }
        assert_eq!(&value_to_text(Some(&Value::Float(2.0)))[..], b"2.0");
    }

    #[test]
    fn text_parsing_follows_the_existing_type() {
        let s = Value::String("x".into());
        assert_eq!(text_to_value(b"72", Some(&s)).unwrap(), Some(Value::String("72".into())));
        assert_eq!(text_to_value(b" padded ", Some(&s)).unwrap(), Some(Value::String(" padded ".into())));
        assert_eq!(text_to_value(b"72", Some(&Value::Float(0.0))).unwrap(), Some(Value::Float(72.0)));
        assert_eq!(text_to_value(b"ON", Some(&Value::Boolean(false))).unwrap(), Some(Value::Boolean(true)));
        assert_eq!(text_to_value(b"0", Some(&Value::Boolean(true))).unwrap(), Some(Value::Boolean(false)));
        // Doesn't fit the hint: fall back to inference rather than drop it.
        assert_eq!(text_to_value(b"1.5", Some(&Value::Integer(0))).unwrap(), Some(Value::Float(1.5)));
        assert_eq!(text_to_value(b"72\n", None).unwrap(), Some(Value::Integer(72)));
    }

    #[test]
    fn text_inference_edge_cases() {
        assert_eq!(text_to_value(b"nan", None).unwrap(), Some(Value::String("nan".into())));
        assert_eq!(text_to_value(b"info", None).unwrap(), Some(Value::String("info".into())));
        assert_eq!(text_to_value(b"1e3", None).unwrap(), Some(Value::Float(1000.0)));
        assert_eq!(text_to_value(b"", None).unwrap(), None);
        // on/off only count as bools for a property that is already a bool.
        assert_eq!(text_to_value(b"on", None).unwrap(), Some(Value::String("on".into())));
        assert!(text_to_value(&[0xff, 0xfe], None).is_err());
    }

    #[test]
    fn empty_text_never_changes_a_non_string_type() {
        for hint in [Value::Integer(5), Value::Float(1.5), Value::Boolean(true)] {
            assert_eq!(text_to_value(b"", Some(&hint)).unwrap(), None, "{:?}", hint);
            assert_eq!(text_to_value(b" \n", Some(&hint)).unwrap(), None, "{:?}", hint);
        }
        let s = Value::String("x".into());
        assert_eq!(text_to_value(b"", Some(&s)).unwrap(), Some(Value::String("".into())));
    }

    #[tokio::test]
    async fn streams_subscribe_lazily_and_see_later_changes() {
        use futures::StreamExt;
        let mut p = Property::from_name("x", Some(1.into()));
        // Nobody subscribed: the value is still stored.
        assert!(p.set_value(2.into()));
        assert_eq!(p.get_value(), Some(Value::Integer(2)));
        assert!(!p.set_value(2.into()), "unchanged value reported as a change");

        let mut clone = p.stream.clone();
        assert!(p.set_value(3.into()));
        assert_eq!(clone.next().await, Some(PropertyValue::from(3)));

        // The property's own stream yields what changes after its first poll.
        let first = tokio::time::timeout(std::time::Duration::from_millis(20), p.stream.next()).await;
        assert!(first.is_err(), "nothing changed yet, got {:?}", first);
        assert!(p.set_value(4.into()));
        assert_eq!(p.stream.next().await, Some(PropertyValue::from(4)));
        assert_eq!(clone.next().await, Some(PropertyValue::from(4)));
    }

    fn round_trip(v: PropertyValue) -> Option<PropertyValue> {
        let p = Property::from_name("x", Some(v));
        let mut b = property_to_bytes(&p, false);
        let (id, val) = decode_property(&mut b).unwrap();
        assert_eq!(id, Property::hash_id("x"));
        assert!(b.is_empty(), "trailing bytes left after decode");
        val
    }

    #[test]
    fn short_and_long_strings_round_trip() {
        for n in [0usize, 1, 255, 256, 300, 70_000] {
            let s = "a".repeat(n);
            assert_eq!(round_trip(PropertyValue::from(s.as_str())), Some(PropertyValue::from(s.as_str())), "len {}", n);
        }
    }

    #[test]
    fn short_strings_keep_old_encoding() {
        let p = Property::from_name("x", Some("hi".into()));
        let b = property_value_to_bytes(&p);
        assert_eq!(&b[..], &[IS_STR as u8, 2, b'h', b'i']);
    }

    #[test]
    fn numbers_round_trip() {
        for v in [0i64, -1, 127, 128, -32768, 40_000, i32::MAX as i64 + 1, i64::MIN] {
            assert_eq!(round_trip(PropertyValue::from(v)), Some(PropertyValue::from(v)));
        }
        assert_eq!(round_trip(PropertyValue::from(1.5f64)), Some(PropertyValue::from(1.5f64)));
        assert_eq!(round_trip(PropertyValue::from(true)), Some(PropertyValue::from(true)));
    }

    #[test]
    fn names_round_trip_and_are_checked() {
        let long = "n".repeat(300);
        let pairs = [("light", Property::hash_id("light")), ("", Property::hash_id("")), (long.as_str(), Property::hash_id(&long))];
        let mut message = encode_names(pairs.iter().map(|(name, id)| (*id, *name)));
        assert_eq!(message.get_u8(), NAMES);
        let decoded = decode_names(&mut message).unwrap();
        assert_eq!(decoded.len(), 3);
        for ((name, id), (got_id, got_name)) in pairs.iter().zip(&decoded) {
            assert_eq!((id, *name), (got_id, got_name.as_str()));
        }

        // A name under another property's id, and input cut short.
        let mut wrong = encode_names([(Property::hash_id("door"), "light")]).slice(1..);
        assert!(decode_names(&mut wrong).is_err());
        let whole = encode_names([(Property::hash_id("light"), "light")]);
        for cut in 2..whole.len() {
            assert!(decode_names(&mut whole.slice(1..cut)).is_err(), "cut at {}", cut);
        }
        // One too long for its length field is left out, not truncated.
        let huge = "x".repeat(70_000);
        assert_eq!(encode_names([(Property::hash_id(&huge), huge.as_str())]).len(), 1);
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        let cases: &[&[u8]] = &[
            &[],                                 // no id
            &[0, 0, 0, 0, 0, 0, 0, 1],           // id, no tag
            &[0, 0, 0, 0, 0, 0, 0, 1, 0x7f],     // unknown tag
            &[0, 0, 0, 0, 0, 0, 0, 1, IS_STR as u8, 5, b'a'], // short body
            &[0, 0, 0, 0, 0, 0, 0, 1, IS_STR as u8, 2, 0xff, 0xfe], // bad utf-8
            &[0, 0, 0, 0, 0, 0, 0, 1, IS_FLOAT as u8, 1, 2], // short float
            &[0, 0, 0, 0, 0, 0, 0, 1, IS_LONG_STR as u8, 0xff, 0xff, 0xff, 0xff],
        ];
        for c in cases {
            let mut b = Bytes::copy_from_slice(c);
            assert!(decode_property(&mut b).is_err(), "{:?} should fail", c);
        }
    }
}
