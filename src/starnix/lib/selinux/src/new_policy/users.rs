// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use selinux_policy_derive::{HasName, HasPolicyId, Parse, Serialize, Validate};

use super::context::{MlsLevel, MlsRange};
use super::error::{ParseError, SerializeError, ValidateError};
use super::parser::{PolicyCursor, PolicyWriter};
use super::traits::{Parse, PolicyId, Serialize, Validate};
use super::{NewPolicy, RoleSet, UserId};

/// Parsed SELinux user definition.
#[derive(Debug, Validate, HasName, HasPolicyId)]
pub struct User {
    id: UserId,
    name: Box<[u8]>,
    bounds: Option<UserId>,
    roles: RoleSet,
    range: MlsRange,
    default_level: MlsLevel,
}

impl User {
    pub fn bounds(&self) -> Option<UserId> {
        self.bounds
    }

    pub fn roles(&self) -> &RoleSet {
        &self.roles
    }

    pub fn mls_range(&self) -> &MlsRange {
        &self.range
    }

    #[cfg(test)]
    pub fn default_level(&self) -> &MlsLevel {
        &self.default_level
    }
}

#[derive(Parse, Serialize)]
struct BinaryUserMetadata {
    key_length: u32,
    id: UserId,
    bounds: Option<UserId>,
}

impl Parse for User {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let metadata = BinaryUserMetadata::parse(cursor)?;
        let name = Box::from(cursor.read_bytes(metadata.key_length as usize)?);
        let roles = RoleSet::parse(cursor)?;
        let range = MlsRange::parse(cursor)?;
        let default_level = MlsLevel::parse(cursor)?;

        Ok(Self { id: metadata.id, name, bounds: metadata.bounds, roles, range, default_level })
    }
}

impl Serialize for User {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let metadata = BinaryUserMetadata {
            key_length: self.name.len() as u32,
            id: self.id,
            bounds: self.bounds,
        };
        metadata.serialize(writer)?;
        writer.write_bytes(&self.name);
        self.roles.serialize(writer)?;
        self.range.serialize(writer)?;
        self.default_level.serialize(writer)?;
        Ok(())
    }
}

impl Validate for UserId {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        policy
            .users()
            .get_by_id(*self)
            .map(|_| ())
            .ok_or_else(|| ValidateError::UnknownId { kind: "user", id: self.as_u32() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::metadata::PolicyVersion;
    use crate::new_policy::traits::{HasName, HasPolicyId};

    #[test]
    fn test_user_parse_and_serialize() {
        let data = [
            // BinaryUserMetadata
            4, 0, 0, 0, // key_length = 4
            1, 0, 0, 0, // id = 1
            0, 0, 0, 0, // bounds = 0 (None)
            // name: "test"
            b't', b'e', b's', b't', // roles (empty RoleSet)
            64, 0, 0, 0, // map_item_size_bits = 64
            0, 0, 0, 0, // high_bit = 0
            0, 0, 0, 0, // count = 0
            // range (MlsRange with low level only)
            1, 0, 0, 0, // count = 1
            1, 0, 0, 0, // sensitivity_low = 1
            // low_categories (empty CategorySet)
            64, 0, 0, 0, // map_item_size_bits = 64
            0, 0, 0, 0, // high_bit = 0
            0, 0, 0, 0, // count = 0
            // default_level (MlsLevel)
            1, 0, 0, 0, // sensitivity = 1
            // categories (empty CategorySet)
            64, 0, 0, 0, // map_item_size_bits = 64
            0, 0, 0, 0, // high_bit = 0
            0, 0, 0, 0, // count = 0
        ];
        let mut cursor = PolicyCursor::new(&data);
        let user = User::parse(&mut cursor).unwrap();
        assert_eq!(user.id(), UserId::from_u32(1).unwrap());
        assert_eq!(user.name(), b"test");
        assert!(user.bounds().is_none());
        assert!(user.roles().is_empty());
        assert_eq!(user.mls_range().low().sensitivity().as_u32(), 1);
        assert_eq!(user.default_level().sensitivity().as_u32(), 1);

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        user.serialize(&mut policy_writer).unwrap();
        assert_eq!(writer, data);
    }
}
