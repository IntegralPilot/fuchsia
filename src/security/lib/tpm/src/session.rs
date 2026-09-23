// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use tpm2_tss_sys as tss_sys;

/// Strong typing for a SessionHandle which is just a subtype of `ESYS_TR`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SessionHandle(tss_sys::ESYS_TR);

impl From<SessionHandle> for tss_sys::ESYS_TR {
    fn from(item: SessionHandle) -> tss_sys::ESYS_TR {
        item.0
    }
}

impl From<tss_sys::ESYS_TR> for SessionHandle {
    fn from(item: tss_sys::ESYS_TR) -> SessionHandle {
        SessionHandle(item)
    }
}
