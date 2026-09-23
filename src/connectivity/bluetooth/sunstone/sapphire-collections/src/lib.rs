// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg_attr(not(feature = "std"), no_std)]

pub mod deque;
pub mod map;
pub mod storage;
pub mod vec;

/// Represents an error when memory allocation fails.
#[derive(Debug)]
pub struct AllocError;
