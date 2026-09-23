// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Compile-time metadata attached to expectations, used in failure diagnostics.

use core::panic::Location;

/// Identifies the register that an expectation was declared against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterRef {
    /// Register type name, as reported by [`std::any::type_name`].
    pub name: &'static str,

    /// Element index, for expectations declared against an
    /// [`mmio::IndexedRegister`].
    pub index: Option<usize>,
}

impl RegisterRef {
    /// Reference to a register declared by [`mmio::register!`].
    pub fn new(name: &'static str) -> Self {
        Self { name, index: None }
    }

    /// Reference to element `index` of an indexed register.
    pub fn indexed(name: &'static str, index: usize) -> Self {
        Self { name, index: Some(index) }
    }
}

/// Source code metadata for an expectation, used to diagnose test failures.
///
/// * `register` names the register type an expectation was declared against,
///   so that mismatch reports can use register names instead of raw offsets.
/// * `location` points at the test line that declared the expectation. It is
///   captured via `#[track_caller]` and [`Location::caller`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectationSourceInfo {
    pub register: Option<RegisterRef>,
    pub location: &'static Location<'static>,
}

impl ExpectationSourceInfo {
    pub fn new(register: Option<RegisterRef>, location: &'static Location<'static>) -> Self {
        Self { register, location }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_register_ref_new() {
        let register = RegisterRef::new("registers::Status");
        assert_eq!(register.name, "registers::Status");
        assert_eq!(register.index, None);
    }

    #[fuchsia::test]
    fn test_register_ref_indexed() {
        let register = RegisterRef::indexed("registers::Data", 2);
        assert_eq!(register.name, "registers::Data");
        assert_eq!(register.index, Some(2));
    }

    #[fuchsia::test]
    fn test_expectation_source_info_new() {
        let location = Location::caller();
        let register = RegisterRef::new("registers::Status");

        let source_info = ExpectationSourceInfo::new(Some(register), location);

        assert_eq!(source_info.register, Some(RegisterRef::new("registers::Status")));
        assert_eq!(source_info.location.file(), location.file());
        assert_eq!(source_info.location.line(), location.line());
    }

    #[fuchsia::test]
    fn test_expectation_source_info_without_register() {
        let source_info = ExpectationSourceInfo::new(None, Location::caller());
        assert_eq!(source_info.register, None);
    }
}
