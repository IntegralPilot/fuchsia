// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_rcu::{RcuBox, RcuReadScope};
use starnix_types::string::{FsStr, FsString};

/// An RCU-protected string.
///
/// This type wraps an `RcuBox<FsString>` and provides a convenient API for reading
/// the string as an `FsStr` within an `RcuReadScope`.
#[derive(Debug, Default)]
pub struct RcuString {
    cell: RcuBox<FsString>,
}

impl RcuString {
    /// Create a new `RcuString`.
    pub fn new(value: impl Into<FsString>) -> Self {
        Self { cell: RcuBox::new(value.into()) }
    }

    /// Read the string value.
    ///
    /// The returned `FsStr` is valid for the duration of the `RcuReadScope`.
    pub fn read<'a>(&self, scope: &'a RcuReadScope) -> &'a FsStr {
        self.cell.as_ref(scope).as_ref()
    }

    /// Update the string value.
    ///
    /// This will replace the underlying `FsString` with a new one. Readers holding
    /// an `RcuReadScope` will continue to see the old value until they drop the scope.
    pub fn update(&self, value: impl Into<FsString>) {
        self.cell.update(value.into());
    }
}

impl From<FsString> for RcuString {
    fn from(value: FsString) -> Self {
        Self::new(value)
    }
}

impl From<&FsStr> for RcuString {
    fn from(value: &FsStr) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rcu_string_read() {
        let s = RcuString::new("hello");
        let scope = RcuReadScope::new();
        assert_eq!(s.read(&scope), "hello");
    }

    #[test]
    fn test_rcu_string_update() {
        let s = RcuString::new("initial");

        // Read initial value
        {
            let scope = RcuReadScope::new();
            assert_eq!(s.read(&scope), "initial");
        }

        // Update value
        s.update("updated");

        // Read new value
        {
            let scope = RcuReadScope::new();
            assert_eq!(s.read(&scope), "updated");
        }
    }
}
