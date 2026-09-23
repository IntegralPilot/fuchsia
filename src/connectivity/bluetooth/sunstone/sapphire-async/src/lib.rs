// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(unused_crate_dependencies)]
#![cfg_attr(not(feature = "std"), no_std)]

pub mod broadcast;
pub mod condition;
pub mod executor;
pub mod mpsc;
pub mod mutex;
pub mod notification;
pub mod rpc;
pub mod semaphore;

pub(crate) mod global_index;

#[cfg(feature = "testing")]
pub mod testing;
