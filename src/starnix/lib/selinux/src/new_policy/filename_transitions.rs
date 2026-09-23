// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::hash::{BuildHasher, Hash, Hasher};

use hashbrown::HashTable;
use rapidhash::RapidBuildHasher;
use selinux_policy_derive::{Parse, Serialize, Validate};

use super::bitmap::IdSet;
use super::error::{ParseError, SerializeError, ValidateError};
use super::metadata::PolicyVersion;
use super::parser::{Array, ByteArray, PolicyCursor, PolicyWriter};
use super::traits::{Parse, Serialize, Validate};
use super::{ClassId, NewPolicy, TypeId, U24Index};

/// Output type mapping for a set of source types in a filename transition (policy version >= 33).
#[derive(Debug, PartialEq, Eq, Parse, Serialize, Validate)]
pub struct FilenameTransitionItem {
    stypes: IdSet<TypeId>,
    out_type: TypeId,
}

/// Filename transition rule grouping items by target type, target class, and filename (policy version >= 33).
#[derive(Debug, PartialEq, Eq, Parse, Serialize, Validate)]
pub struct FilenameTransition {
    filename: ByteArray,
    transition_type: TypeId,
    transition_class: ClassId,
    items: Array<FilenameTransitionItem>,
}

/// Deprecated filename transition rule with one output type per entry (policy version <= 32).
#[derive(Debug, PartialEq, Eq, Parse, Serialize)]
struct DeprecatedFilenameTransition {
    filename: ByteArray,
    source_type: TypeId,
    transition_type: TypeId,
    transition_class: ClassId,
    out_type: TypeId,
}

fn hash_key(
    hasher: &RapidBuildHasher,
    target_type: TypeId,
    target_class: ClassId,
    filename: &[u8],
) -> u64 {
    let mut state = hasher.build_hasher();
    target_type.hash(&mut state);
    target_class.hash(&mut state);
    filename.hash(&mut state);
    state.finish()
}

fn matches_key(
    t: &FilenameTransition,
    target_type: TypeId,
    target_class: ClassId,
    filename: &[u8],
) -> bool {
    t.transition_type == target_type
        && t.transition_class == target_class
        && t.filename.as_ref() == filename
}

/// Table of filename transitions parsed from policy, indexed by (target_type, target_class, filename).
#[derive(Debug)]
pub struct FilenameTransitions {
    transitions: Array<FilenameTransition>,
    index: HashTable<U24Index>,
    target_types: IdSet<TypeId>,
    hasher: RapidBuildHasher,
}

impl FilenameTransitions {
    /// Constructs a [`FilenameTransitions`] table and builds its lookup index and target type bitmap over `transitions`.
    pub fn new(transitions: Array<FilenameTransition>) -> Result<Self, ParseError> {
        let hasher = RapidBuildHasher::default();
        let mut index = HashTable::with_capacity(transitions.len());
        let mut target_types = Vec::with_capacity(transitions.len());

        for (i, transition) in transitions.iter().enumerate() {
            target_types.push(transition.transition_type);
            let u24_idx: U24Index = i.try_into()?;
            let hash = hash_key(
                &hasher,
                transition.transition_type,
                transition.transition_class,
                &transition.filename,
            );
            match index.entry(
                hash,
                |&idx| {
                    matches_key(
                        &transitions[usize::from(idx)],
                        transition.transition_type,
                        transition.transition_class,
                        &transition.filename,
                    )
                },
                |&idx| {
                    let t = &transitions[usize::from(idx)];
                    hash_key(&hasher, t.transition_type, t.transition_class, &t.filename)
                },
            ) {
                hashbrown::hash_table::Entry::Occupied(_) => {
                    return Err(ParseError::DuplicateFilenameTransition {
                        target_type: transition.transition_type,
                        target_class: transition.transition_class,
                        filename: transition.filename.as_ref().into(),
                    });
                }
                hashbrown::hash_table::Entry::Vacant(vacant) => {
                    vacant.insert(u24_idx);
                }
            }
        }

        Ok(Self { transitions, index, target_types: IdSet::from_ids(target_types), hasher })
    }

    /// Returns `true` if any filename transitions are defined for `target_type`.
    pub fn has_filename_transitions_for_target_type(&self, target_type: TypeId) -> bool {
        self.target_types.contains(target_type)
    }

