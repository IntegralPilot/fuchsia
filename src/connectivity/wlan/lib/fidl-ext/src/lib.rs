// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod responder_ext;
pub mod send_result_ext;
pub mod try_unpack;

pub use crate::responder_ext::ResponderExt;
pub use crate::send_result_ext::SendResultExt;
pub use crate::try_unpack::{TryUnpack, WithName};

#[doc(hidden)]
pub use paste::paste as __paste;

#[cfg(test)]
mod tests;
