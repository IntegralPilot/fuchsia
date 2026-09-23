// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/439053417): Investigate why the recursion limit was bumped
// to unblock the toolchain.
#![recursion_limit = "256"]

pub use fxfs_platform_testing::fuchsia::testing::*;
