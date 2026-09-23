// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// A set of helper functions for performing filesystem operations on images.
// These are both exposed as tools themselves and used as part of golden image generation.
pub mod ops;

// A set of helper functions for performing golden image generation and validation.
mod golden_common;
pub mod golden_generate;

// The implementation of the FUSE prototype to mount Fxfs in Linux.
#[cfg(target_os = "linux")]
pub mod fuse_attr;

#[cfg(target_os = "linux")]
pub mod fuse_errors;

#[cfg(target_os = "linux")]
pub mod fuse_fs;

#[cfg(target_os = "linux")]
pub mod fuse_vfs;
