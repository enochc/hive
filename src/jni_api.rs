//! JNI bindings over `embed::EmbeddedHive`, for Android and any other JVM.
//! Always built for Android targets; the `jni` feature builds them for
//! other hosts (a desktop JVM, for tests).
//!
//! The Kotlin half is `bindings/kotlin/com/enochc/hive/NativeHive.kt`. The
//! two are one unit: the function names below encode that class's package
//! and name, and the callbacks are looked up by the method names and
//! signatures its `Callbacks` class has.
//!
//! A running hive is known to the JVM by a handle, a key into `HIVES`
//! rather than a pointer, so a handle used after `nativeStop` (or one that
//! never existed) finds nothing instead of freed memory.

use crate::embed::{property_id, EmbeddedHive, Event, Value};
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jdouble, jint, jlong, JNI_FALSE, JNI_TRUE};
use jni::{JNIEnv, JavaVM};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

fn hives() -> &'static Mutex<HashMap<jlong, Arc<EmbeddedHive>>> {
    static HIVES: OnceLock<Mutex<HashMap<jlong, Arc<EmbeddedHive>>>> = OnceLock::new();
    HIVES.get_or_init(Default::default)
}

fn hive(handle: jlong) -> Option<Arc<EmbeddedHive>> {
    hives().lock().unwrap().get(&handle).cloned()
}

/// Run `f`, turning a panic into a Java `RuntimeException`: unwinding out
/// of an `extern` function would abort the whole app.
fn guarded<T>(env: &mut JNIEnv<'_>, fallback: T, f: impl FnOnce(&mut JNIEnv<'_>) -> T) -> T {
    match catch_unwind(AssertUnwindSafe(|| f(&mut *env))) {
        Ok(value) => value,
        Err(_) => {
            let _ = env.throw_new("java/lang/RuntimeException", "hive panicked, see stderr / logcat");
            fallback
        }
    }
}

/// A Java string as Rust, `None` for `null`.
fn text(env: &mut JNIEnv<'_>, s: &JString<'_>) -> Option<String> {
    if s.as_raw().is_null() {
        return None;
    }
    env.get_string(s).ok().map(Into::into)
}

/// The id to act on: `name`'s when there is one (remembering the name, so
/// events for the property carry it), otherwise `id` as given.
fn target(env: &mut JNIEnv<'_>, hive: &EmbeddedHive, id: jlong, name: &JString<'_>) -> u64 {
    match text(env, name) {
        Some(name) => hive.register_name(&name),
        None => id as u64,
    }
}

/// The JVM object events are delivered to, from the hive's events thread.
struct Callbacks {
    vm: JavaVM,
    target: GlobalRef,
}

impl Callbacks {
    fn deliver(&self, event: Event) {
        // Attaches the events thread the first time through; it detaches
        // itself when it exits.
        let Ok(mut env) = self.vm.attach_current_thread_as_daemon() else {
            return;
        };
        let refs = match &event {
            Event::Peers(peers) => 8 + 2 * peers.len() as i32,
            _ => 8,
        };
        // This thread never returns to Java, so nothing else would free
        // the local references each event creates.
        let sent = env.with_local_frame(refs, |env| self.call(env, &event));
        if sent.is_err() {
            // An exception thrown by the callback. Nothing above us to
            // hand it to; log it and carry on with the next event.
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
    }

    fn call(&self, env: &mut JNIEnv<'_>, event: &Event) -> jni::errors::Result<()> {
        match event {
            Event::Property { id, name, value } => {
                let name = match name {
                    Some(name) => JObject::from(env.new_string(name)?),
                    None => JObject::null(),
                };
                let value = match value {
                    Some(value) => to_java(env, value)?,
                    None => JObject::null(),
                };
                env.call_method(
                    &self.target,
                    "onProperty",
                    "(JLjava/lang/String;Ljava/lang/Object;)V",
                    &[JValue::Long(*id as jlong), JValue::Object(&name), JValue::Object(&value)],
                )?;
            }
            Event::PropertyNamed { id, name } => {
                let name = env.new_string(name)?;
                env.call_method(
                    &self.target,
                    "onPropertyName",
                    "(JLjava/lang/String;)V",
                    &[JValue::Long(*id as jlong), JValue::Object(&name)],
                )?;
            }
            Event::Peers(peers) => {
                let names = env.new_object_array(peers.len() as i32, "java/lang/String", JObject::null())?;
                let addresses = env.new_object_array(peers.len() as i32, "java/lang/String", JObject::null())?;
                for (i, peer) in peers.iter().enumerate() {
                    let name = env.new_string(&peer.name)?;
                    env.set_object_array_element(&names, i as i32, name)?;
                    let address = env.new_string(&peer.address)?;
                    env.set_object_array_element(&addresses, i as i32, address)?;
                }
                env.call_method(
                    &self.target,
                    "onPeers",
                    "([Ljava/lang/String;[Ljava/lang/String;)V",
                    &[JValue::Object(&names), JValue::Object(&addresses)],
                )?;
            }
            Event::PeerMessage(message) => {
                let message = env.new_string(message)?;
                env.call_method(&self.target, "onPeerMessage", "(Ljava/lang/String;)V", &[JValue::Object(&message)])?;
            }
            Event::Connection { connected, peers } => {
                env.call_method(
                    &self.target,
                    "onConnection",
                    "(ZI)V",
                    &[JValue::Bool(*connected as jboolean), JValue::Int(*peers as jint)],
                )?;
            }
        }
        Ok(())
    }
}

/// A property value as the Java object Kotlin sees: `String`, `Boolean`,
/// `Long` or `Double`.
fn to_java<'local>(env: &mut JNIEnv<'local>, value: &Value) -> jni::errors::Result<JObject<'local>> {
    match value {
        Value::String(s) => Ok(env.new_string(s)?.into()),
        Value::Boolean(b) => env
            .call_static_method("java/lang/Boolean", "valueOf", "(Z)Ljava/lang/Boolean;", &[JValue::Bool(*b as jboolean)])?
            .l(),
        Value::Integer(i) => env
            .call_static_method("java/lang/Long", "valueOf", "(J)Ljava/lang/Long;", &[JValue::Long(*i)])?
            .l(),
        Value::Float(f) => env
            .call_static_method("java/lang/Double", "valueOf", "(D)Ljava/lang/Double;", &[JValue::Double(*f)])?
            .l(),
        // Hive has no wire encoding for anything else, so none arrives.
        other => Ok(env.new_string(other.to_string())?.into()),
    }
}

