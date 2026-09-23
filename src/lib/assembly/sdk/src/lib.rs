// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! A ToolProvider that returns tools from the SDK.

mod sdk;

pub use sdk::SdkToolProvider;
