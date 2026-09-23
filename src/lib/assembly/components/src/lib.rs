// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Library for compiling and merging components from cml with the cm tool.
mod components;

pub use components::ComponentBuilder;
