// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Component sandbox traits and capability types.

mod capability;
mod connector;
mod data;
mod dictionary;
mod dir_connector;
mod handle;
mod instance_token;
mod receiver;
mod router;

#[cfg(target_os = "fuchsia")]
pub mod fidl;

pub use self::capability::{Capability, CapabilityBound, ConversionError, RemoteError};
pub use self::connector::{Connectable, Connector};
pub use self::data::Data;
pub use self::dictionary::{
    Dictionary, EntryUpdate, Key as DictKey, UpdateNotifierFn, UpdateNotifierRetention,
};
pub use self::dir_connector::{DirConnectable, DirConnector, DirConnectorMessage};
pub use self::handle::Handle;
pub use self::instance_token::{WeakInstanceToken, WeakInstanceTokenAny};
pub use self::receiver::{DirReceiver, Receiver};
pub use self::router::{Routable, Router, RouterErrorInfo};

#[cfg(target_os = "fuchsia")]
pub use self::fidl::store::serve_capability_store;
