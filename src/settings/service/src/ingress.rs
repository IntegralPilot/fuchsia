// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This mod defines the building blocks for receiving inbound communication from external
//! interfaces, such as FIDL. It also includes common implementations for working with
//! `Jobs` for incoming requests.

/// The [fidl] mod enables defining components that provide inbound communication over FIDL.
pub mod fidl;
