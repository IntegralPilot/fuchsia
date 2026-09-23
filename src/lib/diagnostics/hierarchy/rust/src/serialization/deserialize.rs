// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{ArrayContent, DiagnosticsHierarchy, ExponentialHistogram, LinearHistogram, Property};
use base64::engine::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::marker::PhantomData;
use std::str::FromStr;

#[cfg(feature = "json_schema")]
use schemars::schema::{
    InstanceType, Metadata, ObjectValidation, Schema, SchemaObject, SingleOrVec,
    SubschemaValidation,
};

struct RootVisitor<Key> {
    // Key is unused.
    marker: PhantomData<Key>,
}

impl<'de, Key> Visitor<'de> for RootVisitor<Key>
where
    Key: FromStr + Clone + Hash + Eq + AsRef<str>,
{
    type Value = DiagnosticsHierarchy<Key>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("there should be a single root")
    }

    fn visit_map<V>(self, mut map: V) -> Result<DiagnosticsHierarchy<Key>, V::Error>
    where
        V: MapAccess<'de>,
    {
        let result = match map.next_entry::<String, FieldValue<Key>>()? {
            Some((map_key, value)) => {
                let key = Key::from_str(&map_key)
                    .map_err(|_| de::Error::custom("failed to parse key"))?;
                value.into_node(&key)
            }
            None => return Err(de::Error::invalid_length(0, &"expected a root node")),
        };

        let mut found = 1;
        while map.next_key::<String>()?.is_some() {
            found += 1;
        }

        if found > 1 {
            return Err(de::Error::invalid_length(found, &"expected a single root"));
        }

        result.ok_or_else(|| de::Error::custom("expected node for root"))
    }
}

impl<'de, Key> Deserialize<'de> for DiagnosticsHierarchy<Key>
where
    Key: FromStr + Clone + Hash + Eq + AsRef<str>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(RootVisitor { marker: PhantomData })
    }
}

trait IntoProperty<Key> {
    fn into_property(self, key: &Key) -> Option<Property<Key>>;
}

/// The value of an inspect tree field (node or property).
enum FieldValue<Key> {
    String(String),
    Bytes(Vec<u8>),
    Int(i64),
    Uint(u64),
    Double(f64),
    Bool(bool),
    Array(Vec<NumericValue>),
    LinearIntHistogram(LinearHistogram<i64>),
    LinearUintHistogram(LinearHistogram<u64>),
    LinearDoubleHistogram(LinearHistogram<f64>),
    ExponentialIntHistogram(ExponentialHistogram<i64>),
    ExponentialUintHistogram(ExponentialHistogram<u64>),
    ExponentialDoubleHistogram(ExponentialHistogram<f64>),
    Node(HashMap<Key, FieldValue<Key>>),
    StringList(Vec<String>),
}

impl<Key: Clone> IntoProperty<Key> for FieldValue<Key> {
    fn into_property(self, key: &Key) -> Option<Property<Key>> {
        match self {
            Self::String(value) => Some(Property::String(key.clone(), value)),
            Self::Bytes(value) => Some(Property::Bytes(key.clone(), value)),
            Self::Int(value) => Some(Property::Int(key.clone(), value)),
            Self::Uint(value) => Some(Property::Uint(key.clone(), value)),
            Self::Double(value) => Some(Property::Double(key.clone(), value)),
            Self::Bool(value) => Some(Property::Bool(key.clone(), value)),
            Self::Array(values) => values.into_property(key),
            Self::ExponentialIntHistogram(histogram) => {
                Some(Property::IntArray(key.clone(), ArrayContent::ExponentialHistogram(histogram)))
            }
            Self::ExponentialUintHistogram(histogram) => Some(Property::UintArray(
                key.clone(),
                ArrayContent::ExponentialHistogram(histogram),
            )),
            Self::ExponentialDoubleHistogram(histogram) => Some(Property::DoubleArray(
                key.clone(),
                ArrayContent::ExponentialHistogram(histogram),
            )),
            Self::LinearIntHistogram(histogram) => {
                Some(Property::IntArray(key.clone(), ArrayContent::LinearHistogram(histogram)))
            }
            Self::LinearUintHistogram(histogram) => {
                Some(Property::UintArray(key.clone(), ArrayContent::LinearHistogram(histogram)))
            }
            Self::LinearDoubleHistogram(histogram) => {
                Some(Property::DoubleArray(key.clone(), ArrayContent::LinearHistogram(histogram)))
            }
            Self::StringList(list) => Some(Property::StringList(key.clone(), list)),
            Self::Node(_) => None,
        }
    }
}

