//! Android's log (logcat). Standard output goes nowhere in an Android app,
//! so on Android `init_logging` writes here instead, under the tag `hive`.
//! Uses the system's own liblog, so no extra dependency.
#![cfg(target_os = "android")]

use std::ffi::CString;
use std::os::raw::{c_char, c_int};

#[link(name = "log")]
extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

// android/log.h priorities. Which ones get used depends on the logging
// backend Hive is built with.
#[allow(dead_code)]
pub(crate) const VERBOSE: c_int = 2;
#[allow(dead_code)]
pub(crate) const DEBUG: c_int = 3;
#[allow(dead_code)]
pub(crate) const INFO: c_int = 4;
#[allow(dead_code)]
pub(crate) const WARN: c_int = 5;
pub(crate) const ERROR: c_int = 6;

const TAG: &[u8] = b"hive\0";

/// One log line. logcat adds the time, priority and tag itself.
pub(crate) fn write(priority: c_int, text: &str) {
    // A C string ends at its first NUL; don't let one cut the line short.
    let text = CString::new(text.trim_end().replace('\0', " ")).unwrap_or_default();
    unsafe {
        __android_log_write(priority, TAG.as_ptr() as *const c_char, text.as_ptr());
    }
}

/// Panic messages go to standard error, which is lost too. Log them, then
/// carry on to whatever hook was there before.
pub(crate) fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(ERROR, &format!("panic: {}", info));
        previous(info);
    }));
}

/// Hands tracing's formatted events to logcat, at the event's own level.
#[cfg(all(feature = "tracing", not(feature = "log")))]
pub(crate) struct Logcat;

#[cfg(all(feature = "tracing", not(feature = "log")))]
pub(crate) struct Line(c_int);

#[cfg(all(feature = "tracing", not(feature = "log")))]
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logcat {
    type Writer = Line;

    fn make_writer(&'a self) -> Line {
        Line(INFO)
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Line {
        let level = *meta.level();
        Line(if level == tracing::Level::ERROR {
            ERROR
        } else if level == tracing::Level::WARN {
            WARN
        } else if level == tracing::Level::INFO {
            INFO
        } else if level == tracing::Level::DEBUG {
            DEBUG
        } else {
            VERBOSE
        })
    }
}

#[cfg(all(feature = "tracing", not(feature = "log")))]
impl std::io::Write for Line {
    // The fmt layer writes each event whole, in one call.
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        write(self.0, &String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
