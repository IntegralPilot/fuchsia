// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Library for constructing the Component Id Index.

mod index;

pub use component_id_index::Index;
pub use index::ComponentIdIndexBuilder;
