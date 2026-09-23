// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The `sapphire-gatt` crate, providing Bluetooth GATT and ATT implementations.

#![cfg_attr(not(test), no_std)]
#![allow(async_fn_in_trait)]

pub mod att;
pub mod gatt;