/// Start a hive from a TOML config. Returns its handle, or throws
/// `IllegalArgumentException` with the reason the config was refused.
#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeStart<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    config: JString<'local>,
    callbacks: JObject<'local>,
) -> jlong {
    guarded(&mut env, 0, |env| {
        let started = (|| {
            let config = text(env, &config).ok_or("config is null")?;
            let callbacks = Callbacks {
                vm: env.get_java_vm().map_err(|e| e.to_string())?,
                target: env.new_global_ref(&callbacks).map_err(|e| e.to_string())?,
            };
            EmbeddedHive::start(&config, move |event| callbacks.deliver(event))
        })();
        match started {
            Ok(hive) => {
                static NEXT: AtomicI64 = AtomicI64::new(1);
                let handle = NEXT.fetch_add(1, Ordering::Relaxed);
                hives().lock().unwrap().insert(handle, Arc::new(hive));
                handle
            }
            Err(reason) => {
                let _ = env.throw_new("java/lang/IllegalArgumentException", reason);
                0
            }
        }
    })
}

/// Stop a hive and forget its handle. Events already queued are delivered
/// before this returns, unless it is called from a callback.
#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeStop(mut env: JNIEnv<'_>, _class: JClass<'_>, handle: jlong) {
    guarded(&mut env, (), |_| {
        // Taken out first: `stop` waits for the events thread, which may
        // be in a callback that is using the map.
        let hive = hives().lock().unwrap().remove(&handle);
        if let Some(hive) = hive {
            hive.stop();
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeSetString<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    id: jlong,
    name: JString<'local>,
    value: JString<'local>,
) {
    guarded(&mut env, (), |env| {
        if let (Some(hive), Some(value)) = (hive(handle), text(env, &value)) {
            let id = target(env, &hive, id, &name);
            hive.set_property_by_id(id, value.as_str());
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeSetBoolean<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    id: jlong,
    name: JString<'local>,
    value: jboolean,
) {
    guarded(&mut env, (), |env| {
        if let Some(hive) = hive(handle) {
            let id = target(env, &hive, id, &name);
            hive.set_property_by_id(id, value != JNI_FALSE);
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeSetLong<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    id: jlong,
    name: JString<'local>,
    value: jlong,
) {
    guarded(&mut env, (), |env| {
        if let Some(hive) = hive(handle) {
            let id = target(env, &hive, id, &name);
            hive.set_property_by_id(id, value);
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeSetDouble<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    id: jlong,
    name: JString<'local>,
    value: jdouble,
) {
    guarded(&mut env, (), |env| {
        if let Some(hive) = hive(handle) {
            let id = target(env, &hive, id, &name);
            hive.set_property_by_id(id, value);
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeDelete<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    id: jlong,
    name: JString<'local>,
) {
    guarded(&mut env, (), |env| {
        if let Some(hive) = hive(handle) {
            let id = target(env, &hive, id, &name);
            hive.delete_property_by_id(id);
        }
    })
}

/// Remember what a property is called. Returns its id.
#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeRegisterName<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    name: JString<'local>,
) -> jlong {
    guarded(&mut env, 0, |env| {
        let Some(name) = text(env, &name) else { return 0 };
        match hive(handle) {
            Some(hive) => hive.register_name(&name) as jlong,
            None => property_id(&name) as jlong,
        }
    })
}

/// False if the hive has stopped; says nothing about delivery.
#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeSendToPeer<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    peer: JString<'local>,
    message: JString<'local>,
) -> jboolean {
    guarded(&mut env, JNI_FALSE, |env| {
        let (Some(hive), Some(peer), Some(message)) = (hive(handle), text(env, &peer), text(env, &message)) else {
            return JNI_FALSE;
        };
        match hive.send_to_peer(&peer, &message) {
            Ok(()) => JNI_TRUE,
            Err(_) => JNI_FALSE,
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeRequestPeers(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) {
    guarded(&mut env, (), |_| {
        if let Some(hive) = hive(handle) {
            hive.request_peers();
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeRequestNames(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) {
    guarded(&mut env, (), |_| {
        if let Some(hive) = hive(handle) {
            hive.request_names();
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativeIsConnected(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jboolean {
    guarded(&mut env, JNI_FALSE, |_| match hive(handle) {
        Some(hive) if hive.is_connected() => JNI_TRUE,
        _ => JNI_FALSE,
    })
}

#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativePeerCount(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jint {
    guarded(&mut env, 0, |_| hive(handle).map_or(0, |hive| hive.peer_count() as jint))
}

/// The id a property name has on the wire, as a (possibly negative) long.
#[no_mangle]
pub extern "system" fn Java_com_enochc_hive_NativeHive_nativePropertyId<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
) -> jlong {
    guarded(&mut env, 0, |env| text(env, &name).map_or(0, |name| property_id(&name) as jlong))
}
