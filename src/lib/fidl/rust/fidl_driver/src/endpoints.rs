// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::marker::PhantomData;

use fdf::Channel;

#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct DriverClientEnd<T>(pub(crate) Option<Channel<u8>>, pub(crate) PhantomData<T>);

/// A marker for a particular FIDL protocol.
///
/// Implementations of this trait can be used to manufacture instances of a FIDL
/// protocol and get metadata about a particular protocol.
pub trait DriverProtocolMarker: Sized + Send + Sync + 'static {
    /// The name of the protocol suitable for debug purposes.
    ///
    /// For discoverable protocols, this should be identical to
    /// `<Self as DiscoverableProtocolMarker>::PROTOCOL_NAME`.
    const DEBUG_NAME: &'static str;

    // TODO(364653685): This will contain associated types for proxies and servers.
}
