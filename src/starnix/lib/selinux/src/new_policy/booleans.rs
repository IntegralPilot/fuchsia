// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::NonZeroU32;

use selinux_policy_derive::{HasName, HasPolicyId, Parse, Serialize, Validate};

use super::NewPolicy;
use super::error::{ParseError, SerializeError, ValidateError};
use super::id_type::IdType;
use super::parser::{PolicyCursor, PolicyWriter};
use super::traits::{Parse, PolicyId, Serialize, Validate};

/// Tag type for type safety of policy conditional boolean identifiers.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct ConditionalBooleanTag;

/// Identifies a conditional boolean within a policy.
pub type ConditionalBooleanId = IdType<NonZeroU32, ConditionalBooleanTag>;

/// Parsed SELinux conditional boolean definition.
#[derive(Debug, Validate, HasName, HasPolicyId)]
pub struct ConditionalBoolean {
    id: ConditionalBooleanId,
    active: bool,
    name: Box<[u8]>,
}

impl ConditionalBoolean {
    /// Returns whether this conditional boolean is active (true).
    pub fn active(&self) -> bool {
        self.active
    }
}

#[derive(Parse, Serialize)]
struct BinaryConditionalBooleanMetadata {
    id: u32,
    active: u32,
    key_length: u32,
}

impl Parse for ConditionalBoolean {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let metadata = BinaryConditionalBooleanMetadata::parse(cursor)?;
        let name = Box::from(cursor.read_bytes(metadata.key_length as usize)?);
        let id = ConditionalBooleanId::from_u32(metadata.id)
            .ok_or(ParseError::InvalidId { value: metadata.id })?;
        let active = metadata.active != 0;
        Ok(Self { id, active, name })
    }
}

impl Serialize for ConditionalBoolean {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let metadata = BinaryConditionalBooleanMetadata {
            id: self.id.as_u32(),
            active: if self.active { 1 } else { 0 },
            key_length: self.name.len() as u32,
        };
        metadata.serialize(writer)?;
        writer.write_bytes(&self.name);
        Ok(())
    }
}

impl Validate for ConditionalBooleanId {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        policy.conditional_booleans().get_by_id(*self).map(|_| ()).ok_or_else(|| {
            ValidateError::UnknownId { kind: "conditional boolean", id: self.as_u32() }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::metadata::PolicyVersion;
    use crate::new_policy::traits::{HasName, HasPolicyId};

    #[test]
    fn test_conditional_boolean_parse_and_serialize() {
        let data = [
            1, 0, 0, 0, // id = 1
            1, 0, 0, 0, // active = 1 (true)
            4, 0, 0, 0, // key_length = 4
            b't', b'e', b's', b't', // name: "test"
        ];

        let mut cursor = PolicyCursor::new(&data);
        let boolean = ConditionalBoolean::parse(&mut cursor).expect("parse ConditionalBoolean");
        assert_eq!(boolean.id(), ConditionalBooleanId::from_u32(1).unwrap());
        assert!(boolean.active());
        assert_eq!(boolean.name(), b"test");

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        boolean.serialize(&mut policy_writer).expect("serialize ConditionalBoolean");
        assert_eq!(writer.as_slice(), &data);
    }

    #[test]
    fn test_conditional_booleans() {
        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/conditional_policy");
        let new_policy = NewPolicy::parse(policy_bytes).expect("parse conditional policy");
        new_policy.validate().expect("validate conditional policy");

        let booleans = new_policy.conditional_booleans();
        let first = booleans.get_by_name(b"test_cond_first").expect("test_cond_first");
        assert!(first.active());

        let second = booleans.get_by_name(b"test_cond_second").expect("test_cond_second");
        assert!(!second.active());

        let third = booleans.get_by_name(b"test_cond_third").expect("test_cond_third");
        assert!(third.active());
    }
}
