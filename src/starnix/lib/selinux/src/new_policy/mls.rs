// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use selinux_policy_derive::{HasName, HasPolicyId, Parse, Serialize, Validate};

use super::classes::ClassId;
use super::context::{MlsLevel, MlsRange};
use super::error::{ParseError, SerializeError, ValidateError};
use super::parser::{PolicyCursor, PolicyWriter};
use super::traits::{Parse, PolicyId, Serialize, Validate};
use super::{CategoryId, SensitivityId, TypeId};

/// MLS sensitivity level definition in an SELinux policy.
#[derive(Debug, Validate, HasName, HasPolicyId)]
pub struct Sensitivity {
    id: SensitivityId,
    name: Box<[u8]>,
    is_alias: bool,
    level: MlsLevel,
}

impl Sensitivity {
    /// Returns whether this sensitivity was declared as an alias in policy.
    #[cfg(test)]
    pub fn is_alias(&self) -> bool {
        self.is_alias
    }

    /// Default [`MlsLevel`] associated with this sensitivity.
    #[cfg(test)]
    pub fn level(&self) -> &MlsLevel {
        &self.level
    }
}

#[derive(Parse, Serialize)]
struct BinarySensitivityMetadata {
    length: u32,
    is_alias: u32,
}

impl Parse for Sensitivity {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let metadata = BinarySensitivityMetadata::parse(cursor)?;
        let name = Box::from(cursor.read_bytes(metadata.length as usize)?);
        let level = MlsLevel::parse(cursor)?;
        Ok(Self { id: level.sensitivity(), name, is_alias: metadata.is_alias != 0, level })
    }
}

impl Serialize for Sensitivity {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let metadata = BinarySensitivityMetadata {
            length: self.name.len() as u32,
            is_alias: if self.is_alias { 1 } else { 0 },
        };
        metadata.serialize(writer)?;
        writer.write_bytes(&self.name);
        self.level.serialize(writer)?;
        Ok(())
    }
}

impl Validate for SensitivityId {
    fn validate(&self, policy: &super::NewPolicy) -> Result<(), ValidateError> {
        policy
            .sensitivities()
            .get_by_id(*self)
            .map(|_| ())
            .ok_or_else(|| ValidateError::UnknownId { kind: "sensitivity", id: self.as_u32() })
    }
}

/// MLS category definition in an SELinux policy.
#[derive(Debug, Validate, HasName, HasPolicyId)]
pub struct Category {
    id: CategoryId,
    name: Box<[u8]>,
    is_alias: bool,
}

impl Category {
    /// Returns whether this category was declared as an alias in policy.
    #[cfg(test)]
    pub fn is_alias(&self) -> bool {
        self.is_alias
    }
}

#[derive(Parse, Serialize)]
struct BinaryCategoryMetadata {
    length: u32,
    id: u32,
    is_alias: u32,
}

impl Parse for Category {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let metadata = BinaryCategoryMetadata::parse(cursor)?;
        let name = Box::from(cursor.read_bytes(metadata.length as usize)?);
        let id = CategoryId::from_u32(metadata.id)
            .ok_or(ParseError::InvalidId { value: metadata.id })?;
        Ok(Self { id, name, is_alias: metadata.is_alias != 0 })
    }
}

impl Serialize for Category {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let metadata = BinaryCategoryMetadata {
            length: self.name.len() as u32,
            id: self.id.as_u32(),
            is_alias: if self.is_alias { 1 } else { 0 },
        };
        metadata.serialize(writer)?;
        writer.write_bytes(&self.name);
        Ok(())
    }
}

impl Validate for CategoryId {
    fn validate(&self, policy: &super::NewPolicy) -> Result<(), ValidateError> {
        policy
            .categories()
            .get_by_id(*self)
            .map(|_| ())
            .ok_or_else(|| ValidateError::UnknownId { kind: "category", id: self.as_u32() })
    }
}

/// Range transition rule for target object creation under MLS.
#[derive(Debug, Clone, PartialEq, Eq, Parse, Serialize, Validate)]
pub struct RangeTransition {
    source_type: TypeId,
    target_type: TypeId,
    target_class: ClassId,
    mls_range: MlsRange,
}

impl RangeTransition {
    /// Returns the source [`TypeId`].
    pub fn source_type(&self) -> TypeId {
        self.source_type
    }

