// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Methods from the "Platform Infrastructure Interface" group [1]
///
/// [1] https://openthread.io/reference/group/plat-infra-if
pub trait InfraInterface {
    /// The infra interface driver calls this method to notify OpenThread of
    /// the interface state changes.
    fn plat_infra_if_on_state_changed(&self, id: u32, is_running: bool);

    /// The infra interface driver calls this method to notify OpenThread that
    /// the discovery of NAT64 prefix is done.
    fn plat_infra_if_discover_nat64_prefix_done(&self, infra_if_idx: u32, ip6_prefix: Ip6Prefix);
}

impl<T: InfraInterface + ot::Boxable> InfraInterface for ot::Box<T> {
    fn plat_infra_if_on_state_changed(&self, id: u32, is_running: bool) {
        self.as_ref().plat_infra_if_on_state_changed(id, is_running);
    }

    fn plat_infra_if_discover_nat64_prefix_done(&self, infra_if_idx: u32, ip6_prefix: Ip6Prefix) {
        self.as_ref().plat_infra_if_discover_nat64_prefix_done(infra_if_idx, ip6_prefix);
    }
}

impl InfraInterface for Instance {
    fn plat_infra_if_on_state_changed(&self, id: u32, is_running: bool) {
        unsafe {
            otPlatInfraIfStateChanged(self.as_ot_ptr(), id, is_running);
        }
    }

    fn plat_infra_if_discover_nat64_prefix_done(&self, infra_if_idx: u32, ip6_prefix: Ip6Prefix) {
        unsafe {
            otPlatInfraIfDiscoverNat64PrefixDone(self.as_ot_ptr(), infra_if_idx, &ip6_prefix.into())
        }
    }
}
