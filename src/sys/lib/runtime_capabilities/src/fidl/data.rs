// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Data, WeakInstanceToken};
use fidl_fuchsia_component_sandbox as fsandbox;
use std::sync::Arc;

impl crate::fidl::IntoFsandboxCapability for crate::Data {
    fn into_fsandbox_capability(self, _token: Arc<WeakInstanceToken>) -> fsandbox::Capability {
        fsandbox::Capability::Data(self.to_fsandbox())
    }
}

impl Data {
    pub(crate) fn to_fsandbox(&self) -> fsandbox::Data {
        match self {
            Data::Bytes(bytes) => fsandbox::Data::Bytes(bytes.to_vec()),
            Data::String(string) => fsandbox::Data::String(string.to_string()),
            Data::Int64(num) => fsandbox::Data::Int64(*num),
            Data::Uint64(num) => fsandbox::Data::Uint64(*num),
        }
    }
}
