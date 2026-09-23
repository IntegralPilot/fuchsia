// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::path::Path;

pub struct Build {}

impl Build {
    pub fn new() -> Self {
        unreachable!()
    }

    pub fn file(self, _: impl AsRef<Path>) -> Self {
        unreachable!()
    }

    pub fn compile(self, _: impl AsRef<Path>) -> Self {
        unreachable!()
    }
}
