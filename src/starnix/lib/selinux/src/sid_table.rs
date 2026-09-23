// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::policy::{Policy, SecurityContext, SecurityContextError};
use crate::{InitialSid, ReferenceInitialSid, SecurityId};

use std::num::NonZeroU32;
use std::sync::Arc;

#[derive(Clone)]
enum Entry {
    /// Used in the "typical" case, when a SID is mapped to a [`SecurityContext`].
    Valid { security_context: SecurityContext },

    /// Used for the cases of unused initial SIDs (which are never mapped to [`SecurityContext`]s)
    /// and of SIDs which after a load of a new policy are no longer mapped to valid
    /// [`SecurityContext`]s.
    Invalid { context_string: Vec<u8> },
}

/// Maintains the mapping between [`SecurityId`]s and [`SecurityContext`]s. Grows with use:
/// `SecurityId`s are minted when code asks a `SidTable` for the `SecurityId` associated
/// with a given `SecurityContext` and the `SidTable` doesn't happen to have seen that
/// `SecurityContext` before.
pub struct SidTable {
    /// The policy associated with this [`SidTable`].
    policy: Arc<Policy>,

    /// The mapping from [`SecurityId`] (represented by integer position) to
    /// [`SecurityContext`] (or for some [`SecurityId`]s, something other than a valid
    /// [`SecurityContext`]).
    entries: Vec<Entry>,
}

impl SidTable {
    pub fn new(policy: Arc<Policy>) -> Self {
        Self::new_from(
            policy,
            vec![
                Entry::Invalid { context_string: Vec::new() };
                ReferenceInitialSid::FirstUnused as usize
            ],
        )
    }

    pub fn new_from_previous(policy: Arc<Policy>, previous: &Self) -> Self {
        let mut new_entries = vec![
            Entry::Invalid { context_string: Vec::new() };
            ReferenceInitialSid::FirstUnused as usize
        ];
        new_entries.reserve(previous.entries.len());

        // Remap any existing Security Contexts to use Ids defined by the new policy.
        // TODO: https://fxbug.dev/330677360 - replace serialize/parse with an
        // efficient implementation.
        new_entries.extend(
            previous.entries[ReferenceInitialSid::FirstUnused as usize..].iter().map(
                |previous_entry| {
                    let serialized_context = match previous_entry {
                        Entry::Valid { security_context } => {
                            previous.policy.serialize_security_context(&security_context)
                        }
                        Entry::Invalid { context_string } => context_string.clone(),
                    };
                    let context =
                        policy.parse_security_context(serialized_context.as_slice().into());
                    if let Ok(context) = context {
                        Entry::Valid { security_context: context }
                    } else {
                        Entry::Invalid { context_string: serialized_context }
                    }
                },
            ),
        );

        Self::new_from(policy, new_entries)
    }

    /// Returns the `SecurityId` (SID) representing the supplied `security_context`, if it is
    /// already present in the SID table. If a SID is returned then `security_context` must be valid
    /// by definition, since `security_context_to_sid()` will only insert validated contexts.
    pub fn security_context_to_existing_sid(
        &self,
        security_context: &SecurityContext,
    ) -> Option<SecurityId> {
        let index = &self.entries[ReferenceInitialSid::FirstUnused as usize..]
            .iter()
            .position(|entry| match entry {
                Entry::Valid { security_context: entry_security_context } => {
                    security_context == entry_security_context
                }
                Entry::Invalid { .. } => false,
            })
            .map(|slice_relative_index| {
                slice_relative_index + (ReferenceInitialSid::FirstUnused as usize)
            })?;
        Some(SecurityId(NonZeroU32::new(*index as u32).unwrap()))
    }

    /// Returns a `SecurityId` (SID) representing the supplied `security_context`.
    /// If an entry already exists that matches `security_context` then the existing SID is
    /// returned.
    /// Otherwise the `security_context`'s fields are validated against the policy constraints,
    /// and an explanatory error returned if they are inconsistent.
    pub fn security_context_to_sid(
        &mut self,
        security_context: &SecurityContext,
    ) -> Result<SecurityId, SecurityContextError> {
        if let Some(sid) = self.security_context_to_existing_sid(security_context) {
            return Ok(sid);
        }
        self.policy.validate_security_context(security_context)?;
        let index = self.entries.len();
        self.entries.push(Entry::Valid { security_context: security_context.clone() });
        Ok(SecurityId(NonZeroU32::new(index as u32).unwrap()))
    }

