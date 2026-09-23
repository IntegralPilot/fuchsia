// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Types encoding trace point identifiers.

use netstack3_sync::rc::ResourceToken;

/// A resource identifier that can be used as an argument for trace events.
pub struct TraceResourceId<'a> {
    #[cfg_attr(not(target_os = "fuchsia"), expect(unused))]
    token: ResourceToken<'a>,
}

impl<'a> TraceResourceId<'a> {
    /// Creates a new resource id with the given value.
    pub fn new(token: ResourceToken<'a>) -> Self {
        Self { token }
    }
}

#[cfg(target_os = "fuchsia")]
impl<'a> fuchsia_trace::ArgValue for TraceResourceId<'a> {
    fn of<'x>(key: &'x str, value: Self) -> fuchsia_trace::Arg<'x>
    where
        Self: 'x,
    {
        let Self { token } = value;
        fuchsia_trace::ArgValue::of(key, token.export_value())
    }
    fn of_registered<'x>(
        name_ref: fuchsia_trace::trace_string_ref_t,
        value: Self,
    ) -> fuchsia_trace::Arg<'x>
    where
        Self: 'x,
    {
        let Self { token } = value;
        fuchsia_trace::ArgValue::of_registered(name_ref, token.export_value())
    }
}
