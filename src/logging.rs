//! Picks which crate Hive's log macros go through, chosen at build time:
//!
//! | features            | macros used | where output goes                    |
//! |---------------------|-------------|--------------------------------------|
//! | `tracing` (default) | `tracing`   | the app's tracing subscriber         |
//! | `log`               | `log`       | the app's `log` logger               |
//! | both                | `log`       | the app's `log` logger               |
//! | neither             | none        | compiled out, zero cost              |
//!
//! `log` wins when both are on, so a crate that wants standard `log` output
//! only has to ask for `features = ["log"]`, whether or not it also turns
//! off Hive's default features. Cargo unions features across a dependency
//! graph, and the `log` feature also turns on tracing's own `log` feature,
//! so any tracing events in the same build (from dependencies, tests)
//! reach the `log` logger too when no tracing subscriber is installed.
//!
//! Modules import from here instead of naming a crate:
//! `use crate::logging::{debug, info};`

#[cfg(all(feature = "tracing", not(feature = "log")))]
#[allow(unused_imports)]
pub(crate) use tracing::{debug, error, info, trace, warn};

#[cfg(feature = "log")]
#[allow(unused_imports)]
pub(crate) use log::{debug, error, info, trace, warn};

/// Logging compiled out. Still type-checks the format arguments (and so
/// counts them as used, no unused-variable warnings), inside an `if false`
/// the optimizer removes entirely.
#[cfg(not(any(feature = "log", feature = "tracing")))]
macro_rules! hive_log_noop {
    ($($arg:tt)*) => {{
        if false {
            let _ = ::std::format!($($arg)*);
        }
    }};
}

#[cfg(not(any(feature = "log", feature = "tracing")))]
#[allow(unused_imports)]
pub(crate) use {
    hive_log_noop as debug, hive_log_noop as error, hive_log_noop as info, hive_log_noop as trace,
    hive_log_noop as warn,
};
