// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Next-generation FIDL Rust bindings library.

#![deny(
    future_incompatible,
    missing_docs,
    nonstandard_style,
    unused,
    warnings,
    clippy::all,
    clippy::alloc_instead_of_core,
    clippy::missing_safety_doc,
    clippy::std_instead_of_core,
    clippy::undocumented_unsafe_blocks,
    rustdoc::broken_intra_doc_links,
    rustdoc::missing_crate_level_docs
)]
#![forbid(unsafe_op_in_unsafe_fn)]

pub use ::fidl_next_bind::*;
pub use ::fidl_next_codec::*;
pub use ::fidl_next_protocol::{
    self as protocol, ClientHandler, Flexible, FrameworkError, Message, ProtocolError,
    ServerHandler, Strict, Transport,
};
pub use fidl_next_util as util;

/// FIDL wire type definitions and implementations.
pub mod wire {
    pub use ::fidl_next_codec::wire::*;
    pub use ::fidl_next_protocol::wire::*;
}

/// Fuchsia-specific FIDL extensions.
#[cfg(target_os = "fuchsia")]
pub mod fuchsia {
    pub use ::fidl_next_bind::fuchsia::*;
    pub use ::fidl_next_codec::fuchsia::*;
    pub use ::fidl_next_protocol::fuchsia::*;
}
