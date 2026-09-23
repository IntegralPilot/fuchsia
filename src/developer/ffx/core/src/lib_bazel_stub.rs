// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This is a minimal stub of ffx_core used exclusively for Bazel builds.
// It only re-exports the `ffx_command` proc macro required by FFX plugin args.
//
// This allows the Bazel migration of plugins before Rust FIDL is supported, which
// is used in lib.rs.
//
// TODO(https://fxbug.dev/512640761): delete this file once ffx-core is migrated to Bazel.
pub use core_macros::ffx_command;
