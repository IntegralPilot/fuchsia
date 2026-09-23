// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use attribution_processing::GlobalPrincipalIdentifier;
use std::collections::HashMap;

/// A principal identifier, provided by an attribution provider. This identifier is only unique
/// locally, for a given attribution provider.
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub struct LocalPrincipalIdentifier(pub u64);

impl LocalPrincipalIdentifier {
    pub fn self_identifier() -> Self {
        LocalPrincipalIdentifier(fidl_fuchsia_memory_attribution::SELF)
    }

    #[cfg(test)]
    pub fn new_for_tests(value: u64) -> Self {
        Self(value)
    }
}

/// Map between local and global PrincipalIdentifiers.
#[derive(Default)]
pub struct PrincipalIdMap(HashMap<LocalPrincipalIdentifier, GlobalPrincipalIdentifier>);

impl PrincipalIdMap {
    pub fn insert(
        &mut self,
        local_id: LocalPrincipalIdentifier,
        global_id: GlobalPrincipalIdentifier,
    ) {
        self.0.insert(local_id, global_id);
    }

    /// Returns the GlobalPrincipalIdentifier corresponding to `local_id`, provided by the
    /// Principal `parent_id`.
    pub fn get(
        &self,
        local_id: LocalPrincipalIdentifier,
        parent_id: GlobalPrincipalIdentifier,
    ) -> GlobalPrincipalIdentifier {
        if local_id == LocalPrincipalIdentifier::self_identifier() {
            parent_id
        } else {
            *self.0.get(&local_id).unwrap()
        }
    }
}
