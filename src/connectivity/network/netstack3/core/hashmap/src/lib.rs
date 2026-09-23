// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Re-export HashMap and HashSet symbols used by netstack3 core.
//!
//! This crate exists because the netstack3-core wants to be `no_std` to ensure
//! we're not taking on big OS dependencies in case it's moved to run elsewhere.
//! However, `HashMap` and `HashSet` are *NOT* available in the `alloc` crate.
//!
//! Given we currently only target Fuchsia, it's unnecessary to do anything more
//! complicated here (like using `hashbrown`) and simply having this one-off
//! std-allowed crate is much simpler to deal with.
//!
//! This crate may evolve if necessary to provide different HashMap
//! implementations based on compilation features as necessary.

pub use ::std::collections::{HashMap, HashSet, hash_map, hash_set};
