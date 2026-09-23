// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FIDL wire type definitions and implementations.

mod boxed;
mod envelope;
#[cfg(feature = "fuchsia")]
pub mod fuchsia;
mod primitives;
mod ptr;
mod result;
mod string;
mod table;
mod union;
mod unit;
mod vec;

pub use self::boxed::*;
pub use self::envelope::*;
pub use self::primitives::*;
pub use self::ptr::*;
pub use self::result::*;
pub use self::string::*;
pub use self::table::*;
pub use self::union::*;
pub use self::unit::*;
pub use self::vec::*;
