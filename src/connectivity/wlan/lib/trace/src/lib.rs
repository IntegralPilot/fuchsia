// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod names;

#[doc(hidden)]
pub use fuchsia_trace as __trace;

use fuchsia_trace as trace;

/// Writes a duration begin event immediately and writes a duration end event when the current scope
/// exits. As with all wlan-trace macros, the category will be "wlan" by default.
#[macro_export]
macro_rules! duration {
    ($name:expr $(, $key:expr => $val:expr)* $(,)?) => {
        $crate::__trace::duration_begin!($crate::names::CATEGORY_WLAN, $name $(, $key => $val)* );
        struct DurationEnd;
        impl Drop for DurationEnd {
            fn drop(&mut self) {
                $crate::__trace::duration_end!($crate::names::CATEGORY_WLAN, $name);
            }
        }
        let _scope = DurationEnd;
    };
}

#[macro_export]
macro_rules! duration_begin {
    ($name:expr $(, $key:expr => $val:expr)* $(,)?) => {
        $crate::__trace::duration_begin!($crate::names::CATEGORY_WLAN, $name $(, $key => $val)* );
    };
}

#[macro_export]
macro_rules! duration_end {
    ($name:expr $(, $key:expr => $val:expr)* $(,)?) => {
        $crate::__trace::duration_end!($crate::names::CATEGORY_WLAN, $name $(, $key => $val)* );
    };
}

pub fn instant_wlancfg_start() {
    if let Some(context) = trace::TraceCategoryContext::acquire(names::CATEGORY_WLAN) {
        trace::instant(&context, names::NAME_WLANCFG_START, trace::Scope::Process, &[]);
    }
}

pub fn async_begin_wlansoftmac_tx(async_id: trace::Id, origin: &str) {
    trace::async_begin(
        async_id,
        names::CATEGORY_WLAN,
        names::NAME_WLANSOFTMAC_TX,
        &[trace::ArgValue::of("origin", origin)],
    );
}

pub fn async_end_wlansoftmac_tx(async_id: trace::Id, status: zx::Status) {
    trace::async_end(
        async_id,
        names::CATEGORY_WLAN,
        names::NAME_WLANSOFTMAC_TX,
        &[trace::ArgValue::of("status", format!("{}", status).as_str())],
    );
}

pub fn async_begin_wlansoftmac_rx(async_id: trace::Id) {
    trace::async_begin(async_id, names::CATEGORY_WLAN, names::NAME_WLANSOFTMAC_RX, &[]);
}

pub fn async_instant_wlansoftmac_rx(async_id: trace::Id, status: &str) {
    if let Some(context) = trace::TraceCategoryContext::acquire(names::CATEGORY_WLAN) {
        trace::async_instant(
            async_id,
            &context,
            names::NAME_WLANSOFTMAC_RX,
            &[trace::ArgValue::of("status", status)],
        );
    }
}

pub fn async_end_wlansoftmac_rx(async_id: trace::Id, status: &str) {
    trace::async_end(
        async_id,
        names::CATEGORY_WLAN,
        names::NAME_WLANSOFTMAC_RX,
        &[trace::ArgValue::of("status", status)],
    );
}
