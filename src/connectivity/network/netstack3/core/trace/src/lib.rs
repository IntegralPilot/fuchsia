// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tracing abstractions for Netstack3.
//!
//! This crate provides a tracing frontend that can be compiled with and without
//! fuchsia dependencies for netstack3-core. It encodes the common patterns and
//! trace categories used by netstack3.

#![no_std]
#![warn(
    missing_docs,
    unreachable_patterns,
    clippy::useless_conversion,
    clippy::redundant_clone,
    clippy::precedence
)]

mod id;

/// The trace category used by netstack3.
pub const CATEGORY: &'static str = "net";

/// Internal implementation.
#[cfg(target_os = "fuchsia")]
pub mod __inner {
    use super::CATEGORY;

    pub use fuchsia_trace::{ArgValue, AsTraceStrRef, Scope, duration, instant};
    use fuchsia_trace::{TraceCategoryContext, trace_site_t};

    /// A single trace site cache that is used in the macro expansions.
    ///
    /// Given this crate always forces the same category [`crate::CATEGORY`] for
    /// trace points, we can have a single entry instead of one per call-site.
    static CACHE: trace_site_t = trace_site_t::new(0);

    /// Checks for [`CACHE`] to see if the trace category is enabled.
    #[inline]
    pub fn category_context() -> Option<TraceCategoryContext> {
        TraceCategoryContext::acquire_cached(CATEGORY, &CACHE)
    }

    /// Equivalent of [`fuchsia_trace::duration!`].
    ///
    /// Always uses [`crate::CATEGORY`].
    #[macro_export]
    macro_rules! trace_duration {
        ($name:expr $(, $key:expr => $val:expr)* $(,)?) => {
            let mut args;
            let _scope = {
                if let Some(context) = $crate::__inner::category_context() {
                    use $crate::__inner::AsTraceStrRef as _;

                    args = [$($crate::__inner::ArgValue::of_registered(
                        $key.as_trace_str_ref(&context),
                        $val,
                    )),*];

                    Some($crate::__inner::duration($crate::CATEGORY, $name, &args))
                } else {
                    None
                }
            };
        }
    }

    /// Equivalent of [`fuchsia_trace::instant!`].
    ///
    /// Always uses [`crate::CATEGORY`].
    #[macro_export]
    macro_rules! trace_instant {
        ($name:expr $(, $key:expr => $val:expr)* $(,)?) => {
            if let Some(context) = $crate::__inner::category_context() {
                use $crate::__inner::AsTraceStrRef as _;

                let args = [$($crate::__inner::ArgValue::of_registered(
                    $key.as_trace_str_ref(&context),
                    $val,
                )),*];

                $crate::__inner::instant(
                    &context,
                    $name,
                    $crate::__inner::Scope::Thread,
                    &args
                );
            }
        }
    }
}

/// Internal implementation.
#[cfg(not(target_os = "fuchsia"))]
#[allow(missing_docs)]
pub mod __inner {
    #[macro_export]
    macro_rules! trace_duration {
        ($name:expr $(, $key:expr => $val:expr)* $(,)?) => {
            $(
                let _ = $key;
                let _ = $val;
            )*
        }
    }

    #[macro_export]
    macro_rules! trace_instant {
        ($name:expr $(, $key:expr => $val:expr)* $(,)?) => {
            $(
                let _ = $key;
                let _ = $val;
            )*
        }
    }
}

pub use id::TraceResourceId;
