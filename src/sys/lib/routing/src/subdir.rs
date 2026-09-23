// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for working with subdirectories.

use std::fmt;

use cm_types::{ParseError, RelativePath};

/// A subdirectory of a directory capability.
#[derive(Eq, Ord, PartialOrd, PartialEq, Debug, Hash, Clone, Default)]
pub struct SubDir(RelativePath);

impl SubDir {
    pub fn new(path: impl AsRef<str>) -> Result<Self, ParseError> {
        let path = RelativePath::new(path)?;
        Ok(SubDir(path))
    }

    pub fn dot() -> Self {
        Self(RelativePath::dot())
    }
}

impl AsRef<RelativePath> for SubDir {
    fn as_ref(&self) -> &RelativePath {
        &self.0
    }
}

impl AsMut<RelativePath> for SubDir {
    fn as_mut(&mut self) -> &mut RelativePath {
        &mut self.0
    }
}

impl From<RelativePath> for SubDir {
    fn from(value: RelativePath) -> Self {
        Self(value)
    }
}

impl Into<RelativePath> for SubDir {
    fn into(self) -> RelativePath {
        let Self(path) = self;
        path
    }
}

impl fmt::Display for SubDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
