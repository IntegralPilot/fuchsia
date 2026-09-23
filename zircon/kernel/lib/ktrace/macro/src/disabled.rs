// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// Resolves a string parameter (no-op when built with GCC).
#[macro_export]
macro_rules! resolve_string {
    ($string:ident) => {
        $string
    };
    ($string:literal) => {
        $string
    };
}

/// Resolves a category parameter (no-op when built with GCC).
#[macro_export]
macro_rules! resolve_category {
    ($category:ident) => {
        $category
    };
    ($category:literal) => {
        $category
    };
}

/// Instant event (no-op when built with GCC).
#[macro_export]
macro_rules! instant {
    ($category:tt, $label:tt, $context:expr $(, $key:tt => $val:expr)* $(,)?) => {};
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// CPU instant event (no-op when built with GCC).
#[macro_export]
macro_rules! cpu_instant {
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// Duration begin event (no-op when built with GCC).
#[macro_export]
macro_rules! duration_begin {
    ($category:tt, $label:tt, $context:expr $(, $key:tt => $val:expr)* $(,)?) => {};
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// CPU duration begin event (no-op when built with GCC).
#[macro_export]
macro_rules! cpu_duration_begin {
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// Duration end event (no-op when built with GCC).
#[macro_export]
macro_rules! duration_end {
    ($category:tt, $label:tt, $context:expr $(, $key:tt => $val:expr)* $(,)?) => {};
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// CPU duration end event (no-op when built with GCC).
#[macro_export]
macro_rules! cpu_duration_end {
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// Counter event (no-op when built with GCC).
#[macro_export]
macro_rules! counter {
    ($category:tt, $label:tt, $counter_id:expr $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// Flow begin event (no-op when built with GCC).
#[macro_export]
macro_rules! flow_begin {
    ($category:tt, $label:tt, $flow_id:expr $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// Flow step event (no-op when built with GCC).
#[macro_export]
macro_rules! flow_step {
    ($category:tt, $label:tt, $flow_id:expr $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// Flow end event (no-op when built with GCC).
#[macro_export]
macro_rules! flow_end {
    ($category:tt, $label:tt, $flow_id:expr $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// Begin scope (no-op when built with GCC).
#[macro_export]
macro_rules! begin_scope {
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// CPU begin scope (no-op when built with GCC).
#[macro_export]
macro_rules! cpu_begin_scope {
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// Conditional begin scope (no-op when built with GCC).
#[macro_export]
macro_rules! begin_scope_cond {
    ($cond:expr, $category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {
        Option::<()>::None
    };
}

/// Duration complete event (no-op when built with GCC).
#[macro_export]
macro_rules! complete {
    ($category:tt, $label:tt, $start_timestamp:expr, $context:expr $(, $key:tt => $val:expr)* $(,)?) => {};
    ($category:tt, $label:tt, $start_timestamp:expr $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// CPU duration complete event (no-op when built with GCC).
#[macro_export]
macro_rules! cpu_complete {
    ($category:tt, $label:tt, $start_timestamp:expr $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// Kernel object record (no-op when built with GCC).
#[macro_export]
macro_rules! kernel_object {
    ($category:tt, $koid:expr, $obj_type:expr, $name:expr $(, $key:tt => $val:expr)* $(,)?) => {};
}

/// Kernel object record unconditionally (no-op when built with GCC).
#[macro_export]
macro_rules! kernel_object_always {
    ($koid:expr, $obj_type:expr, $name:expr $(, $key:tt => $val:expr)* $(,)?) => {};
}