    /// Looks up the resulting output type for a file created by `source_type` in `target_type`
    /// directory with object `class` and `name`.
    pub fn compute_filename_transition(
        &self,
        source_type: TypeId,
        target_type: TypeId,
        class: ClassId,
        name: &[u8],
    ) -> Option<TypeId> {
        if !self.target_types.contains(target_type) {
            return None;
        }
        let hash = hash_key(&self.hasher, target_type, class, name);
        let idx = self.index.find(hash, |&i| {
            matches_key(&self.transitions[usize::from(i)], target_type, class, name)
        })?;
        let transition = &self.transitions[usize::from(*idx)];
        let item = transition.items.iter().find(|item| item.stypes.contains(source_type))?;
        Some(item.out_type)
    }

    /// Parses legacy [`DeprecatedFilenameTransition`] records (policy version <= 32)
    /// and groups them into canonical [`FilenameTransition`] structures.
    ///
    /// Assumes that in binary policies generated by `checkpolicy` (policy version <= 32),
    /// filename transitions sharing the same `(transition_type, transition_class, filename)`
    /// and `out_type` are stored consecutively.
    fn parse_deprecated(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        struct GroupedItem {
            stypes: Vec<TypeId>,
            out_type: TypeId,
        }

        struct GroupedTransition {
            filename: ByteArray,
            transition_type: TypeId,
            transition_class: ClassId,
            items: Vec<GroupedItem>,
        }

        let deprecated_list = Array::<DeprecatedFilenameTransition>::parse(cursor)?;
        let mut grouped: Vec<GroupedTransition> = Vec::new();
        let mut current: Option<GroupedTransition> = None;

        for dep in deprecated_list.iter() {
            if let Some(ref mut curr) = current {
                if curr.transition_type == dep.transition_type
                    && curr.transition_class == dep.transition_class
                    && curr.filename == dep.filename
                {
                    if let Some(item) = curr.items.last_mut().filter(|i| i.out_type == dep.out_type)
                    {
                        item.stypes.push(dep.source_type);
                    } else {
                        curr.items.push(GroupedItem {
                            stypes: vec![dep.source_type],
                            out_type: dep.out_type,
                        });
                    }
                    continue;
                }
            }
            if let Some(prev) = current.take() {
                grouped.push(prev);
            }
            current = Some(GroupedTransition {
                filename: dep.filename.clone(),
                transition_type: dep.transition_type,
                transition_class: dep.transition_class,
                items: vec![GroupedItem { stypes: vec![dep.source_type], out_type: dep.out_type }],
            });
        }
        if let Some(prev) = current {
            grouped.push(prev);
        }

        let transitions = grouped
            .into_iter()
            .map(|g| FilenameTransition {
                filename: g.filename,
                transition_type: g.transition_type,
                transition_class: g.transition_class,
                items: Array::from(
                    g.items
                        .into_iter()
                        .map(|item| FilenameTransitionItem {
                            stypes: IdSet::from_ids(item.stypes),
                            out_type: item.out_type,
                        })
                        .collect::<Vec<_>>(),
                ),
            })
            .collect::<Vec<_>>();
        Self::new(Array::from(transitions))
    }

    /// Serializes filename transitions to legacy [`DeprecatedFilenameTransition`] records (policy version <= 32).
    fn serialize_deprecated(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let items = self.transitions.iter().flat_map(|t| t.items.iter());
        let count: usize = items.map(|item| item.stypes.count()).sum();
        let count_u32: u32 = count as u32;
        count_u32.serialize(writer)?;
        for transition in self.transitions.iter() {
            for item in transition.items.iter() {
                for source_type in item.stypes.iter() {
                    let dep = DeprecatedFilenameTransition {
                        filename: transition.filename.clone(),
                        source_type,
                        transition_type: transition.transition_type,
                        transition_class: transition.transition_class,
                        out_type: item.out_type,
                    };
                    dep.serialize(writer)?;
                }
            }
        }
        Ok(())
    }
}

impl Parse for FilenameTransitions {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        if cursor.policy_version() >= PolicyVersion::V33 {
            let list = Array::<FilenameTransition>::parse(cursor)?;
            return Self::new(list);
        }
        Self::parse_deprecated(cursor)
    }
}

