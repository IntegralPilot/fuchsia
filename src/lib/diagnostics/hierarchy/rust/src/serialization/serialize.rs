// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{ArrayContent, DiagnosticsHierarchy, ExponentialHistogram, LinearHistogram, Property};
use base64::engine::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use either::Either;
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use std::collections::BTreeSet;

impl<Key> Serialize for DiagnosticsHierarchy<Key>
where
    Key: AsRef<str>,
{
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_map(Some(1))?;
        let name = self.name.clone();
        s.serialize_entry(&name, &SerializableHierarchyFields { hierarchy: self, moniker: None })?;
        s.end()
    }
}

pub struct SerializableHierarchyFields<'a, Key> {
    pub hierarchy: &'a DiagnosticsHierarchy<Key>,
    pub moniker: Option<&'a str>,
}

fn get_name<'a, K: AsRef<str>>(a: Either<&'a Property<K>, &'a DiagnosticsHierarchy<K>>) -> &'a str {
    match a {
        Either::Left(property) => property.name(),
        Either::Right(node) => &node.name,
    }
}

impl<Key> Serialize for SerializableHierarchyFields<'_, Key>
where
    Key: AsRef<str>,
{
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let items = self.hierarchy.properties.len() + self.hierarchy.children.len();
        let mut s = serializer.serialize_map(Some(items))?;
        let mut it = self
            .hierarchy
            .properties
            .iter()
            .map(Either::Left)
            .chain(self.hierarchy.children.iter().map(Either::Right));
        // From analysis of Inspect files, 99.5% of nodes have =< 16 children.
        let mut seen_names = if items >= 16 { Some(BTreeSet::new()) } else { None };
        while let Some(val) = it.next() {
            let name = get_name(val);
            let is_duplicate = if let Some(ref mut seen) = seen_names {
                !seen.insert(name)
            } else {
                it.clone().any(|a| get_name(a) == name)
            };

            if is_duplicate {
                // TODO(520049138): debug rather than warning until serial spam is resolved.
                log::debug!(
                    item_name:? = name,
                    emitting_component:? = self.moniker;
                    "Encountered duplicate names while serializing Inspect"
                );
            }

            match val {
                Either::Left(property) => {
                    let name = property.name();
                    match property {
                        Property::String(_, value) => s.serialize_entry(name, &value)?,
                        Property::Int(_, value) => s.serialize_entry(name, &value)?,
                        Property::Uint(_, value) => s.serialize_entry(name, &value)?,
                        Property::Double(_, value) => {
                            let value = if value.is_nan()
                                || (value.is_infinite() && value.is_sign_positive())
                            {
                                f64::MAX
                            } else if value.is_infinite() && value.is_sign_negative() {
                                f64::MIN
                            } else {
                                *value
                            };
                            s.serialize_entry(name, &value)?;
                        }
                        Property::Bool(_, value) => s.serialize_entry(name, &value)?,
                        Property::Bytes(_, array) => s.serialize_entry(
                            name,
                            &format!("b64:{}", BASE64_STANDARD.encode(array)),
                        )?,
                        Property::DoubleArray(_, array) => {
                            s.serialize_entry(name, &array)?;
                        }
                        Property::IntArray(_, array) => {
                            s.serialize_entry(name, &array)?;
                        }
                        Property::UintArray(_, array) => {
                            s.serialize_entry(name, &array)?;
                        }
                        Property::StringList(_, list) => {
                            s.serialize_entry(name, &list)?;
                        }
                    }
                }
                Either::Right(child) => s.serialize_entry(
                    &child.name,
                    &SerializableHierarchyFields { hierarchy: child, moniker: self.moniker },
                )?,
            }
        }
        s.end()
    }
}

// A condensed histogram has a vec of counts and a vec of corresponding indexes.
// For serialization, histograms should be condensed if fewer than 50% of the
// counts are nonzero. We don't worry about decondensing histograms with more
// than 50% nonzero counts - they'd still be correct, but we shouldn't get them.
pub(crate) fn maybe_condense_histogram<T>(
    counts: &[T],
    indexes: &Option<Vec<usize>>,
) -> Option<(Vec<T>, Vec<usize>)>
where
    T: PartialEq + num_traits::Zero + Copy,
{
    if indexes.is_some() {
        return None;
    }
    let mut condensed_counts = vec![];
    let mut indexes = vec![];
    let cutoff_len = counts.len() / 2;
    for (index, count) in counts.iter().enumerate() {
        if *count != T::zero() {
            indexes.push(index);
            condensed_counts.push(*count);
            if condensed_counts.len() > cutoff_len {
                return None;
            }
        }
    }
    Some((condensed_counts, indexes))
}

