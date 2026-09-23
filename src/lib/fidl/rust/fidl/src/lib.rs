// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Library and runtime for fidl bindings.

#![warn(clippy::all)]
#![deny(missing_docs)]

#[macro_use]
pub mod encoding;

pub mod client;
pub mod epitaph;
pub mod for_fidl_message_crate;
pub mod handle;
pub mod marker;
pub mod prelude;
pub mod server;

pub mod endpoints;
pub use endpoints::MethodType;

mod persistence;
pub use persistence::*;

mod serializable;
pub use serializable::*;

mod error;
pub use self::error::{Epitaph, Error, Result, TransportError};

pub use handle::*;
pub use server::ServeInner;

mod time;
pub use time::*;
