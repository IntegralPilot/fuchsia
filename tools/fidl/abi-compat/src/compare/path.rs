// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Display;

use crate::{Scope, Version};

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq)]
pub struct Path {
    version: Version,
    string: String,
}
impl Path {
    #[cfg(test)]
    pub fn empty() -> Self {
        Self::new(Version::new("0"), String::new())
    }
    pub fn new(version: Version, string: String) -> Self {
        Self { version, string }
    }
}

impl Path {
    pub fn api_level(&self) -> &str {
        self.version.api_level()
    }
    pub fn scope(&self) -> Scope {
        self.version.scope()
    }
    pub fn string(&self) -> &str {
        &self.string
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.string())
    }
}
