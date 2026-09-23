// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Decider {
    executability_restrictions: system_image::ExecutabilityRestrictions,
    base_packages: Arc<crate::BaseIndex>,
}

impl Decider {
    pub(crate) fn new(
        executability_restrictions: system_image::ExecutabilityRestrictions,
        base_packages: Arc<crate::BaseIndex>,
    ) -> Self {
        Self { executability_restrictions, base_packages }
    }

    pub(crate) fn decide(&self, package: fuchsia_hash::Hash) -> ExecutabilityStatus {
        use ExecutabilityStatus::*;
        use system_image::ExecutabilityRestrictions::*;
        let is_base = self.base_packages.is_package(package);
        match (is_base, self.executability_restrictions) {
            (true, _) => Allowed,
            (false, Enforce) => Forbidden,
            (false, DoNotEnforce) => Allowed,
        }
    }
}

pub(crate) enum ExecutabilityStatus {
    Allowed,
    Forbidden,
}

impl From<ExecutabilityStatus> for fidl_fuchsia_io::Flags {
    fn from(status: ExecutabilityStatus) -> Self {
        match status {
            ExecutabilityStatus::Allowed => fio::PERM_READABLE | fio::PERM_EXECUTABLE,
            ExecutabilityStatus::Forbidden => fio::PERM_READABLE,
        }
    }
}
