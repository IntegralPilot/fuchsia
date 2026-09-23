// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use selinux_policy_derive::{Parse, Serialize, Validate};

use super::context::Context;
use super::parser::Array;

/// Policy entry defining an initial security context for a well-known security ID.
#[derive(Debug, Clone, PartialEq, Eq, Parse, Serialize, Validate)]
pub struct InitialSid {
    id: u32,
    context: Context,
}

impl InitialSid {
    /// Returns the raw numerical ID of this initial SID entry.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Returns the initial security context defined by this entry.
    pub fn context(&self) -> &Context {
        &self.context
    }
}

/// Table of [`InitialSid`] entries defining initial security contexts for well-known security IDs.
#[derive(Debug, Clone, PartialEq, Eq, Parse, Serialize, Validate)]
pub struct InitialSids {
    entries: Array<InitialSid>,
}

impl InitialSids {
    /// Returns the initial security context for the specified numeric ID, if present.
    pub fn get_by_id(&self, id: u32) -> Option<&Context> {
        self.entries.iter().find(|initial| initial.id() == id).map(|initial| initial.context())
    }

    /// Returns the number of initial SID entries in the table.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the initial SID table is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::metadata::PolicyVersion;
    use crate::new_policy::parser::{PolicyCursor, PolicyWriter};
    use crate::new_policy::traits::{Parse, Serialize};

    #[test]
    fn test_initial_sid_parse_and_serialize() {
        let mut bytes = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut bytes);
        1u32.serialize(&mut policy_writer).unwrap(); // id = Kernel (1)
        1u32.serialize(&mut policy_writer).unwrap(); // user
        1u32.serialize(&mut policy_writer).unwrap(); // role
        1u32.serialize(&mut policy_writer).unwrap(); // type
        1u32.serialize(&mut policy_writer).unwrap(); // mls count (low only)
        1u32.serialize(&mut policy_writer).unwrap(); // sensitivity
        64u32.serialize(&mut policy_writer).unwrap(); // ExtensibleBitmap map_item_size_bits = 64
        0u32.serialize(&mut policy_writer).unwrap(); // ExtensibleBitmap high_bit = 0
        0u32.serialize(&mut policy_writer).unwrap(); // ExtensibleBitmap array count = 0

        let mut cursor = PolicyCursor::new(&bytes);
        let initial_sid = InitialSid::parse(&mut cursor).expect("parse InitialSid");
        assert_eq!(initial_sid.id(), 1);

        let mut out = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut out);
        initial_sid.serialize(&mut policy_writer).expect("serialize InitialSid");
        assert_eq!(out, bytes);
    }
}