impl<Key: AsRef<str> + Clone> FieldValue<Key> {
    fn is_property(&self) -> bool {
        !matches!(self, Self::Node(_))
    }

    fn into_node(self, key: &Key) -> Option<DiagnosticsHierarchy<Key>> {
        match self {
            Self::Node(map) => {
                let mut properties = vec![];
                let mut children = vec![];
                for (map_key, value) in map {
                    if value.is_property() {
                        properties.push(value.into_property(&map_key).unwrap());
                    } else {
                        children.push(value.into_node(&map_key).unwrap());
                    }
                }
                Some(DiagnosticsHierarchy::new(key.as_ref(), properties, children))
            }
            _ => None,
        }
    }
}

impl<'de, Key> Deserialize<'de> for FieldValue<Key>
where
    Key: FromStr + Hash + Eq,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(FieldVisitor { marker: PhantomData })
    }
}

struct FieldVisitor<Key> {
    marker: PhantomData<Key>,
}

// Histogram type may be: linear or exponential; double, int, or uint.
// If it's linear, step will be Some and initial_step and step_multiplier will be None.
// if it's exponential, step will be None and initial_step and step_multiplier will be Some.
// In all cases, size must be usize (u64).
// indexes will be either None or Some(Vec<NumericValue>) which must be convertible to Vec<usize>.
// counts will be Vec<NumericValue>.
// If the histogram is double, every number must be f64 and counts must convert to Vec<f64>.
// If it's not double, the numbers will be some mix of i64 and u64. If even one can't fit in
// an i64, then the histogram must be represented as u64 and none of the numbers can be
// negative.
// The deserializer will have used i64 for any number that fits, and we can assume most
// histograms will fit in i64, so first we check if all numbers are i64 and counts can convert
// to Vec<i64>. If not, then we have to check if a mix of i64 and u64 can all be u64.

fn value_as_u64<Key>(value: &FieldValue<Key>) -> Option<u64> {
    match value {
        FieldValue::Uint(value) => Some(*value),
        FieldValue::Int(value) => u64::try_from(*value).ok(),
        _ => None,
    }
}

fn value_as_i64<Key>(value: &FieldValue<Key>) -> Option<i64> {
    match value {
        FieldValue::Int(value) => Some(*value),
        FieldValue::Uint(value) => i64::try_from(*value).ok(),
        _ => None,
    }
}

