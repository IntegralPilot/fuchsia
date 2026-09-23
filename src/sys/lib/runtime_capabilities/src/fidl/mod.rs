// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod capability;
mod connector;
mod connector_router;
mod data;
mod data_router;
pub(crate) mod dictionary;
mod dictionary_router;
pub(crate) mod dir_connector;
mod dir_connector_router;
mod handle;
mod instance_token;
pub(crate) mod receiver;
pub(crate) mod registry;
pub(crate) mod router;
pub(crate) mod store;

use crate::WeakInstanceToken;
use fidl_fuchsia_component_sandbox as fsandbox;
use std::sync::Arc;

pub trait IntoFsandboxCapability {
    fn into_fsandbox_capability(self, _token: Arc<WeakInstanceToken>) -> fsandbox::Capability
    where
        Self: Sized;
}