    /// Returns the [`SecurityContext`] associated with `sid`.
    ///
    /// If `sid` was invalidated by a policy reload then the "unlabeled"
    /// context is returned instead.
    pub fn sid_to_security_context(&self, sid: SecurityId) -> &SecurityContext {
        match &self.entries[sid.0.get() as usize] {
            Entry::Valid { security_context } => security_context,
            Entry::Invalid { .. } => match &self.entries[InitialSid::Unlabeled as usize] {
                Entry::Valid { security_context } => security_context,
                Entry::Invalid { .. } => unreachable!("initial SID unlabeled must be valid"),
            },
        }
    }

    fn new_from(policy: Arc<Policy>, mut new_entries: Vec<Entry>) -> Self {
        for initial_sid in InitialSid::all_variants() {
            let initial_context = policy.initial_context(*initial_sid);
            new_entries[*initial_sid as usize] = Entry::Valid { security_context: initial_context };
        }

        SidTable { policy, entries: new_entries }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::policy::parse_policy_by_value;

    const TESTS_BINARY_POLICY: &[u8] =
        include_bytes!("../testdata/micro_policies/security_server_tests_policy");

    fn test_policy() -> Arc<Policy> {
        let unvalidated = parse_policy_by_value(TESTS_BINARY_POLICY.to_vec()).unwrap();
        Arc::new(unvalidated.validate().unwrap())
    }

    #[test]
    fn sid_to_security_context() {
        let policy = test_policy();
        let security_context = policy
            .parse_security_context(b"unconfined_u:unconfined_r:unconfined_t:s0".into())
            .unwrap();
        let mut sid_table = SidTable::new(policy);
        let sid = sid_table.security_context_to_sid(&security_context).unwrap();
        assert_eq!(*sid_table.sid_to_security_context(sid), security_context);
    }

    #[test]
    fn sids_for_different_security_contexts_differ() {
        let policy = test_policy();
        let mut sid_table = SidTable::new(policy.clone());
        let sid1 = sid_table.security_context_to_sid(
            &policy.parse_security_context(b"user0:object_r:type0:s0".into()).unwrap(),
        );
        let sid2 = sid_table.security_context_to_sid(
            &policy
                .parse_security_context(b"unconfined_u:unconfined_r:unconfined_t:s0".into())
                .unwrap(),
        );
        assert_ne!(sid1, sid2);
    }

    #[test]
    fn sids_for_same_security_context_are_equal() {
        let policy = test_policy();
        let security_context = policy
            .parse_security_context(b"unconfined_u:unconfined_r:unconfined_t:s0".into())
            .unwrap();
        let mut sid_table = SidTable::new(policy);
        let sid_count_before = sid_table.entries.len();
        let sid1 = sid_table.security_context_to_sid(&security_context);
        let sid2 = sid_table.security_context_to_sid(&security_context);
        assert_eq!(sid1, sid2);
        assert_eq!(sid_table.entries.len(), sid_count_before + 1);
    }

    #[test]
    fn sids_allocated_outside_initial_range() {
        let policy = test_policy();
        let security_context = policy
            .parse_security_context(b"unconfined_u:unconfined_r:unconfined_t:s0".into())
            .unwrap();
        let mut sid_table = SidTable::new(policy);
        let sid_count_before = sid_table.entries.len();
        let sid = sid_table.security_context_to_sid(&security_context).unwrap();
        assert_eq!(sid_table.entries.len(), sid_count_before + 1);
        assert!(sid.0.get() >= ReferenceInitialSid::FirstUnused as u32);
    }

    #[test]
    fn initial_sids_remapped_to_dynamic_sids() {
        let file_initial_sid = InitialSid::File.into();
        let policy = test_policy();
        let mut sid_table = SidTable::new(policy);
        let file_initial_security_context = sid_table.sid_to_security_context(file_initial_sid);
        let file_dynamic_sid =
            sid_table.security_context_to_sid(&file_initial_security_context.clone()).unwrap();
        assert_ne!(file_initial_sid.0.get(), file_dynamic_sid.0.get());
        assert!(file_dynamic_sid.0.get() >= ReferenceInitialSid::FirstUnused as u32);
    }
}