// Try to convert indexes to Option<Vec<usize>> and declared_len to usize, and check consistency of
// several sizes. Return Err(()) if anything goes wrong, and the converted (indexes, declared_len)
// if it's all good,
fn sanitize_histogram_parameters<Key>(
    indexes: Option<&FieldValue<Key>>,
    n_parameters: usize,
    dict_len: usize,
    counts: &FieldValue<Key>,
    size: &FieldValue<Key>,
) -> Result<(Option<Vec<usize>>, usize), ()> {
    let size = match size {
        FieldValue::Uint(size) => *size as usize,
        FieldValue::Int(size) if *size >= 0 => *size as usize,
        _ => return Err(()),
    };
    // An empty array will be cast as a StringList. A condensed histogram of all-zeroes will thus
    // have empty StringList counts and indexes. If we sanitize successfully, we'll return an
    // empty Vec<usize> for indexes, and we will have verified that counts is empty too.
    let counts_len = match counts {
        FieldValue::Array(counts) => counts.len(),
        FieldValue::StringList(counts) if counts.is_empty() => 0,
        _ => return Err(()),
    };
    if size < 3 {
        // We need at least 3 (overflow + 1) buckets to be a valid histogram
        return Err(());
    }
    // It's OK if indexes is None. If it's Some, it must be a Vec<NumericValue> that converts to a
    // Vec<usize>. Also, check that various lengths are consistent.
    match indexes {
        None => {
            if counts_len != size || dict_len != n_parameters - 1 {
                return Err(());
            }
            Ok((None, size))
        }
        Some(FieldValue::StringList(indexes)) if indexes.is_empty() => {
            if counts_len != 0 || dict_len != n_parameters {
                return Err(());
            }
            Ok((Some(vec![]), size))
        }
        Some(FieldValue::Array(indexes)) => {
            if indexes.len() != counts_len || counts_len > size || dict_len != n_parameters {
                return Err(());
            }
            let indexes = indexes
                .iter()
                .map(|value| match value.as_u64() {
                    None => None,
                    Some(i) if (i as usize) < size => Some(i as usize),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>();
            match indexes {
                None => Err(()),
                Some(indexes) => Ok((Some(indexes), size)),
            }
        }
        _ => Err(()),
    }
}

fn match_linear_histogram<Key>(
    floor: &FieldValue<Key>,
    step: &FieldValue<Key>,
    counts: &FieldValue<Key>,
    indexes: Option<&FieldValue<Key>>,
    size: &FieldValue<Key>,
    dict_len: usize,
) -> Option<FieldValue<Key>> {
    let (indexes, size) = match sanitize_histogram_parameters(indexes, 5, dict_len, counts, size) {
        Ok((indexes, size)) => (indexes, size),
        Err(()) => return None,
    };
    // A double histogram will have all types double. An int or uint histogram may have a mix
    // of int and uint members.
    match (floor, step, counts) {
        (FieldValue::Double(floor), FieldValue::Double(step), counts) => {
            let counts = parse_f64_list(counts)?;
            Some(FieldValue::LinearDoubleHistogram(LinearHistogram {
                floor: *floor,
                step: *step,
                counts,
                indexes,
                size,
            }))
        }
        (floor, step, counts) => {
            let counts_i64 = parse_i64_list(counts);
            if let (Some(counts), Some(floor), Some(step)) =
                (counts_i64, value_as_i64(floor), value_as_i64(step))
            {
                return Some(FieldValue::LinearIntHistogram(LinearHistogram {
                    floor,
                    step,
                    counts,
                    indexes,
                    size,
                }));
            }
            // At this point, it's unsigned, or nothing.
            if let (Some(counts), Some(floor), Some(step)) =
                (parse_u64_list(counts), value_as_u64(floor), value_as_u64(step))
            {
                return Some(FieldValue::LinearUintHistogram(LinearHistogram {
                    floor,
                    step,
                    counts,
                    indexes,
                    size,
                }));
            }
            None
        }
    }
}

fn match_exponential_histogram<Key>(
    floor: &FieldValue<Key>,
    initial_step: &FieldValue<Key>,
    step_multiplier: &FieldValue<Key>,
    counts: &FieldValue<Key>,
    indexes: Option<&FieldValue<Key>>,
    size: &FieldValue<Key>,
    dict_len: usize,
) -> Option<FieldValue<Key>> {
    let (indexes, size) = match sanitize_histogram_parameters(indexes, 6, dict_len, counts, size) {
        Ok((indexes, size)) => (indexes, size),
        Err(()) => return None,
    };
    match (floor, initial_step, step_multiplier, counts) {
        (
            FieldValue::Double(floor),
            FieldValue::Double(initial_step),
            FieldValue::Double(step_multiplier),
            counts,
        ) => {
            let counts = parse_f64_list(counts)?;
            Some(FieldValue::ExponentialDoubleHistogram(ExponentialHistogram {
                floor: *floor,
                initial_step: *initial_step,
                step_multiplier: *step_multiplier,
                counts,
                indexes,
                size,
            }))
        }
        (floor, initial_step, step_multiplier, counts) => {
            let counts_i64 = parse_i64_list(counts);
            if let (Some(counts), Some(floor), Some(initial_step), Some(step_multiplier)) = (
                counts_i64,
                value_as_i64(floor),
                value_as_i64(initial_step),
                value_as_i64(step_multiplier),
            ) {
                return Some(FieldValue::ExponentialIntHistogram(ExponentialHistogram {
                    floor,
                    initial_step,
                    step_multiplier,
                    counts,
                    indexes,
                    size,
                }));
            }
            // At this point, it's unsigned, or nothing.
            if let (Some(counts), Some(floor), Some(initial_step), Some(step_multiplier)) = (
                parse_u64_list(counts),
                value_as_u64(floor),
                value_as_u64(initial_step),
                value_as_u64(step_multiplier),
            ) {
                return Some(FieldValue::ExponentialUintHistogram(ExponentialHistogram {
                    floor,
                    initial_step,
                    step_multiplier,
                    counts,
                    indexes,
                    size,
                }));
            }
            None
        }
    }
}

fn match_histogram<Key>(dict: &HashMap<Key, FieldValue<Key>>) -> Option<FieldValue<Key>>
where
    Key: FromStr + Hash + Eq,
{
    // Quick checks for efficiency - most maps won't be histograms.
    let dict_len = dict.len();
    if !(4..=6).contains(&dict_len) {
        return None;
    }
    let get_key = |name: &str| -> Option<&FieldValue<Key>> {
        let key = Key::from_str(name).ok()?;
        dict.get(&key)
    };
    let floor = get_key("floor")?;
    let step = get_key("step");
    let initial_step = get_key("initial_step");
    let step_multiplier = get_key("step_multiplier");
    let counts = get_key("counts");
    let indexes = get_key("indexes");
    let size = get_key("size");
    // Indexes may be None if the histogram isn't condensed.
    match (step, initial_step, step_multiplier, counts, indexes, size) {
        (Some(step), None, None, Some(counts), indexes, Some(size)) => {
            match_linear_histogram(floor, step, counts, indexes, size, dict_len)
        }
        (None, Some(initial_step), Some(step_multiplier), Some(counts), indexes, Some(size)) => {
            match_exponential_histogram(
                floor,
                initial_step,
                step_multiplier,
                counts,
                indexes,
                size,
                dict_len,
            )
        }
        _ => None,
    }
}

impl<'de, Key> Visitor<'de> for FieldVisitor<Key>
where
    Key: FromStr + Hash + Eq,
{
    type Value = FieldValue<Key>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("failed to field")
    }

    fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
    where
        V: MapAccess<'de>,
    {
        let mut entries = vec![];
        while let Some(entry) = map.next_entry::<String, FieldValue<Key>>()? {
            entries.push(entry);
        }

        let node = entries
            .into_iter()
            .map(|(key, value)| Key::from_str(&key).map(|key| (key, value)))
            .collect::<Result<HashMap<Key, FieldValue<Key>>, _>>()
            .map_err(|_| de::Error::custom("failed to parse key"))?;
        if let Some(histogram) = match_histogram(&node) {
            Ok(histogram)
        } else {
            Ok(FieldValue::Node(node))
        }
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(FieldValue::Int(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(FieldValue::Uint(value))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(FieldValue::Double(value))
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(FieldValue::Bool(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.starts_with("b64:") {
            let bytes64 = value.replace("b64:", "");
            let bytes = BASE64_STANDARD
                .decode(&bytes64)
                .map_err(|_| de::Error::custom("failed to decode bytes"))?;
            return Ok(FieldValue::Bytes(bytes));
        }
        Ok(FieldValue::String(value.to_string()))
    }

    fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
    where
        S: SeqAccess<'de>,
    {
        let mut result = vec![];
        while let Some(elem) = seq.next_element::<SeqItem>()? {
            result.push(elem);
        }
        // There can be two types of sequences: regular arrays (containing numeric values),
        // and string lists (containing only strings). There cannot be a
        // sequence containing a mix of them.
        let mut array = vec![];
        let mut strings = vec![];
        for item in result {
            match item {
                SeqItem::Value(x) => array.push(x),
                SeqItem::StringValue(x) => strings.push(x),
            }
        }

        match (!array.is_empty(), !strings.is_empty()) {
            (true, false) => Ok(FieldValue::Array(array)),
            (false, _) => {
                // Numeric arrays cannot be empty, but string lists can.
                // Histograms can contain empty arrays of numbers, but we'll check for empty
                // StringList in the histogram-matching code, and know what it means.
                Ok(FieldValue::StringList(strings))
            }
            _ => Err(de::Error::custom("unexpected sequence containing mixed values")),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SeqItem {
    Value(NumericValue),
    StringValue(String),
}

enum NumericValue {
    Positive(u64),
    Negative(i64),
    Double(f64),
}

impl NumericValue {
    #[inline]
    fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Positive(x) if *x <= i64::MAX as u64 => Some(*x as i64),
            Self::Negative(x) => Some(*x),
            _ => None,
        }
    }

    #[inline]
    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Positive(x) => Some(*x),
            _ => None,
        }
    }

    #[inline]
    fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Double(x) => Some(*x),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for NumericValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NumericValueVisitor)
    }
}

impl<Key: Clone> IntoProperty<Key> for Vec<NumericValue> {
    fn into_property(self, key: &Key) -> Option<Property<Key>> {
        if let Some(values) = parse_f64_vec(&self) {
            return Some(Property::DoubleArray(key.clone(), ArrayContent::Values(values)));
        }
        if let Some(values) = parse_i64_vec(&self) {
            return Some(Property::IntArray(key.clone(), ArrayContent::Values(values)));
        }
        if let Some(values) = parse_u64_vec(&self) {
            return Some(Property::UintArray(key.clone(), ArrayContent::Values(values)));
        }
        None
    }
}

macro_rules! parse_numeric_vec_impls {
    ($($type:ty),*) => {
        $(
            paste::paste! {
                fn [<parse_ $type _vec>](vec: &[NumericValue]) -> Option<Vec<$type>> {
                    vec.iter().map(|value| value.[<as_ $type>]()).collect::<Option<Vec<_>>>()
                }

                // Histograms can contain empty lists for counts and indexes. These will be
                // deserialized as empty StringLists and should parse to empty vecs
                // of any numeric type.
                fn [<parse_ $type _list>]<Key>(list: &FieldValue<Key>) -> Option<Vec<$type>> {
                    match list {
                        FieldValue::StringList(list) if list.len() == 0 => Some(vec![]),
                        FieldValue::Array(vec) => [<parse_ $type _vec>](vec),
                        _ => None,
                    }
                }
            }
        )*
    };
}

// Generates the following functions:
// fn parse_f64_vec()
// fn parse_u64_vec()
// fn parse_i64_vec()
// fn parse_f64_list()
// fn parse_u64_list()
// fn parse_i64_list()
parse_numeric_vec_impls!(f64, u64, i64);

struct NumericValueVisitor;

impl Visitor<'_> for NumericValueVisitor {
    type Value = NumericValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("failed to deserialize bucket array")
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        if value < 0 {
            return Ok(NumericValue::Negative(value));
        }
        Ok(NumericValue::Positive(value as u64))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(NumericValue::Positive(value))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(NumericValue::Double(value))
    }
}

