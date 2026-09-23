// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Typed wrappers for FIDL bindings.
//!
//! This crate wraps a number of "untyped" items to add more type safety, and provides some basic
//! [utility methods for use on Fuchsia](fuchsia).

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
    // TODO: re-enable this lint after justifying unsafe blocks
    // clippy::undocumented_unsafe_blocks,
    rustdoc::broken_intra_doc_links,
    rustdoc::missing_crate_level_docs
)]
#![forbid(unsafe_op_in_unsafe_fn)]

mod client;
#[cfg(feature = "compat")]
mod compat;
mod decoded;
mod endpoint;
mod error;
mod executor;
#[cfg(feature = "fuchsia")]
pub mod fuchsia;
#[cfg(feature = "fasync")]
pub mod fuchsia_async;
mod future;
mod protocol;
mod server;
mod service;

pub use self::client::*;
pub use self::decoded::*;
pub use self::endpoint::*;
pub use self::error::*;
pub use self::executor::*;
pub use self::future::*;
pub use self::protocol::*;
pub use self::server::*;
pub use self::service::*;
