// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use strum::VariantArray as _;
use strum_macros::VariantArray;

use super::error::ValidateError;
use super::traits::{PolicyId, Validate};
use super::{NewPolicy, bitmap};

/// Reference policy capability Ids.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, VariantArray)]
pub enum PolicyCap {
    NetworkPeerControls = 0,
    OpenPerms = 1,
    ExtendedSocketClass = 2,
    AlwaysCheckNetwork = 3,
    CgroupSeclabel = 4,
    NnpNosuidTransition = 5,
    GenfsSeclabelSymlinks = 6,
    IoctlSkipCloexec = 7,
    UserspaceInitialContext = 8,
    NetlinkXperm = 9,
    NetifWildcard = 10,
    GenfsSeclabelWildcard = 11,
    FunctionfsSeclabel = 12,
    MemfdClass = 13,
}

impl PolicyCap {
    pub fn name(&self) -> &str {
        match self {
            Self::NetworkPeerControls => "network_peer_controls",
            Self::OpenPerms => "open_perms",
            Self::ExtendedSocketClass => "extended_socket_class",
            Self::AlwaysCheckNetwork => "always_check_network",
            Self::CgroupSeclabel => "cgroup_seclabel",
            Self::NnpNosuidTransition => "nnp_nosuid_transition",
            Self::GenfsSeclabelSymlinks => "genfs_seclabel_symlinks",
            Self::IoctlSkipCloexec => "ioctl_skip_cloexec",
            Self::UserspaceInitialContext => "userspace_initial_context",
            Self::NetlinkXperm => "netlink_xperm",
            Self::NetifWildcard => "netif_wildcard",
            Self::GenfsSeclabelWildcard => "genfs_seclabel_wildcard",
            Self::FunctionfsSeclabel => "functionfs_seclabel",
            Self::MemfdClass => "memfd_class",
        }
    }

    pub fn by_name(name: &str) -> Option<Self> {
        Self::VARIANTS.iter().find(|x| x.name() == name).copied()
    }
}

/// Set of enabled policy capabilities.
pub type PolicyCapSet = bitmap::IdSet<PolicyCap, true>;

impl PolicyId for PolicyCap {
    fn as_u32(&self) -> u32 {
        *self as u32
    }

    fn from_u32(value: u32) -> Option<Self> {
        Self::VARIANTS.get(value as usize).copied()
    }
}

impl Validate for PolicyCap {
    fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_capabilities() {
        for capability in PolicyCap::VARIANTS {
            assert_eq!(Some(*capability), PolicyCap::by_name(capability.name()));
        }
    }
}
