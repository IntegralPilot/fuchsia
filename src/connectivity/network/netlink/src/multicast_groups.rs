// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A module for managing Netlink multicast group memberships.
//!
//! A Netlink socket can subscribe to any number of multicast groups, as defined
//! by the Netlink protocol family that the socket is connected to. There are
//! two modes of specifying the multicast group memberships. Mode 1 is referred
//! to as "legacy" throughout this module because it was replaced by mode 2,
//! referred to as "modern", in Linux 2.6.14.
//!     Mode 1: Specifying `nl_groups`, a 32 bit bitmask, when binding the
//!             the socket.
//!     Mode 2: Setting the `NETLINK_ADD_MEMBERSHIP` or
//!            `NETLINK_DROP_MEMBERSHIP` socket option.
//!
//! Note that both mode 1 and mode 2 are supported (for backwards
//! compatibility), and the two modes operate over different sets of constants.
//! The "modern" constants correspond to the index of the set-bit in their
//! "legacy" counterpart. For example, consider this sample of NETLINK_ROUTE
//! constants:
//!     RTNLGRP_LINK:   legacy (1), modern (1),
//!     RTNLGRP_NOTIFY: legacy (2), modern (2),
//!     RTNLGRP_NEIGH:  legacy (4), modern (3),
//!     RTNLGRP_TC:     legacy (8), modern (4),
//!
//! The [`MulticastGroupMemberships`] struct exposed by this module tracks the
//! memberships independently of the mode via which they are set. For example, a
//! `NETLINK_ROUTE` client could bind to `RTMGRP_IPV6_IFADDR` (256), to start
//! receiving IPv6 address events, and later set the `NETLINK_DROP_MEMBERSHIP`
//! socket option to `RTNLGRP_IPV6_IFADDR` (9), to stop receiving events.

use std::marker::PhantomData;

use bit_set::BitSet;

use crate::logging::{log_info, log_warn};
use crate::protocol_family::NamedNetlinkFamily;

// Safe "as" conversion because u32::BITS (32) will fit into any usize.
const U32_BITS_USIZE: usize = u32::BITS as usize;

/// A modern (non-legacy) multicast group. Interpreted as a single group.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ModernGroup(pub u32);

impl Into<usize> for ModernGroup {
    fn into(self) -> usize {
        let ModernGroup(group) = self;
        group.try_into().expect("expected usize >= u32")
    }
}

/// An error indicating that a modern group has no mapping to a legacy
/// group.
#[derive(Debug, PartialEq)]
pub struct NoMappingFromModernToLegacyGroupError;

impl TryFrom<ModernGroup> for SingleLegacyGroup {
    type Error = NoMappingFromModernToLegacyGroupError;

    fn try_from(
        ModernGroup(group): ModernGroup,
    ) -> Result<SingleLegacyGroup, NoMappingFromModernToLegacyGroupError> {
        if let Some(group) = 1u32.checked_shl(group) {
            Ok(SingleLegacyGroup(group))
        } else {
            Err(NoMappingFromModernToLegacyGroupError)
        }
    }
}

/// A set of legacy multicast groups. Interpreted as a bit mask, where each set
/// bit corresponds to a different group membership.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LegacyGroups(pub u32);

/// A single legacy multicast group membership. At most 1 bit is set.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SingleLegacyGroup(u32);

impl SingleLegacyGroup {
    /// Returns the group number as a `u32`.
    pub fn inner(&self) -> u32 {
        let SingleLegacyGroup(inner) = self;
        *inner
    }
}

/// Error returned when attempting to convert a u32 into [`SingleLegacyGroup`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MultipleBitsSetError;

impl TryFrom<u32> for SingleLegacyGroup {
    type Error = MultipleBitsSetError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        (value.count_ones() <= 1).then_some(SingleLegacyGroup(value)).ok_or(MultipleBitsSetError)
    }
}

/// Indicates support for a multicast group with respect to a particular
/// protocol family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GroupSupport {
    /// The group is valid for the protocol family and supported.
    Supported,
    /// The group is valid for the protocol family but not supported.
    Unsupported,
}

/// Error returned when attempting to join an invalid [`ModernGroup`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InvalidModernGroupError;

/// Multicast group semantics that are specific to a particular protocol family.
pub(crate) trait MulticastCapableNetlinkFamily {
    /// Returns whether `group` is supported for this protocol family, or an
    /// error if it's not a valid group for this family.
    fn check_support(group: &ModernGroup) -> Result<GroupSupport, InvalidModernGroupError>;
}

