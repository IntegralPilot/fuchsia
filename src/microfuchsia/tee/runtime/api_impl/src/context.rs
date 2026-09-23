// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cell::RefCell;
use std::ops::Range;

use crate::crypto::Operations;
use crate::props::Properties;
use crate::storage::Storage;

// Context stores information about the current state of the TA runtime during a
// call into the TA. API entry points are expected to retrieve the current context
// and provide the relevant portions to implementation entry points.
pub struct Context {
    pub properties: Properties,
    pub storage: Storage,
    pub operations: Operations,
    pub mapped_param_ranges: Vec<Range<usize>>,
}

// The TA entry points are FFI calls that are expected to call back into the
// runtime via the TEE_* entry points and so the control flow is unusual
// compared to regular Rust code. Logically the context is part of the state
// used by the runtime entry points. However, instead of carrying context
// through parameters on the stack as purely Rust code would do we have to store
// the context somewhere else and recover it from the FFI entry points. Instead,
// the state is stored in a thread local.
thread_local! {
    static CURRENT_CONTEXT: RefCell<Context> = RefCell::new(Context::new());
}

pub fn with_current<F, R>(f: F) -> R
where
    F: FnOnce(&Context) -> R,
{
    CURRENT_CONTEXT.with_borrow(f)
}

pub fn with_current_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut Context) -> R,
{
    CURRENT_CONTEXT.with_borrow_mut(f)
}

impl Context {
    pub fn new() -> Self {
        Self {
            properties: Properties::new(),
            storage: Storage::new(),
            operations: Operations::new(),
            mapped_param_ranges: vec![],
        }
    }

    pub fn cleanup_after_call(&mut self) {
        self.mapped_param_ranges.clear();
    }
}
