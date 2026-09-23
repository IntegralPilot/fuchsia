// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod allocated;
mod inline;

#[cfg(feature = "std")]
pub use allocated::Global;
pub use inline::{ArrayStorage, InlineStorage, InlineStorageHandle};