/// Manages the current multicast group memberships of a single connection to
/// Netlink.
///
/// Memberships are stored entirely using the modern set of constants. Legacy
/// memberships are translated to their modern equivalent before being stored.
#[derive(Debug)]
pub(crate) struct MulticastGroupMemberships<F: MulticastCapableNetlinkFamily> {
    /// Aspects of multicast group memberships that are family specific.
    family: PhantomData<F>,

    // The current multicast group memberships, stored as modern memberships.
    // Membership in multicast group "N" is determined by whether the "Nth" bit
    // of the `BitSet` is set.
    memberships: BitSet,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Mutation {
    None,
    Add(ModernGroup),
    Del(ModernGroup),
}

/// Error returned when attempting to join invalid [`LegacyGroups`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InvalidLegacyGroupsError;

impl<F: MulticastCapableNetlinkFamily + NamedNetlinkFamily> MulticastGroupMemberships<F> {
    /// Instantiate a new [`MulticastGroupMemberships`].
    pub(crate) fn new() -> MulticastGroupMemberships<F> {
        MulticastGroupMemberships { family: PhantomData, memberships: Default::default() }
    }

    /// Returns `True` if `self` is a member of `group`.
    pub(crate) fn member_of_group(&self, group: ModernGroup) -> bool {
        self.memberships.contains(group.into())
    }

    /// Adds the given multicast group membership.
    ///
    /// Returns `true` if the membership was newly added.
    pub(crate) fn add_membership(
        &mut self,
        group: ModernGroup,
    ) -> Result<bool, InvalidModernGroupError> {
        let MulticastGroupMemberships { family: _, memberships } = self;
        if F::check_support(&group)? == GroupSupport::Unsupported {
            log_info!(
                "{}: adding membership for valid but unsupported multicast group {:?}",
                F::NAME,
                group
            );
        }
        let was_absent = memberships.insert(group.into());
        return Ok(was_absent);
    }

    /// Deletes the given multicast group membership.
    ///
    /// Returns `true` if the membership was newly removed.
    pub(crate) fn del_membership(
        &mut self,
        group: ModernGroup,
    ) -> Result<bool, InvalidModernGroupError> {
        let MulticastGroupMemberships { family: _, memberships } = self;
        let _ = F::check_support(&group)?;
        let was_present = memberships.remove(group.into());
        return Ok(was_present);
    }

    /// Sets the legacy multicast group memberships.
    ///
    /// Legacy memberships are translated into their modern equivalent before
    /// being written.
    ///
    /// Returns the mutations applied.
    pub(crate) fn set_legacy_memberships(
        &mut self,
        LegacyGroups(requested_groups): LegacyGroups,
    ) -> Result<Vec<Mutation>, InvalidLegacyGroupsError> {
        crate::logging::log_debug!("set_legacy_memberships");
        let MulticastGroupMemberships { family: _, memberships } = self;

        let mut mutations = [Mutation::None; U32_BITS_USIZE];
        // Validate and record all the mutations that will need to be applied.
        for i in 0..U32_BITS_USIZE {
            let raw_legacy_group = 1 << i;
            let legacy_group: SingleLegacyGroup = raw_legacy_group
                .try_into()
                .expect("raw_legacy_group unexpectedly had multiple bits set");

            let modern_group = ModernGroup(legacy_group.inner().ilog2() + 1);
            let support = F::check_support(&modern_group);
            let is_member_of_group = requested_groups & raw_legacy_group != 0;
            mutations[i] = match (support, is_member_of_group) {
                (Ok(support), true) => {
                    if support == GroupSupport::Unsupported {
                        log_info!(
                            "{}: adding membership for valid but unsupported multicast group {:?}",
                            F::NAME,
                            modern_group
                        );
                    }
                    Mutation::Add(modern_group)
                }
                (Ok(_), false) => Mutation::Del(modern_group),
                (Err(_), true) => {
                    log_warn!(
                        "{}: failed to join legacy groups ({:?}) because of invalid group: {:?}",
                        F::NAME,
                        requested_groups,
                        legacy_group
                    );
                    return Err(InvalidLegacyGroupsError);
                }
                (Err(_), false) => Mutation::None,
            };
        }

        let mut return_mutations = Vec::new();
        // Apply all of the mutations.
        for mutation in mutations {
            match mutation {
                Mutation::None => {}
                Mutation::Add(group) => {
                    crate::logging::log_debug!("mutation: add {group:?}");
                    let was_absent = memberships.insert(group.into());
                    if was_absent {
                        return_mutations.push(Mutation::Add(group));
                    }
                }
                Mutation::Del(group) => {
                    crate::logging::log_debug!("mutation: del {group:?}");
                    let was_present = memberships.remove(group.into());
                    if was_present {
                        return_mutations.push(Mutation::Del(group));
                    }
                }
            }
        }
        Ok(return_mutations)
    }

