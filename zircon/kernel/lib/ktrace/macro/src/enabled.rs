// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// Resolves a string parameter to a reference to an `InternedString`.
/// If a string literal is provided, it is statically interned at compile-time.
#[macro_export]
macro_rules! resolve_string {
    ($string:ident) => {
        $string
    };
    ($string:literal) => {{
        #[unsafe(link_section = "__fxt_interned_string_table")]
        #[used]
        static STRING: crate::ktrace_rs::InternedString =
            unsafe { crate::ktrace_rs::InternedString::new_raw(concat!($string, "\0").as_ptr()) };
        &STRING
    }};
    ($string:expr) => {
        $string
    };
}

/// Resolves a category parameter to a reference to an `InternedCategory`.
/// If a string literal is provided, it is declared as an external category.
#[macro_export]
macro_rules! resolve_category {
    ($category:ident) => {
        $category
    };
    ($category:literal) => {{
        crate::ktrace_rs::declare_interned_category!(CATEGORY, $category, extern);
        CATEGORY
    }};
}

/// Writes an instant event associated with the current thread when the given category is enabled.
///
/// # Arguments:
/// - category: Filter category for the event. Expects a string literal or expression.
/// - label: Label for the event. Expects a string literal or expression.
/// - ...: List of key => value argument pairs.
#[macro_export]
macro_rules! instant {
    ($category:tt, $label:tt, $context:expr $(, $key:tt => $val:expr)* $(,)?) => {
        {
            let category = $crate::resolve_category!($category);
            let ktrace = crate::ktrace_rs::KTrace::get_instance();
            if ktrace.is_category_enabled(category) {
                ktrace.emit_event(
                    crate::ktrace_rs::EventType::Instant,
                    category,
                    $crate::resolve_string!($label),
                    crate::ktrace_rs::timer_current_boot_ticks(),
                    $context,
                    None,
                    &[
                        $(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*
                    ],
                );
            }
        }
    };
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {
        $crate::instant!($category, $label, crate::ktrace_rs::Context::Thread $(, $key => $val)*)
    };
}

/// Similar to `instant!`, but associates the event with the current CPU instead of the current
/// thread.
#[macro_export]
macro_rules! cpu_instant {
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {
        $crate::instant!($category, $label, crate::ktrace_rs::Context::Cpu $(, $key => $val)*)
    };
}

/// Writes a duration begin event associated with the current thread when the given category is
/// enabled.
///
/// # Arguments:
/// - category: Filter category for the event. Expects a string literal or expression.
/// - label: Label for the event. Expects a string literal or expression.
/// - ...: List of key => value argument pairs.
#[macro_export]
macro_rules! duration_begin {
    ($category:tt, $label:tt, $context:expr $(, $key:tt => $val:expr)* $(,)?) => {
        {
            let category = $crate::resolve_category!($category);
            let ktrace = crate::ktrace_rs::KTrace::get_instance();
            if ktrace.is_category_enabled(category) {
                ktrace.emit_event(
                    crate::ktrace_rs::EventType::DurationBegin,
                    category,
                    $crate::resolve_string!($label),
                    crate::ktrace_rs::timer_current_boot_ticks(),
                    $context,
                    None,
                    &[
                        $(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*
                    ],
                );
            }
        }
    };
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {
        $crate::duration_begin!($category, $label, crate::ktrace_rs::Context::Thread $(, $key => $val)*)
    };
}

/// Similar to `duration_begin!`, but associates the event with the current CPU instead of the
/// current thread.
#[macro_export]
macro_rules! cpu_duration_begin {
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {
        $crate::duration_begin!($category, $label, crate::ktrace_rs::Context::Cpu $(, $key => $val)*)
    };
}

/// Writes a duration end event associated with the current thread when the given category is
/// enabled.
///
/// # Arguments:
/// - category: Filter category for the event. Expects a string literal or expression.
/// - label: Label for the event. Expects a string literal or expression.
/// - ...: List of key => value argument pairs.
#[macro_export]
macro_rules! duration_end {
    ($category:tt, $label:tt, $context:expr $(, $key:tt => $val:expr)* $(,)?) => {
        {
            let category = $crate::resolve_category!($category);
            let ktrace = crate::ktrace_rs::KTrace::get_instance();
            if ktrace.is_category_enabled(category) {
                ktrace.emit_event(
                    crate::ktrace_rs::EventType::DurationEnd,
                    category,
                    $crate::resolve_string!($label),
                    crate::ktrace_rs::timer_current_boot_ticks(),
                    $context,
                    None,
                    &[
                        $(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*
                    ],
                );
            }
        }
    };
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {
        $crate::duration_end!($category, $label, crate::ktrace_rs::Context::Thread $(, $key => $val)*)
    };
}

/// Similar to `duration_end!`, but associates the event with the current CPU instead of the
/// current thread.
#[macro_export]
macro_rules! cpu_duration_end {
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {
        $crate::duration_end!($category, $label, crate::ktrace_rs::Context::Cpu $(, $key => $val)*)
    };
}

/// Writes a counter event associated with the current thread when the given category is enabled.
///
/// Each argument is rendered as a separate value series named "<label>:<arg name>:<counter_id>".
///
/// # Arguments:
/// - category: Filter category for the event. Expects a string literal or expression.
/// - label: Label for the event. Expects a string literal or expression.
/// - counter_id: Correlation id for the event. Must be convertible to u64.
/// - ...: List of key => value argument pairs.
#[macro_export]
macro_rules! counter {
    ($category:tt, $label:tt, $counter_id:expr $(, $key:tt => $val:expr)* $(,)?) => {
        {
            let category = $crate::resolve_category!($category);
            let ktrace = crate::ktrace_rs::KTrace::get_instance();
            if ktrace.is_category_enabled(category) {
                ktrace.emit_event(
                    crate::ktrace_rs::EventType::Counter,
                    category,
                    $crate::resolve_string!($label),
                    crate::ktrace_rs::timer_current_boot_ticks(),
                    crate::ktrace_rs::Context::Thread,
                    Some($counter_id as u64),
                    &[
                        $(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*
                    ],
                );
            }
        }
    };
}

/// Writes a flow begin event associated with the current thread when the given category is enabled.
///
/// # Arguments:
/// - category: Filter category for the event. Expects a string literal or expression.
/// - label: Label for the event. Expects a string literal or expression.
/// - flow_id: Flow id for the event. Must be convertible to u64.
/// - ...: List of key => value argument pairs.
#[macro_export]
macro_rules! flow_begin {
    ($category:tt, $label:tt, $flow_id:expr $(, $key:tt => $val:expr)* $(,)?) => {
        {
            let category = $crate::resolve_category!($category);
            let ktrace = crate::ktrace_rs::KTrace::get_instance();
            if ktrace.is_category_enabled(category) {
                ktrace.emit_event(
                    crate::ktrace_rs::EventType::FlowBegin,
                    category,
                    $crate::resolve_string!($label),
                    crate::ktrace_rs::timer_current_boot_ticks(),
                    crate::ktrace_rs::Context::Thread,
                    Some($flow_id as u64),
                    &[
                        $(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*
                    ],
                );
            }
        }
    };
}

/// Writes a flow step event associated with the current thread when the given category is enabled.
///
/// # Arguments:
/// - category: Filter category for the event. Expects a string literal or expression.
/// - label: Label for the event. Expects a string literal or expression.
/// - flow_id: Flow id for the event. Must be convertible to u64.
/// - ...: List of key => value argument pairs.
#[macro_export]
macro_rules! flow_step {
    ($category:tt, $label:tt, $flow_id:expr $(, $key:tt => $val:expr)* $(,)?) => {
        {
            let category = $crate::resolve_category!($category);
            let ktrace = crate::ktrace_rs::KTrace::get_instance();
            if ktrace.is_category_enabled(category) {
                ktrace.emit_event(
                    crate::ktrace_rs::EventType::FlowStep,
                    category,
                    $crate::resolve_string!($label),
                    crate::ktrace_rs::timer_current_boot_ticks(),
                    crate::ktrace_rs::Context::Thread,
                    Some($flow_id as u64),
                    &[
                        $(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*
                    ],
                );
            }
        }
    };
}

/// Writes a flow end event associated with the current thread when the given category is enabled.
///
/// # Arguments:
/// - category: Filter category for the event. Expects a string literal or expression.
/// - label: Label for the event. Expects a string literal or expression.
/// - flow_id: Flow id for the event. Must be convertible to u64.
/// - ...: List of key => value argument pairs.
#[macro_export]
macro_rules! flow_end {
    ($category:tt, $label:tt, $flow_id:expr $(, $key:tt => $val:expr)* $(,)?) => {
        {
            let category = $crate::resolve_category!($category);
            let ktrace = crate::ktrace_rs::KTrace::get_instance();
            if ktrace.is_category_enabled(category) {
                ktrace.emit_event(
                    crate::ktrace_rs::EventType::FlowEnd,
                    category,
                    $crate::resolve_string!($label),
                    crate::ktrace_rs::timer_current_boot_ticks(),
                    crate::ktrace_rs::Context::Thread,
                    Some($flow_id as u64),
                    &[
                        $(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*
                    ],
                );
            }
        }
    };
}

/// Creates a delegate to capture the given arguments at the beginning of a scope when the given
/// category is enabled. The returned value should be used to construct a `ktrace::Scope` to track
/// the lifetime of the scope and emit the complete trace event. The complete event is associated
/// with the current thread.
///
/// # Arguments:
/// - category: Filter category for the event. Expects a string literal or expression.
/// - label: Label for the event. Expects a string literal or expression.
/// - ...: List of key => value argument pairs.
#[macro_export]
macro_rules! begin_scope {
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {
        crate::ktrace_rs::KTraceScope::begin(
            $crate::resolve_category!($category),
            $crate::resolve_string!($label),
            crate::ktrace_rs::Context::Thread,
            [$(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*],
        )
    };
}

/// Similar to `begin_scope!`, but associates the event with the current CPU instead of the
/// current thread.
#[macro_export]
macro_rules! cpu_begin_scope {
    ($category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {
        crate::ktrace_rs::KTraceScope::begin(
            $crate::resolve_category!($category),
            $crate::resolve_string!($label),
            crate::ktrace_rs::Context::Cpu,
            [$(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*],
        )
    };
}

/// Similar to `begin_scope!`, but checks the given runtime_condition, in addition to the given
/// category, to determine whether to emit the event.
#[macro_export]
macro_rules! begin_scope_cond {
    ($cond:expr, $category:tt, $label:tt $(, $key:tt => $val:expr)* $(,)?) => {
        {
            let category = $crate::resolve_category!($category);
            let ktrace = crate::ktrace_rs::KTrace::get_instance();
            if $cond && ktrace.is_category_enabled(category) {
                Some(crate::ktrace_rs::KTraceScope::begin(
                    category,
                    $crate::resolve_string!($label),
                    crate::ktrace_rs::Context::Thread,
                    [$(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*],
                ))
            } else {
                None
            }
        }
    };
}

/// Writes a duration complete event associated with the current thread when the given category is
/// enabled.
///
/// # Arguments:
/// - category: Filter category for the event. Expects a string literal or expression.
/// - label: Label for the event. Expects a string literal or expression.
/// - start_timestamp: The starting timestamp for the event. Must be convertible to i64.
/// - ...: List of key => value argument pairs.
#[macro_export]
macro_rules! complete {
    ($category:tt, $label:tt, $start_timestamp:expr, $context:expr $(, $key:tt => $val:expr)* $(,)?) => {
        {
            let category = $crate::resolve_category!($category);
            let ktrace = crate::ktrace_rs::KTrace::get_instance();
            if ktrace.is_category_enabled(category) {
                ktrace.emit_event(
                    crate::ktrace_rs::EventType::DurationComplete,
                    category,
                    $crate::resolve_string!($label),
                    $start_timestamp,
                    $context,
                    Some(crate::ktrace_rs::timer_current_boot_ticks().0 as u64),
                    &[
                        $(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*
                    ],
                );
            }
        }
    };
    ($category:tt, $label:tt, $start_timestamp:expr $(, $key:tt => $val:expr)* $(,)?) => {
        $crate::complete!($category, $label, $start_timestamp, crate::ktrace_rs::Context::Thread $(, $key => $val)*)
    };
}

/// Similar to `complete!`, but associates the event with the current CPU instead of the current
/// thread.
#[macro_export]
macro_rules! cpu_complete {
    ($category:tt, $label:tt, $start_timestamp:expr $(, $key:tt => $val:expr)* $(,)?) => {
        $crate::complete!($category, $label, $start_timestamp, crate::ktrace_rs::Context::Cpu $(, $key => $val)*)
    };
}

/// Writes a kernel object record when the given trace category is enabled.
///
/// # Arguments:
/// - category: Filter category for the object record. Expects a string literal or expression.
/// - koid: Kernel object id of the object. Expects type u64.
/// - obj_type: The type the object. Expects type u32.
/// - name: The name of the object. Expects a string literal or expression.
/// - ...: List of key => value argument pairs.
#[macro_export]
macro_rules! kernel_object {
    ($category:tt, $koid:expr, $obj_type:expr, $name:expr $(, $key:tt => $val:expr)* $(,)?) => {
        {
            let category = $crate::resolve_category!($category);
            let ktrace = crate::ktrace_rs::KTrace::get_instance();
            if ktrace.is_category_enabled(category) {
                ktrace.emit_kernel_object_outlined(
                    $koid as u64,
                    $obj_type as u32,
                    $crate::resolve_string!($name),
                    &[
                        $(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*
                    ],
                );
            }
        }
    };
}

/// Writes a kernel object record unconditionally. Useful for generating the initial set of object
/// info records before tracing is enabled.
///
/// # Arguments:
/// - koid: Kernel object id of the object. Expects type u64.
/// - obj_type: The type the object. Expects type u32.
/// - name: The name of the object. Expects a string literal or expression.
/// - ...: List of key => value argument pairs.
#[macro_export]
macro_rules! kernel_object_always {
    ($koid:expr, $obj_type:expr, $name:expr $(, $key:tt => $val:expr)* $(,)?) => {
        {
            let ktrace = crate::ktrace_rs::KTrace::get_instance();
            ktrace.emit_kernel_object_outlined(
                $koid as u64,
                $obj_type as u32,
                $crate::resolve_string!($name),
                &[
                    $(crate::ktrace_rs::Argument::new($crate::resolve_string!($key), $val)),*
                ],
            );
        }
    };
}
