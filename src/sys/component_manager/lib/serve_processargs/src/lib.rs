// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fuchsia_runtime::{HandleInfo, HandleType};
use runtime_capabilities::{Capability, DictKey};
use std::collections::HashMap;
use thiserror::Error;

mod namespace;

pub use crate::namespace::{BuildNamespaceError, NamespaceBuilder, ignore_not_found};

/// How to deliver a particular capability from a dict to an Elf process. Broadly speaking,
/// one could either deliver a capability using namespace entries, or using numbered handles.
pub enum Delivery {
    /// Install the capability as a `fuchsia.io` object, within some parent directory serviced by
    /// the framework, and discoverable at a path such as "/svc/foo/bar".
    ///
    /// As a result, a namespace entry will be created in the resulting processargs, corresponding
    /// to the parent directory, e.g. "/svc/foo".
    ///
    /// For example, installing a `runtime_capabilities::Connector` at "/svc/fuchsia.examples.Echo"
    /// will cause the framework to spin up a `fuchsia.io/Directory` implementation backing "/svc",
    /// containing a filesystem object named "fuchsia.examples.Echo".
    ///
    /// Not all capability types are installable as `fuchsia.io` objects. A one-shot handle is not
    /// supported because `fuchsia.io` does not have a protocol for delivering one-shot handles.
    /// Use [Delivery::Handle] for those.
    NamespacedObject(cm_types::Path),

    /// Install the capability as a `fuchsia.io` object by creating a namespace entry at the
    /// provided path. The difference between [Delivery::NamespacedObject] and
    /// [Delivery::NamespaceEntry] is that the former will create a namespace entry at the parent
    /// directory.
    ///
    /// For example, installing a `runtime_capabilities::Directory` at "/data" will result in a namespace entry
    /// at "/data". A request will be sent to the capability when the user writes to the
    /// namespace entry.
    NamespaceEntry(cm_types::Path),

    /// Installs the Zircon handle representation of this capability at the processargs slot
    /// described by [HandleInfo].
    ///
    /// The following handle types are disallowed because they will collide with the implementation
    /// of incoming namespace and outgoing directory:
    ///
    /// - [HandleType::NamespaceDirectory]
    /// - [HandleType::DirectoryRequest]
    ///
    Handle(HandleInfo),
}

pub enum DeliveryMapEntry {
    Delivery(Delivery),
    Dict(DeliveryMap),
}

/// A nested dictionary mapping capability names to delivery method.
///
/// Each entry in a [Dict] should have a corresponding entry here describing how the
/// capability will be delivered to the process. If a [Dict] has a nested [Dict], then there
/// will be a corresponding nested [DeliveryMapEntry::Dict] containing the [DeliveryMap] for the
/// capabilities in the nested [Dict].
pub type DeliveryMap = HashMap<DictKey, DeliveryMapEntry>;

#[derive(Error, Debug)]
pub enum DeliveryError {
    #[error("the key `{0}` is not found in the dict")]
    NotInDict(DictKey),

    #[error("wrong type: the delivery map expected `{0}` to be a nested Dict in the dict")]
    NotADict(DictKey),

    #[error("unused capabilities in dict: `{0:?}`")]
    UnusedCapabilities(Vec<DictKey>),

    #[error("handle type `{0:?}` is not allowed to be installed into processargs")]
    UnsupportedHandleType(HandleType),

    #[error("namespace configuration error: `{0}`")]
    NamespaceError(namespace::BuildNamespaceError),

    #[error("capability `{0:?}` is not allowed to be installed into processargs")]
    UnsupportedCapability(Capability),
}
