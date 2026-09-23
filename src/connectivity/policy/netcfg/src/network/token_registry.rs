// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Defines structures for tracking NetworkTokens and their associated zx::EventPair.

use fidl_fuchsia_net_policy_properties as fnp_properties;
use policy_properties::NetworkTokenExt as _;
use std::collections::HashMap;

pub(crate) struct TokenRegistry<Contents> {
    contents_to_token: HashMap<Contents, (zx::Koid, fnp_properties::NetworkToken)>,
    token_to_contents: HashMap<zx::Koid, (zx::EventPair, Contents)>,
}

impl<Contents> Default for TokenRegistry<Contents> {
    fn default() -> Self {
        Self { contents_to_token: Default::default(), token_to_contents: Default::default() }
    }
}

impl<Contents> TokenRegistry<Contents> {
    pub(crate) fn get_contents(
        &self,
        token: &fnp_properties::NetworkToken,
    ) -> Result<&Contents, zx::Status> {
        // If the provided token has a value of None, or the koid fetching
        // fails, this will return an error.
        self.token_to_contents
            .get(&token.koid()?)
            .map(|(_, contents)| contents)
            .ok_or(zx::Status::NOT_FOUND)
    }

    pub(crate) fn drop_if<F: FnMut(&Contents) -> bool>(&mut self, mut predicate: F)
    where
        Contents: std::cmp::Eq + std::hash::Hash,
    {
        let to_drop = self.contents_to_token.extract_if(|c, _| predicate(c));
        for (_, (koid, _)) in to_drop {
            assert!(
                self.token_to_contents.remove(&koid).is_some(),
                "tried to delete token_to_contents entry that doesn't exist. This should never \
                happen"
            );
        }
    }

    fn insert_data(
        token_to_contents: &mut HashMap<zx::Koid, (zx::EventPair, Contents)>,
        data: Contents,
    ) -> (zx::Koid, fnp_properties::NetworkToken) {
        let (watcher, token) = zx::EventPair::create();
        let koid = token.koid().expect("unable to fetch koid for event_pair just created");
        let existing = token_to_contents.insert(koid, (watcher, data));
        assert!(
            existing.is_none(),
            "Encountered collision in token_to_contents, this should never happen."
        );
        (koid, fnp_properties::NetworkToken { value: token })
    }

    pub(crate) fn get_token(&self, contents: &Contents) -> Option<&fnp_properties::NetworkToken>
    where
        Contents: std::cmp::Eq + std::hash::Hash,
    {
        self.contents_to_token.get(&contents).map(|(_, t)| t)
    }

    /// Checks if a token exists for the provided [`NetworkTokenContents`] and if not creates one.
    /// Returns: The network token.
    pub(crate) fn ensure_token(&mut self, contents: Contents) -> TokenEntry<'_, Contents>
    where
        Contents: std::cmp::Eq + std::hash::Hash + Clone,
    {
        match self.contents_to_token.entry(contents) {
            std::collections::hash_map::Entry::Occupied(occupied_entry) => {
                TokenEntry(occupied_entry)
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                let (koid, tok) =
                    Self::insert_data(&mut self.token_to_contents, vacant_entry.key().clone());
                TokenEntry(vacant_entry.insert_entry((koid, tok)))
            }
        }
    }
}

pub(crate) struct TokenEntry<'a, Contents>(
    std::collections::hash_map::OccupiedEntry<
        'a,
        Contents,
        (zx::Koid, fnp_properties::NetworkToken),
    >,
);

impl<'a, Contents> TokenEntry<'a, Contents> {
    pub fn get(&self) -> &fnp_properties::NetworkToken {
        &self.0.get().1
    }
}
