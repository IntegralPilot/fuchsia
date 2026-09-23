// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

//! # Diagnostics log types serde
//!
//! Serializers and deserializers for types used in various places for logging.
//!
//! Intended as utilities for used with serde as:
//!
//! ```
//! use diagnostics_log_types_serde::{optinoal_severity, Severity};
//!
//! #[derive(Serialize, Deserialize)]
//! struct SomeType {
//!     #[serde(default, with = "optional_severity")]
//!     severity: Option<Severity>
//! }
//! ```

mod serde_ext;

pub use diagnostics_log_types::Severity;
pub use serde_ext::*;
