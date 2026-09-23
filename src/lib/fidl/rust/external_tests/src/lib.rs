// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

mod data_types;
#[cfg(target_os = "fuchsia")]
mod handle_rights;
mod message;
mod persistence;
#[cfg(target_os = "fuchsia")]
mod unknown_interactions;
