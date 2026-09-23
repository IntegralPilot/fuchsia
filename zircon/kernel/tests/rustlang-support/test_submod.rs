// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// The super (test_mod.rs) has a `pub mod test_dep_pub_mod;` via GN.
pub use super::test_dep_pub_mod::test_fn;
