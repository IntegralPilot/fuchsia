// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::prelude_internal::*;
use openthread_sys::otChangedFlags;

unsafe extern "C" {
    /// Handle OT state change in platform radio
    pub fn platformRadioHandleStateChange(instance: *mut otsys::otInstance, flags: otChangedFlags);
}
