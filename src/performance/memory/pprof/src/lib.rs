// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;

// Re-export the raw protobuf types.
pub use pprof_proto::perfetto::third_party::perftools::profiles::*;

mod module_map;
pub use module_map::{ModuleMapBuilder, ModuleMapResolver};

mod string_table;
pub use string_table::StringTableBuilder;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Attempted to add a new mapping that would overlap one or more existing mappings")]
    MappingWouldOverlap,
}
