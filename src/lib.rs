// use std::fs;
#![warn(rust_2018_idioms)]

use std::ffi::CStr;
#[cfg(any(feature = "tracing", feature = "log"))]
use chrono::Local;
pub use tokio_util::sync::CancellationToken;
pub use futures;
use std::os::raw::c_char;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use crate::hive::Hive;

#[cfg(feature = "gui")]
pub mod hive_gui;
mod hive_macros;
mod logging;
#[cfg(target_os = "android")]
mod logcat;
mod security;
mod config;
pub mod property;
pub mod signal;
pub mod hive;
pub mod peer;
pub mod handler;
pub mod embed;

// mod backoff; // unused — property debounce is handled in Property::set_value

#[cfg(feature = "websock")]
pub mod websocket;

#[cfg(feature = "bluetooth")]
pub mod bluetooth;

#[cfg(feature = "mqtt")]
pub mod mqtt;


#[cfg(feature="bluetooth")]
#[macro_use]
extern crate lazy_static;

#[cfg(any(target_os = "android", feature = "jni"))]
mod jni_api;

// INIT LOGGING

/// Log level for `init_logging`. Same variant names as `log::LevelFilter`,
/// so `init_logging(Some(LevelFilter::Debug))` reads the same whichever
/// logging backend Hive was built with (see the `logging` module).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LevelFilter {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[cfg(feature = "tracing")]
impl From<LevelFilter> for tracing::level_filters::LevelFilter {
    fn from(l: LevelFilter) -> Self {
        use tracing::level_filters::LevelFilter as T;
        match l {
            LevelFilter::Off => T::OFF,
            LevelFilter::Error => T::ERROR,
            LevelFilter::Warn => T::WARN,
            LevelFilter::Info => T::INFO,
            LevelFilter::Debug => T::DEBUG,
            LevelFilter::Trace => T::TRACE,
        }
    }
}

#[cfg(feature = "log")]
impl From<LevelFilter> for log::LevelFilter {
    fn from(l: LevelFilter) -> Self {
        match l {
            LevelFilter::Off => log::LevelFilter::Off,
            LevelFilter::Error => log::LevelFilter::Error,
            LevelFilter::Warn => log::LevelFilter::Warn,
            LevelFilter::Info => log::LevelFilter::Info,
            LevelFilter::Debug => log::LevelFilter::Debug,
            LevelFilter::Trace => log::LevelFilter::Trace,
        }
    }
}

/// Minute, second and microsecond.
#[cfg(any(feature = "tracing", feature = "log"))]
#[cfg_attr(target_os = "android", allow(dead_code))]
fn short_time() -> impl std::fmt::Display {
    Local::now().format("%M:%S.%6f")
}

#[cfg(all(feature = "tracing", not(feature = "log")))]
#[cfg_attr(target_os = "android", allow(dead_code))]
struct ShortTime;

#[cfg(all(feature = "tracing", not(feature = "log")))]
impl tracing_subscriber::fmt::time::FormatTime for ShortTime {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        write!(w, "{}", short_time())
    }
}

/// Minimal stdout `log` logger, used by `init_logging` in `log` builds.
#[cfg(feature = "log")]
struct StdoutLogger;

#[cfg(feature = "log")]
impl log::Log for StdoutLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        #[cfg(target_os = "android")]
        if self.enabled(record.metadata()) {
            let priority = match record.level() {
                log::Level::Error => logcat::ERROR,
                log::Level::Warn => logcat::WARN,
                log::Level::Info => logcat::INFO,
                log::Level::Debug => logcat::DEBUG,
                log::Level::Trace => logcat::VERBOSE,
            };
            let line = format!("{}:{}: {}", record.file().unwrap_or("?"), record.line().unwrap_or(0), record.args());
            logcat::write(priority, &line);
        }
        #[cfg(not(target_os = "android"))]
        if self.enabled(record.metadata()) {
            println!(
                "{} {:>5} {}:{}: {}",
                short_time(),
                record.level(),
                record.file().unwrap_or("?"),
                record.line().unwrap_or(0),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

/// Install a simple stdout logger for whichever backend Hive was built
/// with. Safe to call more than once (tests sharing a process do); only the
/// first call takes effect. Apps that set up their own logger or
/// subscriber shouldn't call this at all, Hive's output goes to theirs.
///
/// On Android, where standard output is discarded, it logs to logcat
/// instead, under the tag `hive` and at each message's own priority, and
/// sends panic messages there as well.
///
/// - `tracing` (default): installs a tracing-subscriber fmt subscriber.
///   With `mqtt` or `gui`, dependencies that log through `log` are bridged
///   into it.
/// - `log` (with or without `tracing`): installs a `log` logger.
/// - neither: does nothing, logging is compiled out.
pub fn init_logging(level: Option<LevelFilter>) {
    // Info by default: at Debug/Trace every message on the hot path logs
    // several lines.
    #[allow(unused_variables)]
    let level = level.unwrap_or(LevelFilter::Info);

    #[cfg(target_os = "android")]
    {
        static PANICS: std::sync::Once = std::sync::Once::new();
        PANICS.call_once(logcat::log_panics);
    }

    #[cfg(all(feature = "tracing", not(feature = "log")))]
    {
        let builder = tracing_subscriber::fmt()
            .with_max_level(tracing::level_filters::LevelFilter::from(level))
            .with_target(false)
            .with_file(true)
            .with_line_number(true)
            .with_ansi(false);
        // logcat stamps every line with the time and priority itself.
        #[cfg(target_os = "android")]
        let _ = builder.without_time().with_level(false).with_writer(logcat::Logcat).try_init();
        #[cfg(not(target_os = "android"))]
        let _ = builder.with_timer(ShortTime).try_init();
    }

    #[cfg(feature = "log")]
    {
        static LOGGER: StdoutLogger = StdoutLogger;
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(level.into());
        }
    }
}




#[no_mangle]
pub unsafe extern "C" fn newHive(props: *const c_char) -> Hive {
    let c_str = CStr::from_ptr(props);
    let prop_str_pointer = c_str.to_str().unwrap_or_else(|_| "you");

    Hive::new_from_str("Hive1", prop_str_pointer )
}

// fn get_toml_config(file_path: &str) -> toml::Value{
//     let foo: String = fs::read_to_string("examples/properties.toml").unwrap().parse().unwrap();
//     return toml::from_str(&foo).unwrap();
// }

pub fn panic_after<T, F>(d: Duration, f: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T,
    F: Send + 'static,
{
    let (done_tx, done_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let val = f();
        done_tx.send(()).expect("Unable to send completion signal");
        val
    });

    match done_rx.recv_timeout(d) {
        Ok(_) => handle.join().expect("Thread panicked"),
        Err(_) => panic!("Thread took too long"),
    }
}