    pub(crate) fn iter_groups(&self) -> impl Iterator<Item = ModernGroup> + '_ {
        self.memberships.into_iter().map(|n: usize| {
            let n = u32::try_from(n).expect("all ModernGroups fit in u32");
            ModernGroup(n)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::protocol_family::testutil::{
        FakeProtocolFamily, INVALID_LEGACY_GROUP, INVALID_MODERN_GROUP, LEGACY_GROUP1,
        LEGACY_GROUP2, LEGACY_GROUP3, MODERN_GROUP1, MODERN_GROUP2, MODERN_GROUP3,
    };

    #[test]
    fn test_single_legacy_groups() {
        assert_eq!(0.try_into(), Ok(SingleLegacyGroup(0)));
        assert_eq!(0x00010000.try_into(), Ok(SingleLegacyGroup(0x00010000)));
        assert_eq!(
            <u32 as TryInto<SingleLegacyGroup>>::try_into(0x00010100),
            Err(MultipleBitsSetError {})
        );
    }

    #[test]
    fn test_modern_to_legacy_groups() {
        assert_eq!(ModernGroup(0).try_into(), Ok(SingleLegacyGroup(0b00000001)));
        assert_eq!(ModernGroup(4).try_into(), Ok(SingleLegacyGroup(0b00010000)));
        assert_eq!(
            ModernGroup(100).try_into(),
            Err::<SingleLegacyGroup, _>(NoMappingFromModernToLegacyGroupError)
        );
    }

    #[test]
    fn test_add_del_membership() {
        let mut memberships = MulticastGroupMemberships::<FakeProtocolFamily>::new();

        assert!(!memberships.member_of_group(MODERN_GROUP1));
        assert!(!memberships.member_of_group(MODERN_GROUP2));
        assert!(!memberships.member_of_group(MODERN_GROUP3));

        // Add one membership, and verify the others are unaffected.
        let changed = memberships.add_membership(MODERN_GROUP1).expect("failed to add");
        assert!(changed, "should have changed group memberships");
        assert!(memberships.member_of_group(MODERN_GROUP1));
        assert!(!memberships.member_of_group(MODERN_GROUP2));
        assert!(!memberships.member_of_group(MODERN_GROUP3));
        // Add a second & third membership.
        let changed = memberships.add_membership(MODERN_GROUP2).expect("failed to add");
        assert!(changed, "should have changed group memberships");
        let changed = memberships.add_membership(MODERN_GROUP3).expect("failed to add");
        assert!(changed, "should have changed group memberships");
        assert!(memberships.member_of_group(MODERN_GROUP1));
        assert!(memberships.member_of_group(MODERN_GROUP2));
        assert!(memberships.member_of_group(MODERN_GROUP3));
        // Remove one membership, and verify the others are unaffected.
        let changed = memberships.del_membership(MODERN_GROUP1).expect("failed to del");
        assert!(changed, "should have changed group memberships");
        assert!(!memberships.member_of_group(MODERN_GROUP1));
        assert!(memberships.member_of_group(MODERN_GROUP2));
        assert!(memberships.member_of_group(MODERN_GROUP3));
        // Remove the second & third membership.
        let changed = memberships.del_membership(MODERN_GROUP2).expect("failed to del");
        assert!(changed, "should have changed group memberships");
        let changed = memberships.del_membership(MODERN_GROUP3).expect("failed to del");
        assert!(changed, "should have changed group memberships");
        assert!(!memberships.member_of_group(MODERN_GROUP1));
        assert!(!memberships.member_of_group(MODERN_GROUP2));
        assert!(!memberships.member_of_group(MODERN_GROUP3));
        // Verify Adding/Deleting an invalid group fails.
        assert_eq!(
            memberships.add_membership(INVALID_MODERN_GROUP),
            Err(InvalidModernGroupError {})
        );
        assert_eq!(
            memberships.del_membership(INVALID_MODERN_GROUP),
            Err(InvalidModernGroupError {})
        );
    }

    #[test]
    fn test_legacy_memberships() {
        let mut memberships = MulticastGroupMemberships::<FakeProtocolFamily>::new();

        assert!(!memberships.member_of_group(MODERN_GROUP1));
        assert!(!memberships.member_of_group(MODERN_GROUP2));
        assert!(!memberships.member_of_group(MODERN_GROUP3));

        // Add one membership and verify the others are unaffected.
        let mutations = memberships
            .set_legacy_memberships(LegacyGroups(LEGACY_GROUP1))
            .expect("failed to set legacy groups");
        let expected_mutations = [Mutation::Add(MODERN_GROUP1)];
        assert_eq!(mutations, expected_mutations);
        assert!(memberships.member_of_group(MODERN_GROUP1));
        assert!(!memberships.member_of_group(MODERN_GROUP2));
        assert!(!memberships.member_of_group(MODERN_GROUP3));
        // Add a second & third membership.
        let mutations = memberships
            .set_legacy_memberships(LegacyGroups(LEGACY_GROUP1 | LEGACY_GROUP2 | LEGACY_GROUP3))
            .expect("failed to set legacy groups");
        let expected_mutations = [Mutation::Add(MODERN_GROUP2), Mutation::Add(MODERN_GROUP3)];
        assert_eq!(mutations, expected_mutations);
        assert!(memberships.member_of_group(MODERN_GROUP1));
        assert!(memberships.member_of_group(MODERN_GROUP2));
        assert!(memberships.member_of_group(MODERN_GROUP3));
        // Remove one membership and verify the others are unaffected.
        let mutations = memberships
            .set_legacy_memberships(LegacyGroups(LEGACY_GROUP2 | LEGACY_GROUP3))
            .expect("failed to set legacy_groups");
        let expected_mutations = [Mutation::Del(MODERN_GROUP1)];
        assert_eq!(mutations, expected_mutations);
        assert!(!memberships.member_of_group(MODERN_GROUP1));
        assert!(memberships.member_of_group(MODERN_GROUP2));
        assert!(memberships.member_of_group(MODERN_GROUP3));
        // Remove the second & third membership.
        let mutations = memberships
            .set_legacy_memberships(LegacyGroups(0))
            .expect("failed to set legacy groups");
        let expected_mutations = [Mutation::Del(MODERN_GROUP2), Mutation::Del(MODERN_GROUP3)];
        assert_eq!(mutations, expected_mutations);
        assert!(!memberships.member_of_group(MODERN_GROUP1));
        assert!(!memberships.member_of_group(MODERN_GROUP2));
        assert!(!memberships.member_of_group(MODERN_GROUP3));
        // Verify that setting an invalid group fails.
        assert_eq!(
            memberships.set_legacy_memberships(LegacyGroups(INVALID_LEGACY_GROUP)),
            Err(InvalidLegacyGroupsError {})
        );
    }

    #[test]
    fn test_legacy_and_modern_memberships() {
        let mut memberships = MulticastGroupMemberships::<FakeProtocolFamily>::new();

        assert!(!memberships.member_of_group(MODERN_GROUP1));
        assert!(!memberships.member_of_group(MODERN_GROUP2));

        // Add memberships by their legacy group and drop by their modern group.
        let _: Vec<Mutation> = memberships
            .set_legacy_memberships(LegacyGroups(LEGACY_GROUP1 | LEGACY_GROUP2))
            .expect("failed to set legacy groups");
        assert!(memberships.member_of_group(MODERN_GROUP1));
        assert!(memberships.member_of_group(MODERN_GROUP2));
        let _: bool = memberships.del_membership(MODERN_GROUP1).expect("failed to del");
        assert!(!memberships.member_of_group(MODERN_GROUP1));
        assert!(memberships.member_of_group(MODERN_GROUP2));
        let _: bool = memberships.del_membership(MODERN_GROUP2).expect("failed to del");
        assert!(!memberships.member_of_group(MODERN_GROUP1));
        assert!(!memberships.member_of_group(MODERN_GROUP2));

        // Add memberships by their modern group and drop by their legacy group.
        let _: bool = memberships.add_membership(MODERN_GROUP1).expect("failed to add");
        let _: bool = memberships.add_membership(MODERN_GROUP2).expect("failed to add");
        assert!(memberships.member_of_group(MODERN_GROUP1));
        assert!(memberships.member_of_group(MODERN_GROUP2));
        let _: Vec<Mutation> = memberships
            .set_legacy_memberships(LegacyGroups(LEGACY_GROUP2))
            .expect("failed to set legacy groups");
        assert!(!memberships.member_of_group(MODERN_GROUP1));
        assert!(memberships.member_of_group(MODERN_GROUP2));
        let _: Vec<Mutation> = memberships
            .set_legacy_memberships(LegacyGroups(0))
            .expect("failed to set legacy groups");
        assert!(!memberships.member_of_group(MODERN_GROUP1));
        assert!(!memberships.member_of_group(MODERN_GROUP2));
    }
}
