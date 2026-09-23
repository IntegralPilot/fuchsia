// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This is designed to be imported by users as `use crate::log::*` so we should be judicious when
// adding to this module.

pub use log::{debug, error, info, warn};