macro_rules! impl_serialize_for_array_value {
    ($($type:ty,)*) => {
        $(
            impl Serialize for ArrayContent<$type> {
                fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    match self {
                        ArrayContent::LinearHistogram(LinearHistogram {
                            floor,
                            step,
                            counts,
                            indexes,
                            size,
                        }) => {
                            let condensation = maybe_condense_histogram(counts, indexes);
                            let (counts, indexes) = match condensation {
                                None => (counts.to_vec(), indexes.as_ref().map(|v| v.to_vec())),
                                Some((cc, ci)) => (cc, Some(ci)),
                            };
                            LinearHistogram {
                                floor: *floor,
                                step: *step,
                                counts,
                                indexes,
                                size: *size,
                            }.serialize(serializer)
                        }
                        ArrayContent::ExponentialHistogram(ExponentialHistogram {
                            floor,
                            initial_step,
                            step_multiplier,
                            counts,
                            indexes,
                            size,
                        }) => {
                            let condensation = maybe_condense_histogram(counts, indexes);
                            let (counts, indexes) = match condensation {
                                None => (counts.to_vec(), indexes.as_ref().map(|v| v.to_vec())),
                                Some((cc, ci)) => (cc, Some(ci)),
                            };
                            ExponentialHistogram {
                                floor: *floor,
                                size: *size,
                                initial_step: *initial_step,
                                step_multiplier: *step_multiplier,
                                counts,
                                indexes,
                            }
                            .serialize(serializer)
                        }
                        ArrayContent::Values(values) => {
                            let mut s = serializer.serialize_seq(Some(values.len()))?;
                            for value in values {
                                s.serialize_element(&value)?;
                            }
                            s.end()
                        }
                    }
                }
            }
        )*
    }
}

impl_serialize_for_array_value![i64, u64, f64,];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArrayFormat, hierarchy};

    #[fuchsia::test]
    fn serialize_json() {
        let mut hierarchy = test_hierarchy();
        hierarchy.sort();
        let expected = expected_json();
        let result = serde_json::to_string_pretty(&hierarchy).expect("failed to serialize");
        let parsed_json_expected: serde_json::Value = serde_json::from_str(&expected).unwrap();
        let parsed_json_result: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed_json_result, parsed_json_expected);
    }

    #[fuchsia::test]
    fn serialize_doubles() {
        let hierarchy = hierarchy! {
            root: {
                inf: f64::INFINITY,
                neg_inf: f64::NEG_INFINITY,
                nan: f64::NAN,
            }
        };
        let result = serde_json::to_string_pretty(&hierarchy).expect("serialized");
        assert_eq!(
            result,
            r#"{
  "root": {
    "inf": 1.7976931348623157e+308,
    "neg_inf": -1.7976931348623157e+308,
    "nan": 1.7976931348623157e+308
  }
}"#
        );
    }

    fn test_hierarchy() -> DiagnosticsHierarchy {
        DiagnosticsHierarchy::new(
            "root",
            vec![
                Property::UintArray("array".to_string(), ArrayContent::Values(vec![0, 2, 4])),
                Property::Bool("bool_true".to_string(), true),
                Property::Bool("bool_false".to_string(), false),
                Property::StringList(
                    "string_list".to_string(),
                    vec!["foo".to_string(), "bar".to_string()],
                ),
                Property::StringList("empty_string_list".to_string(), vec![]),
            ],
            vec![
                DiagnosticsHierarchy::new(
                    "a",
                    vec![
                        Property::Double("double".to_string(), 2.5),
                        Property::DoubleArray(
                            "histogram".to_string(),
                            ArrayContent::new(
                                vec![0.0, 2.0, 4.0, 1.0, 3.0, 4.0],
                                ArrayFormat::ExponentialHistogram,
                            )
                            .unwrap(),
                        ),
                        Property::Bytes("bytes".to_string(), vec![5u8, 0xf1, 0xab]),
                    ],
                    vec![],
                ),
                DiagnosticsHierarchy::new(
                    "b",
                    vec![
                        Property::Int("int".to_string(), -2),
                        Property::String("string".to_string(), "some value".to_string()),
                        Property::IntArray(
                            "histogram".to_string(),
                            ArrayContent::new(vec![0, 2, 4, 1, 3], ArrayFormat::LinearHistogram)
                                .unwrap(),
                        ),
                        Property::IntArray(
                            "condensed_histogram".to_string(),
                            ArrayContent::new(vec![0, 2, 0, 1, 0], ArrayFormat::LinearHistogram)
                                .unwrap(),
                        ),
                    ],
                    vec![],
                ),
            ],
        )
    }

    fn expected_json() -> String {
        r#"{
  "root": {
    "array": [
      0,
      2,
      4
    ],
    "bool_false": false,
    "bool_true": true,
    "empty_string_list": [],
    "string_list": [
      "foo",
      "bar"
    ],
    "a": {
      "bytes": "b64:BfGr",
      "double": 2.5,
      "histogram": {
        "size": 3,
        "floor": 0.0,
        "initial_step": 2.0,
        "step_multiplier": 4.0,
        "counts": [1.0, 3.0, 4.0]
      }
    },
    "b": {
      "histogram": {
        "floor": 0,
        "step": 2,
        "counts": [
          4,
          1,
          3
        ],
        "size": 3
      },
    "condensed_histogram": {
        "size": 3,
        "floor": 0,
        "step": 2,
        "counts": [
          1
        ],
        "indexes": [
          1
        ]
      },
      "int": -2,
      "string": "some value"
    }
  }
}"#
        .to_string()
    }
}