impl Serialize for FilenameTransitions {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        if writer.version() >= PolicyVersion::V33 {
            return self.transitions.serialize(writer);
        }
        self.serialize_deprecated(writer)
    }
}

impl Validate for FilenameTransitions {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        self.transitions.validate(policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU16;

    #[test]
    fn test_filename_transition_geq33_parse_and_serialize() {
        // Build serialized FilenameTransition array with 1 entry:
        // count = 1
        // entry 0:
        //   filename: len = 4, data = "test"
        //   transition_type = 2
        //   transition_class = 3
        //   items count = 1:
        //     item 0:
        //       stypes IdSet: len = 1, bit u64 = 1 << 1 (type 1)
        //       out_type = 4
        let mut bytes = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut bytes);
        1u32.serialize(&mut policy_writer).unwrap(); // array count
        4u32.serialize(&mut policy_writer).unwrap(); // filename len
        policy_writer.write_bytes(b"test");
        2u32.serialize(&mut policy_writer).unwrap(); // transition_type
        3u32.serialize(&mut policy_writer).unwrap(); // transition_class
        1u32.serialize(&mut policy_writer).unwrap(); // items count
        64u32.serialize(&mut policy_writer).unwrap(); // IdSet map_item_size_bits = 64
        64u32.serialize(&mut policy_writer).unwrap(); // IdSet high_bit = 64
        1u32.serialize(&mut policy_writer).unwrap(); // IdSet items_count = 1
        0u32.serialize(&mut policy_writer).unwrap(); // IdSet node start_bit = 0
        1u64.serialize(&mut policy_writer).unwrap(); // IdSet bit 0 (type 1)
        4u32.serialize(&mut policy_writer).unwrap(); // out_type

        let mut cursor = PolicyCursor::new(&bytes);
        cursor.set_policy_version(PolicyVersion::V33);
        let transitions = cursor.parse::<FilenameTransitions>().expect("parse geq33");

        let s_id = TypeId::new(NonZeroU16::new(1).unwrap());
        let t_id = TypeId::new(NonZeroU16::new(2).unwrap());
        let c_id = ClassId::new(NonZeroU16::new(3).unwrap());
        let out_id = TypeId::new(NonZeroU16::new(4).unwrap());

        assert_eq!(
            transitions.compute_filename_transition(s_id, t_id, c_id, b"test"),
            Some(out_id)
        );
        assert_eq!(transitions.compute_filename_transition(s_id, t_id, c_id, b"other"), None);

        let mut out = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut out);
        transitions.serialize(&mut policy_writer).expect("serialize geq33");
        assert_eq!(out, bytes);
    }

    #[test]
    fn test_filename_transition_leq32_parse_and_convert() {
        // Build serialized DeprecatedFilenameTransition array with 2 entries:
        // entry 0: filename = "test", source = 1, target = 2, class = 3, out = 4
        // entry 1: filename = "test", source = 5, target = 2, class = 3, out = 4
        let mut bytes = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V30, &mut bytes);
        2u32.serialize(&mut policy_writer).unwrap(); // array count
        4u32.serialize(&mut policy_writer).unwrap(); // filename len
        policy_writer.write_bytes(b"test");
        1u32.serialize(&mut policy_writer).unwrap(); // source_type
        2u32.serialize(&mut policy_writer).unwrap(); // transition_type
        3u32.serialize(&mut policy_writer).unwrap(); // transition_class
        4u32.serialize(&mut policy_writer).unwrap(); // out_type

        4u32.serialize(&mut policy_writer).unwrap(); // filename len
        policy_writer.write_bytes(b"test");
        5u32.serialize(&mut policy_writer).unwrap(); // source_type
        2u32.serialize(&mut policy_writer).unwrap(); // transition_type
        3u32.serialize(&mut policy_writer).unwrap(); // transition_class
        4u32.serialize(&mut policy_writer).unwrap(); // out_type

        let mut cursor = PolicyCursor::new(&bytes);
        cursor.set_policy_version(PolicyVersion::V30);
        let transitions = cursor.parse::<FilenameTransitions>().expect("parse leq32");

        let s1_id = TypeId::new(NonZeroU16::new(1).unwrap());
        let s5_id = TypeId::new(NonZeroU16::new(5).unwrap());
        let s9_id = TypeId::new(NonZeroU16::new(9).unwrap());
        let t_id = TypeId::new(NonZeroU16::new(2).unwrap());
        let c_id = ClassId::new(NonZeroU16::new(3).unwrap());
        let out_id = TypeId::new(NonZeroU16::new(4).unwrap());

        assert_eq!(
            transitions.compute_filename_transition(s1_id, t_id, c_id, b"test"),
            Some(out_id)
        );
        assert_eq!(
            transitions.compute_filename_transition(s5_id, t_id, c_id, b"test"),
            Some(out_id)
        );
        assert_eq!(transitions.compute_filename_transition(s9_id, t_id, c_id, b"test"), None);

        let mut out = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V30, &mut out);
        transitions.serialize(&mut policy_writer).expect("serialize v30");
        assert_eq!(out, bytes);
    }

    #[test]
    fn test_filename_transition_policy_elements() {
        use crate::new_policy::traits::HasPolicyId;

        let policy_bytes =
            include_bytes!("../../testdata/composite_policies/compiled/type_transition_policy");
        let new_policy = NewPolicy::parse(policy_bytes).expect("parse type_transition policy");
        new_policy.validate().expect("validate type_transition policy");

        let transitions = new_policy.filename_transitions();
        assert_eq!(transitions.transitions.len(), 1);

        let source_type =
            new_policy.types().get_by_name(b"source_t").expect("look up source_t").id();
        let target_type =
            new_policy.types().get_by_name(b"target_t").expect("look up target_t").id();
        let class = new_policy.classes().get_by_name(b"file").expect("look up file").id();
        let special_transition = new_policy
            .types()
            .get_by_name(b"special_transition_t")
            .expect("look up special_transition_t")
            .id();

        assert_eq!(
            transitions.compute_filename_transition(
                source_type,
                target_type,
                class,
                b"special_file",
            ),
            Some(special_transition),
        );
        assert_eq!(
            transitions.compute_filename_transition(source_type, target_type, class, b"other_file"),
            None,
        );
    }

    #[test]
    fn test_filename_transition_duplicate_key_error() {
        let mut bytes = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut bytes);
        2u32.serialize(&mut policy_writer).unwrap(); // array count = 2

        // entry 0
        4u32.serialize(&mut policy_writer).unwrap(); // filename len
        policy_writer.write_bytes(b"test");
        2u32.serialize(&mut policy_writer).unwrap(); // transition_type
        3u32.serialize(&mut policy_writer).unwrap(); // transition_class
        1u32.serialize(&mut policy_writer).unwrap(); // items count
        64u32.serialize(&mut policy_writer).unwrap(); // IdSet map_item_size_bits = 64
        64u32.serialize(&mut policy_writer).unwrap(); // IdSet high_bit = 64
        1u32.serialize(&mut policy_writer).unwrap(); // IdSet items_count = 1
        0u32.serialize(&mut policy_writer).unwrap(); // IdSet node start_bit = 0
        1u64.serialize(&mut policy_writer).unwrap(); // IdSet bit 0 (type 1)
        4u32.serialize(&mut policy_writer).unwrap(); // out_type

        // entry 1 (duplicate target_type=2, target_class=3, filename="test")
        4u32.serialize(&mut policy_writer).unwrap(); // filename len
        policy_writer.write_bytes(b"test");
        2u32.serialize(&mut policy_writer).unwrap(); // transition_type
        3u32.serialize(&mut policy_writer).unwrap(); // transition_class
        1u32.serialize(&mut policy_writer).unwrap(); // items count
        64u32.serialize(&mut policy_writer).unwrap(); // IdSet map_item_size_bits = 64
        64u32.serialize(&mut policy_writer).unwrap(); // IdSet high_bit = 64
        1u32.serialize(&mut policy_writer).unwrap(); // IdSet items_count = 1
        0u32.serialize(&mut policy_writer).unwrap(); // IdSet node start_bit = 0
        1u64.serialize(&mut policy_writer).unwrap(); // IdSet bit 0 (type 1)
        5u32.serialize(&mut policy_writer).unwrap(); // out_type

        let mut cursor = PolicyCursor::new(&bytes);
        cursor.set_policy_version(PolicyVersion::V33);
        let result = cursor.parse::<FilenameTransitions>();
        assert!(matches!(result, Err(ParseError::DuplicateFilenameTransition { .. })));
    }
}
