// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Experimental features.
//!
//! This module hosts experimental APIs, most notably round-robin multi-resolution time series.

mod vec1;

pub mod clock;
pub mod inspect;
pub mod series;
pub mod testing;

pub use crate::experimental::vec1::Vec1;

pub mod prelude {
    pub use crate::experimental::clock::{DurationExt as _, QuantaExt as _, TimestampExt as _};
    pub use crate::experimental::series::{TimeMatrixFold, TimeMatrixTick};
}