// Due to the custom encoding/decoding in use, the json schema does not adhere to the structure of
// the rust definition, so must be implemented manually here.
#[cfg(feature = "json_schema")]
impl<T> schemars::JsonSchema for DiagnosticsHierarchy<T> {
    fn schema_name() -> String {
        "DiagnosticsHierarchy".to_owned()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> Schema {
        let property_schema = SchemaObject {
            metadata: Some(Box::new(Metadata {
                description: Some(
                    "A property, which can be any standard object, or an array, or a histogram"
                        .to_owned(),
                ),
                ..Default::default()
            })),
            subschemas: Some(Box::new(SubschemaValidation {
                any_of: Some(vec![
                    String::json_schema(generator),
                    Vec::<f64>::json_schema(generator),
                    Vec::<i64>::json_schema(generator),
                    Vec::<u64>::json_schema(generator),
                    Vec::<String>::json_schema(generator),
                    LinearHistogram::<u64>::json_schema(generator),
                    LinearHistogram::<i64>::json_schema(generator),
                    LinearHistogram::<f64>::json_schema(generator),
                    ExponentialHistogram::<u64>::json_schema(generator),
                    ExponentialHistogram::<i64>::json_schema(generator),
                    ExponentialHistogram::<f64>::json_schema(generator),
                    i64::json_schema(generator),
                    u64::json_schema(generator),
                    f64::json_schema(generator),
                    bool::json_schema(generator),
                    // Includes a recursive self-reference.
                    SchemaObject {
                        instance_type: Some(SingleOrVec::Single(Box::new(InstanceType::Object))),
                        reference: Some(format!("#/definitions/{}", Self::schema_name())),
                        ..Default::default()
                    }
                    .into(),
                ]),
                ..Default::default()
            })),
            ..Default::default()
        };
        SchemaObject {
            instance_type: Some(SingleOrVec::Single(Box::new(InstanceType::Object))),
            object: Some(Box::new(ObjectValidation {
                min_properties: Some(1),
                pattern_properties: [("^.*$".to_owned(), property_schema.into())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            })),
            ..Default::default()
        }
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArrayFormat;

    #[cfg(feature = "json_schema")]
    use ffx_writer::VerifiedMachineWriter;

    #[fuchsia::test]
    fn deserialize_json() {
        let json_string = get_single_json_hierarchy();
        let mut parsed_hierarchy: DiagnosticsHierarchy =
            serde_json::from_str(&json_string).expect("deserialized");
        let mut expected_hierarchy = get_unambigious_deserializable_hierarchy();
        parsed_hierarchy.sort();
        expected_hierarchy.sort();
        assert_eq!(expected_hierarchy, parsed_hierarchy);
    }

    #[cfg(feature = "json_schema")]
    #[fuchsia::test]
    fn verify_schema() {
        let json_string = get_single_json_hierarchy();
        let data: serde_json::Value =
            serde_json::from_str(&json_string).expect("valid json string");
        let root = data.get("root").expect("expected root node");
        if let Err(e) = VerifiedMachineWriter::<DiagnosticsHierarchy>::verify_schema(root) {
            panic!("Error verifying schema of `{data:#?}`: {e:#?}")
        }
    }

    #[fuchsia::test]
    fn reversible_deserialize() {
        let mut original_hierarchy = get_unambigious_deserializable_hierarchy();
        let result =
            serde_json::to_string(&original_hierarchy).expect("failed to format hierarchy");
        let mut parsed_hierarchy: DiagnosticsHierarchy =
            serde_json::from_str(&result).expect("deserialized");
        parsed_hierarchy.sort();
        original_hierarchy.sort();
        assert_eq!(original_hierarchy, parsed_hierarchy);
    }

    #[fuchsia::test]
    fn test_exp_histogram() {
        let mut hierarchy = DiagnosticsHierarchy::new(
            "root".to_string(),
            vec![Property::IntArray(
                "histogram".to_string(),
                ArrayContent::new(
                    vec![1000, 1000, 2, 1, 2, 3, 4, 5, 6],
                    ArrayFormat::ExponentialHistogram,
                )
                .unwrap(),
            )],
            vec![],
        );
        let expected_json = serde_json::json!({
            "root": {
                "histogram": {
                    "floor": 1000,
                    "initial_step": 1000,
                    "step_multiplier": 2,
                    "counts": [1, 2, 3, 4, 5, 6],
                    "size": 6
                }
            }
        });
        let result_json = serde_json::json!(hierarchy);
        assert_eq!(result_json, expected_json);
        let mut parsed_hierarchy: DiagnosticsHierarchy =
            serde_json::from_value(result_json).expect("deserialized");
        parsed_hierarchy.sort();
        hierarchy.sort();
        assert_eq!(hierarchy, parsed_hierarchy);
    }

    // Creates a hierarchy that isn't lossy due to its unambigious values.
    fn get_unambigious_deserializable_hierarchy() -> DiagnosticsHierarchy {
        DiagnosticsHierarchy::new(
            "root",
            vec![
                Property::UintArray(
                    "array".to_string(),
                    ArrayContent::Values(vec![0, 2, u64::MAX]),
                ),
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
                                vec![0.0, 2.0, 4.0, 1.0, 3.0, 4.0, 7.0],
                                ArrayFormat::ExponentialHistogram,
                            )
                            .unwrap(),
                        ),
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
                    ],
                    vec![],
                ),
            ],
        )
    }

    pub fn get_single_json_hierarchy() -> String {
        "{ \"root\": {
                \"a\": {
                    \"double\": 2.5,
                    \"histogram\": {
                        \"floor\": 0.0,
                        \"initial_step\": 2.0,
                        \"step_multiplier\": 4.0,
                        \"counts\": [1.0, 3.0, 4.0, 7.0],
                        \"size\": 4
                    }
                },
                \"array\": [
                    0,
                    2,
                    18446744073709551615
                ],
                \"string_list\": [\"foo\", \"bar\"],
                \"empty_string_list\": [],
                \"b\": {
                    \"histogram\": {
                        \"floor\": 0,
                        \"step\": 2,
                        \"counts\": [4, 1, 3],
                        \"size\": 3
                    },
                    \"int\": -2,
                    \"string\": \"some value\"
                },
                \"bool_false\": false,
                \"bool_true\": true
            }}"
        .to_string()
    }
}
