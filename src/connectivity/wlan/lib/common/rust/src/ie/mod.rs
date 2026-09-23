// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod fake_ies;
pub mod intersect;
pub mod owe_transition;
pub mod rsn;
pub mod wpa;
pub mod wsc;

mod constants;
mod fields;
mod id;
mod merger;
mod parse;
mod rates_writer;
mod reader;
mod write;

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

pub use constants::*;
pub use fake_ies::*;
pub use fields::*;
pub use id::*;
pub use intersect::*;
pub use merger::*;
pub use parse::*;
pub use rates_writer::*;
pub use reader::*;
pub use write::*;

#[repr(C, packed)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned)]
pub struct Header {
    pub id: Id,
    pub body_len: u8,
}
