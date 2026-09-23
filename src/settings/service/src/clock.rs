// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(not(test))]
pub use settings_common::clock::inspect_format_now;

#[cfg(test)]
pub(crate) use settings_common::clock::mock::{self, inspect_format_now};