    /// Returns the target [`TypeId`].
    pub fn target_type(&self) -> TypeId {
        self.target_type
    }

    /// Returns the target [`ClassId`].
    pub fn target_class(&self) -> ClassId {
        self.target_class
    }

    /// Returns the target [`MlsRange`].
    pub fn mls_range(&self) -> &MlsRange {
        &self.mls_range
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::CategorySet;
    use crate::new_policy::metadata::PolicyVersion;
    use crate::new_policy::parser::PolicyWriter;
    use crate::new_policy::traits::{HasName, HasPolicyId};

    #[test]
    fn test_category_parse_and_serialize() {
        let data = [
            4, 0, 0, 0, // length = 4
            5, 0, 0, 0, // id = 5
            1, 0, 0, 0, // is_alias = 1 (true)
            b't', b'e', b's', b't', // name: "test"
        ];
        let mut cursor = PolicyCursor::new(&data);
        let category = Category::parse(&mut cursor).expect("parse Category");
        assert_eq!(category.id(), CategoryId::from_u32(5).unwrap());
        assert!(category.is_alias());
        assert_eq!(category.name(), b"test");

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        category.serialize(&mut policy_writer).expect("serialize Category");
        assert_eq!(writer.as_slice(), &data);
    }

    #[test]
    fn test_category_invalid_id() {
        let data = [
            4, 0, 0, 0, // length = 4
            0, 0, 0, 0, // id = 0 (invalid)
            0, 0, 0, 0, // is_alias = 0
            b't', b'e', b's', b't', // name: "test"
        ];
        let mut cursor = PolicyCursor::new(&data);
        assert!(matches!(Category::parse(&mut cursor), Err(ParseError::InvalidId { value: 0 })));
    }

    #[test]
    fn test_sensitivity_parse_and_serialize() {
        let data = [
            4, 0, 0, 0, // length = 4
            0, 0, 0, 0, // is_alias = 0 (false)
            b's', b'e', b'n', b's', // name: "sens"
            // Trailing MlsLevel on wire (level_datum->level):
            2, 0, 0, 0, // sensitivity = 2
            // CategorySet (ExtensibleBitmap):
            64, 0, 0, 0, // map_item_size_bits = 64
            0, 0, 0, 0, // high_bit = 0
            0, 0, 0, 0, // categories count = 0
        ];

        let mut cursor = PolicyCursor::new(&data);
        let sensitivity = Sensitivity::parse(&mut cursor).expect("parse Sensitivity");
        assert_eq!(sensitivity.id(), SensitivityId::from_u32(2).unwrap());
        assert!(!sensitivity.is_alias());
        assert_eq!(sensitivity.name(), b"sens");
        assert_eq!(sensitivity.level().sensitivity(), SensitivityId::from_u32(2).unwrap());
        assert_eq!(sensitivity.level().categories(), &CategorySet::from_ids([]));

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        sensitivity.serialize(&mut policy_writer).expect("serialize Sensitivity");
        assert_eq!(writer.as_slice(), &data);
    }

    #[test]
    fn test_range_transition_parse_and_serialize() {
        let data = [
            1, 0, 0, 0, // source_type = 1
            2, 0, 0, 0, // target_type = 2
            3, 0, 0, 0, // target_class = 3
            // MlsRange:
            1, 0, 0, 0, // levels_count = 1
            1, 0, 0, 0, // sensitivity_low = 1
            64, 0, 0, 0, // map_item_size_bits = 64
            0, 0, 0, 0, // high_bit = 0
            0, 0, 0, 0, // categories count = 0
        ];
        let mut cursor = PolicyCursor::new(&data);
        let transition = RangeTransition::parse(&mut cursor).expect("parse RangeTransition");
        assert_eq!(transition.source_type(), TypeId::from_u32(1).unwrap());
        assert_eq!(transition.target_type(), TypeId::from_u32(2).unwrap());
        assert_eq!(transition.target_class(), ClassId::from_u32(3).unwrap());
        assert_eq!(transition.mls_range().low().sensitivity(), SensitivityId::from_u32(1).unwrap());

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        transition.serialize(&mut policy_writer).expect("serialize RangeTransition");
        assert_eq!(writer.as_slice(), &data);
    }
}
