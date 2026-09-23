// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A library of common utilities used by `cmc` and related tools.
//! To manually regenerate reference documentation from doc comments in
//! this file, see the instructions at:
//!
//!   tools/lib/reference_doc/macro/derive-reference-doc-tests/src/test_data/README.md

pub mod error;
pub mod features;
pub mod load;
pub mod one_or_many;
pub mod types;
pub(crate) mod validate;

#[allow(unused)] // A test-only macro is defined outside of a test builds.
pub mod translate;

use crate::error::Error;
use cml_macro::{OneOrMany, Reference};
use json5format::{FormatOptions, PathOption};
use maplit::{hashmap, hashset};
use serde::{Deserialize, Serialize, de, ser};
use std::fmt;
use std::hash::Hash;
use std::num::NonZeroU32;
use std::str::FromStr;
use std::sync::Arc;

pub use crate::types::capability::{Capability, CapabilityFromRef, ContextCapability};
pub use crate::types::capability_id::CapabilityId;
pub use crate::types::child::Child;
pub use crate::types::collection::Collection;
use crate::types::common::{ContextCapabilityClause, ContextPathClause, ContextSpanned};
pub use crate::types::document::{Document, DocumentContext, parse_and_hydrate};
pub use crate::types::environment::{Environment, ResolverRegistration};
pub use crate::types::expose::{ContextExpose, Expose};
pub use crate::types::offer::{Offer, OfferFromRef, OfferToAllCapability, OfferToRef};
pub use crate::types::program::Program;
pub use crate::types::r#use::{Use, UseFromRef};

pub use cm_types::{
    AllowedOffers, Availability, BorrowedName, BoundedName, DeliveryType, DependencyType,
    Durability, HandleType, Name, NamespacePath, OnTerminate, ParseError, Path, RelativePath,
    StartupMode, StorageId, Url,
};
use error::Location;

pub use crate::one_or_many::OneOrMany;
pub use crate::translate::{CompileOptions, compile};
pub use crate::validate::{CapabilityRequirements, MustUseRequirement};

/// Parses a string `buffer` into a [Document]. `file` is used for error reporting.
pub fn parse_one_document(buffer: &String, file: &std::path::Path) -> Result<Document, Error> {
    serde_json5::from_str(&buffer).map_err(|e| {
        let serde_json5::Error::Message { location, msg } = e;
        let location = location.map(|l| Location { line: l.line, column: l.column });
        Error::parse(msg, location, Some(file))
    })
}

pub fn load_cml_with_context(
    buffer: &String,
    file: &std::path::Path,
) -> Result<DocumentContext, Error> {
    let file_arc = Arc::from(file);
    parse_and_hydrate(file_arc, buffer)
}

/// Generates deserializer for `OneOrMany<Name>`.
#[derive(OneOrMany, Debug, Clone)]
#[one_or_many(
    expected = "a name or nonempty array of names, with unique elements",
    inner_type = "Name",
    min_length = 1,
    unique_items = true
)]
pub struct OneOrManyNames;

/// Generates deserializer for `OneOrMany<Path>`.
#[derive(OneOrMany, Debug, Clone)]
#[one_or_many(
    expected = "a path or nonempty array of paths, with unique elements",
    inner_type = "Path",
    min_length = 1,
    unique_items = true
)]
pub struct OneOrManyPaths;

/// Generates deserializer for `OneOrMany<EventScope>`.
#[derive(OneOrMany, Debug, Clone)]
#[one_or_many(
    expected = "one or an array of \"#<collection-name>\", or \"#<child-name>\"",
    inner_type = "EventScope",
    min_length = 1,
    unique_items = true
)]
pub struct OneOrManyEventScope;

/// A reference in an `offer to` or `exose to`.
#[derive(Debug, Deserialize, PartialEq, Eq, Hash, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceAvailability {
    Required,
    Unknown,
}

impl Default for SourceAvailability {
    fn default() -> Self {
        Self::Required
    }
}

impl<T> CanonicalizeContext for Vec<T>
where
    T: CanonicalizeContext + ContextCapabilityClause + ContextPathClause + Clone + PartialEq,
{
    fn canonicalize_context(&mut self) {
        let mut to_merge: Vec<(T, Vec<ContextSpanned<Name>>)> = Vec::new();
        let mut to_keep: Vec<T> = vec![];

        self.iter().for_each(|c| {
            // Any entry with a `path` set cannot be merged with another.
            if !c.are_many_names_allowed() || c.path().is_some() {
                to_keep.push(c.clone());
                return;
            }

            let mut names = c.names();
            let mut copy: T = c.clone();

            let synthetic_name = Name::from_str("a").unwrap();
            let spanned = ContextSpanned { value: synthetic_name, origin: c.origin().clone() };
            copy.set_names(vec![spanned]);

            let r = to_merge.iter().position(|(t, _)| t == &copy);
            match r {
                Some(i) => to_merge[i].1.append(&mut names),
                None => to_merge.push((copy, names)),
            };
        });

        let mut merged = to_merge
            .into_iter()
            .map(|(mut t, mut names)| {
                names.sort_by(|a, b| a.value.cmp(&b.value));

                t.set_names(names);
                t
            })
            .collect::<Vec<_>>();

        to_keep.append(&mut merged);
        *self = to_keep;

        self.iter_mut().for_each(|c| c.canonicalize_context());

        self.sort_by(|a, b| {
            // Sort by capability type string first
            let a_type = a.capability_type(None).unwrap();
            let b_type = b.capability_type(None).unwrap();

            a_type.cmp(b_type).then_with(|| {
                let a_names = a.names();
                let b_names = b.names();

                // Compare the first name in the list (e.g., "fuchsia.logger.Log") ignoring Origin differences.
                let a_first_val = &a_names.first().unwrap().value;
                let b_first_val = &b_names.first().unwrap().value;

                a_first_val.cmp(b_first_val)
            })
        });
    }
}

/// A relative reference to another object. This is a generic type that can encode any supported
/// reference subtype. For named references, it holds a reference to the name instead of the name
/// itself.
///
/// Objects of this type are usually derived from conversions of context-specific reference
/// types that `#[derive(Reference)]`. This type makes it easy to write helper functions that operate on
/// generic references.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum AnyRef<'a> {
    /// A named reference. Parsed as `#name`.
    Named(&'a BorrowedName),
    /// A reference to the parent. Parsed as `parent`.
    Parent,
    /// A reference to the framework (component manager). Parsed as `framework`.
    Framework,
    /// A reference to the debug. Parsed as `debug`.
    Debug,
    /// A reference to this component. Parsed as `self`.
    Self_,
    /// An intentionally omitted reference.
    Void,
    /// A reference to a dictionary. Parsed as a dictionary path.
    Dictionary(&'a DictionaryRef),
    /// A reference to a dictionary defined by this component. Parsed as
    /// `self/<dictionary>`.
    OwnDictionary(&'a BorrowedName),
}

/// Format an `AnyRef` as a string.
impl fmt::Display for AnyRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(name) => write!(f, "#{}", name),
            Self::Parent => write!(f, "parent"),
            Self::Framework => write!(f, "framework"),
            Self::Debug => write!(f, "debug"),
            Self::Self_ => write!(f, "self"),
            Self::Void => write!(f, "void"),
            Self::Dictionary(d) => write!(f, "{}", d),
            Self::OwnDictionary(name) => write!(f, "self/{}", name),
        }
    }
}

/// A reference to a (possibly nested) dictionary.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct DictionaryRef {
    /// Path to the dictionary relative to `root_dictionary`.
    pub path: RelativePath,
    pub root: RootDictionaryRef,
}

impl<'a> From<&'a DictionaryRef> for AnyRef<'a> {
    fn from(r: &'a DictionaryRef) -> Self {
        Self::Dictionary(r)
    }
}

impl<'a> From<&'a Name> for AnyRef<'a> {
    fn from(name: &'a Name) -> Self {
        AnyRef::Named(name.as_ref())
    }
}

impl<'a> From<&'a BorrowedName> for AnyRef<'a> {
    fn from(name: &'a BorrowedName) -> Self {
        AnyRef::Named(name)
    }
}

impl FromStr for DictionaryRef {
    type Err = ParseError;

    fn from_str(path: &str) -> Result<Self, ParseError> {
        match path.find('/') {
            Some(n) => {
                let root = path[..n].parse().map_err(|_| ParseError::InvalidValue)?;
                let path = RelativePath::new(&path[n + 1..])?;
                Ok(Self { root, path })
            }
            None => Err(ParseError::InvalidValue),
        }
    }
}

impl fmt::Display for DictionaryRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.root, self.path)
    }
}

impl ser::Serialize for DictionaryRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        format!("{}", self).serialize(serializer)
    }
}

const DICTIONARY_REF_EXPECT_STR: &str = "a path to a dictionary no more \
    than 4095 characters in length";

impl<'de> de::Deserialize<'de> for DictionaryRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = DictionaryRef;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(DICTIONARY_REF_EXPECT_STR)
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                s.parse().map_err(|err| match err {
                    ParseError::InvalidValue => {
                        E::invalid_value(de::Unexpected::Str(s), &DICTIONARY_REF_EXPECT_STR)
                    }
                    ParseError::TooLong | ParseError::Empty => {
                        E::invalid_length(s.len(), &DICTIONARY_REF_EXPECT_STR)
                    }
                    e => {
                        panic!("unexpected parse error: {:?}", e);
                    }
                })
            }
        }

        deserializer.deserialize_string(Visitor)
    }
}

/// A reference to a root dictionary.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Reference)]
#[reference(expected = "\"parent\", \"self\", \"#<child-name>\"")]
pub enum RootDictionaryRef {
    /// A reference to a child.
    Named(Name),
    /// A reference to the parent.
    Parent,
    /// A reference to this component.
    Self_,
}

/// The scope of an event.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Reference, Ord, PartialOrd)]
#[reference(expected = "\"#<collection-name>\", \"#<child-name>\", or none")]
pub enum EventScope {
    /// A reference to a child or a collection.
    Named(Name),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfigType {
    Bool,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Int8,
    Int16,
    Int32,
    Int64,
    String,
    Vector,
}

impl From<&cm_rust::ConfigValueType> for ConfigType {
    fn from(value: &cm_rust::ConfigValueType) -> Self {
        match value {
            cm_rust::ConfigValueType::Bool => ConfigType::Bool,
            cm_rust::ConfigValueType::Uint8 => ConfigType::Uint8,
            cm_rust::ConfigValueType::Int8 => ConfigType::Int8,
            cm_rust::ConfigValueType::Uint16 => ConfigType::Uint16,
            cm_rust::ConfigValueType::Int16 => ConfigType::Int16,
            cm_rust::ConfigValueType::Uint32 => ConfigType::Uint32,
            cm_rust::ConfigValueType::Int32 => ConfigType::Int32,
            cm_rust::ConfigValueType::Uint64 => ConfigType::Uint64,
            cm_rust::ConfigValueType::Int64 => ConfigType::Int64,
            cm_rust::ConfigValueType::String { .. } => ConfigType::String,
            cm_rust::ConfigValueType::Vector { .. } => ConfigType::Vector,
        }
    }
}

#[derive(Clone, Deserialize, Debug, PartialEq, Serialize)]
#[serde(tag = "type", deny_unknown_fields, rename_all = "lowercase")]
pub enum ConfigNestedValueType {
    Bool {},
    Uint8 {},
    Uint16 {},
    Uint32 {},
    Uint64 {},
    Int8 {},
    Int16 {},
    Int32 {},
    Int64 {},
    String { max_size: NonZeroU32 },
}

impl ConfigNestedValueType {
    /// Update the hasher by digesting the ConfigVectorElementType enum value
    pub fn update_digest(&self, hasher: &mut impl sha2::Digest) {
        let val = match self {
            ConfigNestedValueType::Bool {} => 0u8,
            ConfigNestedValueType::Uint8 {} => 1u8,
            ConfigNestedValueType::Uint16 {} => 2u8,
            ConfigNestedValueType::Uint32 {} => 3u8,
            ConfigNestedValueType::Uint64 {} => 4u8,
            ConfigNestedValueType::Int8 {} => 5u8,
            ConfigNestedValueType::Int16 {} => 6u8,
            ConfigNestedValueType::Int32 {} => 7u8,
            ConfigNestedValueType::Int64 {} => 8u8,
            ConfigNestedValueType::String { max_size } => {
                hasher.update(max_size.get().to_le_bytes());
                9u8
            }
        };
        hasher.update([val])
    }
}

impl From<ConfigNestedValueType> for cm_rust::ConfigNestedValueType {
    fn from(value: ConfigNestedValueType) -> Self {
        match value {
            ConfigNestedValueType::Bool {} => cm_rust::ConfigNestedValueType::Bool,
            ConfigNestedValueType::Uint8 {} => cm_rust::ConfigNestedValueType::Uint8,
            ConfigNestedValueType::Uint16 {} => cm_rust::ConfigNestedValueType::Uint16,
            ConfigNestedValueType::Uint32 {} => cm_rust::ConfigNestedValueType::Uint32,
            ConfigNestedValueType::Uint64 {} => cm_rust::ConfigNestedValueType::Uint64,
            ConfigNestedValueType::Int8 {} => cm_rust::ConfigNestedValueType::Int8,
            ConfigNestedValueType::Int16 {} => cm_rust::ConfigNestedValueType::Int16,
            ConfigNestedValueType::Int32 {} => cm_rust::ConfigNestedValueType::Int32,
            ConfigNestedValueType::Int64 {} => cm_rust::ConfigNestedValueType::Int64,
            ConfigNestedValueType::String { max_size } => {
                cm_rust::ConfigNestedValueType::String { max_size: max_size.into() }
            }
        }
    }
}

impl TryFrom<&cm_rust::ConfigNestedValueType> for ConfigNestedValueType {
    type Error = ();
    fn try_from(nested: &cm_rust::ConfigNestedValueType) -> Result<Self, ()> {
        Ok(match nested {
            cm_rust::ConfigNestedValueType::Bool => ConfigNestedValueType::Bool {},
            cm_rust::ConfigNestedValueType::Uint8 => ConfigNestedValueType::Uint8 {},
            cm_rust::ConfigNestedValueType::Int8 => ConfigNestedValueType::Int8 {},
            cm_rust::ConfigNestedValueType::Uint16 => ConfigNestedValueType::Uint16 {},
            cm_rust::ConfigNestedValueType::Int16 => ConfigNestedValueType::Int16 {},
            cm_rust::ConfigNestedValueType::Uint32 => ConfigNestedValueType::Uint32 {},
            cm_rust::ConfigNestedValueType::Int32 => ConfigNestedValueType::Int32 {},
            cm_rust::ConfigNestedValueType::Uint64 => ConfigNestedValueType::Uint64 {},
            cm_rust::ConfigNestedValueType::Int64 => ConfigNestedValueType::Int64 {},
            cm_rust::ConfigNestedValueType::String { max_size } => {
                ConfigNestedValueType::String { max_size: NonZeroU32::new(*max_size).ok_or(())? }
            }
        })
    }
}

#[derive(Clone, Hash, Debug, PartialEq, PartialOrd, Eq, Ord, Serialize)]
pub struct ConfigKey(String);

impl ConfigKey {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for ConfigKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ConfigKey {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        let length = s.len();
        if length == 0 {
            return Err(ParseError::Empty);
        }
        if length > 64 {
            return Err(ParseError::TooLong);
        }

        // identifiers must start with a letter
        let first_is_letter = s.chars().next().expect("non-empty string").is_ascii_lowercase();
        // can contain letters, numbers, and underscores
        let contains_invalid_chars =
            s.chars().any(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
        // cannot end with an underscore
        let last_is_underscore = s.chars().next_back().expect("non-empty string") == '_';

        if !first_is_letter || contains_invalid_chars || last_is_underscore {
            return Err(ParseError::InvalidValue);
        }

        Ok(Self(s.to_string()))
    }
}

impl<'de> de::Deserialize<'de> for ConfigKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = ConfigKey;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "a non-empty string no more than 64 characters in length, which must \
                    start with a letter, can contain letters, numbers, and underscores, \
                    but cannot end with an underscore",
                )
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                s.parse().map_err(|err| match err {
                    ParseError::InvalidValue => E::invalid_value(
                        de::Unexpected::Str(s),
                        &"a name which must start with a letter, can contain letters, \
                        numbers, and underscores, but cannot end with an underscore",
                    ),
                    ParseError::TooLong | ParseError::Empty => E::invalid_length(
                        s.len(),
                        &"a non-empty name no more than 64 characters in length",
                    ),
                    e => {
                        panic!("unexpected parse error: {:?}", e);
                    }
                })
            }
        }
        deserializer.deserialize_string(Visitor)
    }
}

#[derive(Clone, Deserialize, Debug, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub enum ConfigRuntimeSource {
    Parent,
}

#[derive(Clone, Deserialize, Debug, PartialEq, Serialize)]
#[serde(tag = "type", deny_unknown_fields, rename_all = "lowercase")]
pub enum ConfigValueType {
    Bool {
        #[serde(skip_serializing_if = "Option::is_none")]
        mutability: Option<Vec<ConfigRuntimeSource>>,
    },
    Uint8 {
        #[serde(skip_serializing_if = "Option::is_none")]
        mutability: Option<Vec<ConfigRuntimeSource>>,
    },
    Uint16 {
        #[serde(skip_serializing_if = "Option::is_none")]
        mutability: Option<Vec<ConfigRuntimeSource>>,
    },
    Uint32 {
        #[serde(skip_serializing_if = "Option::is_none")]
        mutability: Option<Vec<ConfigRuntimeSource>>,
    },
    Uint64 {
        #[serde(skip_serializing_if = "Option::is_none")]
        mutability: Option<Vec<ConfigRuntimeSource>>,
    },
    Int8 {
        #[serde(skip_serializing_if = "Option::is_none")]
        mutability: Option<Vec<ConfigRuntimeSource>>,
    },
    Int16 {
        #[serde(skip_serializing_if = "Option::is_none")]
        mutability: Option<Vec<ConfigRuntimeSource>>,
    },
    Int32 {
        #[serde(skip_serializing_if = "Option::is_none")]
        mutability: Option<Vec<ConfigRuntimeSource>>,
    },
    Int64 {
        #[serde(skip_serializing_if = "Option::is_none")]
        mutability: Option<Vec<ConfigRuntimeSource>>,
    },
    String {
        max_size: NonZeroU32,
        #[serde(skip_serializing_if = "Option::is_none")]
        mutability: Option<Vec<ConfigRuntimeSource>>,
    },
    Vector {
        max_count: NonZeroU32,
        element: ConfigNestedValueType,
        #[serde(skip_serializing_if = "Option::is_none")]
        mutability: Option<Vec<ConfigRuntimeSource>>,
    },
}

impl ConfigValueType {
    /// Update the hasher by digesting the ConfigValueType enum value
    pub fn update_digest(&self, hasher: &mut impl sha2::Digest) {
        let val = match self {
            ConfigValueType::Bool { .. } => 0u8,
            ConfigValueType::Uint8 { .. } => 1u8,
            ConfigValueType::Uint16 { .. } => 2u8,
            ConfigValueType::Uint32 { .. } => 3u8,
            ConfigValueType::Uint64 { .. } => 4u8,
            ConfigValueType::Int8 { .. } => 5u8,
            ConfigValueType::Int16 { .. } => 6u8,
            ConfigValueType::Int32 { .. } => 7u8,
            ConfigValueType::Int64 { .. } => 8u8,
            ConfigValueType::String { max_size, .. } => {
                hasher.update(max_size.get().to_le_bytes());
                9u8
            }
            ConfigValueType::Vector { max_count, element, .. } => {
                hasher.update(max_count.get().to_le_bytes());
                element.update_digest(hasher);
                10u8
            }
        };
        hasher.update([val])
    }
}

impl From<ConfigValueType> for cm_rust::ConfigValueType {
    fn from(value: ConfigValueType) -> Self {
        match value {
            ConfigValueType::Bool { .. } => cm_rust::ConfigValueType::Bool,
            ConfigValueType::Uint8 { .. } => cm_rust::ConfigValueType::Uint8,
            ConfigValueType::Uint16 { .. } => cm_rust::ConfigValueType::Uint16,
            ConfigValueType::Uint32 { .. } => cm_rust::ConfigValueType::Uint32,
            ConfigValueType::Uint64 { .. } => cm_rust::ConfigValueType::Uint64,
            ConfigValueType::Int8 { .. } => cm_rust::ConfigValueType::Int8,
            ConfigValueType::Int16 { .. } => cm_rust::ConfigValueType::Int16,
            ConfigValueType::Int32 { .. } => cm_rust::ConfigValueType::Int32,
            ConfigValueType::Int64 { .. } => cm_rust::ConfigValueType::Int64,
            ConfigValueType::String { max_size, .. } => {
                cm_rust::ConfigValueType::String { max_size: max_size.into() }
            }
            ConfigValueType::Vector { max_count, element, .. } => {
                cm_rust::ConfigValueType::Vector {
                    max_count: max_count.into(),
                    nested_type: element.into(),
                }
            }
        }
    }
}

pub trait FromClause {
    fn from_(&self) -> OneOrMany<AnyRef<'_>>;
}

pub trait FromClauseContext {
    fn from_(&self) -> ContextSpanned<OneOrMany<AnyRef<'_>>>;
}

pub trait CapabilityClause: Clone + PartialEq + std::fmt::Debug {
    fn service(&self) -> Option<OneOrMany<&BorrowedName>>;
    fn protocol(&self) -> Option<OneOrMany<&BorrowedName>>;
    fn directory(&self) -> Option<OneOrMany<&BorrowedName>>;
    fn storage(&self) -> Option<OneOrMany<&BorrowedName>>;
    fn runner(&self) -> Option<OneOrMany<&BorrowedName>>;
    fn resolver(&self) -> Option<OneOrMany<&BorrowedName>>;
    fn event_stream(&self) -> Option<OneOrMany<&BorrowedName>>;
    fn dictionary(&self) -> Option<OneOrMany<&BorrowedName>>;
    fn config(&self) -> Option<OneOrMany<&BorrowedName>>;
    fn set_service(&mut self, o: Option<OneOrMany<Name>>);
    fn set_protocol(&mut self, o: Option<OneOrMany<Name>>);
    fn set_directory(&mut self, o: Option<OneOrMany<Name>>);
    fn set_storage(&mut self, o: Option<OneOrMany<Name>>);
    fn set_runner(&mut self, o: Option<OneOrMany<Name>>);
    fn set_resolver(&mut self, o: Option<OneOrMany<Name>>);
    fn set_event_stream(&mut self, o: Option<OneOrMany<Name>>);
    fn set_dictionary(&mut self, o: Option<OneOrMany<Name>>);
    fn set_config(&mut self, o: Option<OneOrMany<Name>>);

    fn availability(&self) -> Option<Availability>;
    fn set_availability(&mut self, a: Option<Availability>);

    /// Returns the name of the capability for display purposes.
    /// If `service()` returns `Some`, the capability name must be "service", etc.
    ///
    /// Returns an error if the capability name is not set, or if there is more than one.
    fn capability_type(&self) -> Result<&'static str, Error> {
        let mut types = Vec::new();
        if self.service().is_some() {
            types.push("service");
        }
        if self.protocol().is_some() {
            types.push("protocol");
        }
        if self.directory().is_some() {
            types.push("directory");
        }
        if self.storage().is_some() {
            types.push("storage");
        }
        if self.event_stream().is_some() {
            types.push("event_stream");
        }
        if self.runner().is_some() {
            types.push("runner");
        }
        if self.config().is_some() {
            types.push("config");
        }
        if self.resolver().is_some() {
            types.push("resolver");
        }
        if self.dictionary().is_some() {
            types.push("dictionary");
        }
        match types.len() {
            0 => {
                let supported_keywords = self
                    .supported()
                    .iter()
                    .map(|k| format!("\"{}\"", k))
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(Error::validate(format!(
                    "`{}` declaration is missing a capability keyword, one of: {}",
                    self.decl_type(),
                    supported_keywords,
                )))
            }
            1 => Ok(types[0]),
            _ => Err(Error::validate(format!(
                "{} declaration has multiple capability types defined: {:?}",
                self.decl_type(),
                types
            ))),
        }
    }

    /// Returns true if this capability type allows the ::Many variant of OneOrMany.
    fn are_many_names_allowed(&self) -> bool;

    fn decl_type(&self) -> &'static str;
    fn supported(&self) -> &[&'static str];

    /// Returns the names of the capabilities in this clause.
    /// If `protocol()` returns `Some(OneOrMany::Many(vec!["a", "b"]))`, this returns!["a", "b"].
    fn names(&self) -> Vec<&BorrowedName> {
        let res = vec![
            self.service(),
            self.protocol(),
            self.directory(),
            self.storage(),
            self.runner(),
            self.config(),
            self.resolver(),
            self.event_stream(),
            self.dictionary(),
        ];
        res.into_iter()
            .map(|o| o.map(|o| o.into_iter().collect::<Vec<&BorrowedName>>()).unwrap_or(vec![]))
            .flatten()
            .collect()
    }

    fn set_names(&mut self, names: Vec<Name>) {
        let names = match names.len() {
            0 => None,
            1 => Some(OneOrMany::One(names.first().unwrap().clone())),
            _ => Some(OneOrMany::Many(names)),
        };

        let cap_type = self.capability_type().unwrap();
        if cap_type == "protocol" {
            self.set_protocol(names);
        } else if cap_type == "service" {
            self.set_service(names);
        } else if cap_type == "directory" {
            self.set_directory(names);
        } else if cap_type == "storage" {
            self.set_storage(names);
        } else if cap_type == "runner" {
            self.set_runner(names);
        } else if cap_type == "resolver" {
            self.set_resolver(names);
        } else if cap_type == "event_stream" {
            self.set_event_stream(names);
        } else if cap_type == "dictionary" {
            self.set_dictionary(names);
        } else if cap_type == "config" {
            self.set_config(names);
        } else {
            panic!("Unknown capability type {}", cap_type);
        }
    }
}

trait CanonicalizeContext {
    fn canonicalize_context(&mut self);
}

pub trait AsClauseContext {
    fn r#as(&self) -> Option<ContextSpanned<&BorrowedName>>;
}

pub fn alias_or_name_context<'a>(
    alias: Option<ContextSpanned<&'a BorrowedName>>,
    name: &'a BorrowedName,
    origin: Arc<std::path::Path>,
) -> ContextSpanned<&'a BorrowedName> {
    alias.unwrap_or(ContextSpanned { value: name, origin })
}

pub fn alias_or_path<'a>(alias: Option<&'a Path>, path: &'a Path) -> &'a Path {
    alias.unwrap_or(path)
}

pub fn format_cml(buffer: &str, file: Option<&std::path::Path>) -> Result<Vec<u8>, Error> {
    let general_order = PathOption::PropertyNameOrder(vec![
        "name",
        "url",
        "startup",
        "environment",
        "config",
        "dictionary",
        "durability",
        "service",
        "protocol",
        "directory",
        "storage",
        "runner",
        "resolver",
        "event",
        "event_stream",
        "from",
        "as",
        "to",
        "rights",
        "path",
        "subdir",
        "filter",
        "dependency",
        "extends",
        "runners",
        "resolvers",
        "debug",
    ]);
    let options = FormatOptions {
        collapse_containers_of_one: true,
        sort_array_items: true, // but use options_by_path to turn this off for program args
        options_by_path: hashmap! {
            "/*" => hashset! {
                PathOption::PropertyNameOrder(vec![
                    "name",
                    "composite_name",
                    "include",
                    "program",
                    "children",
                    "collections",
                    "capabilities",
                    "use",
                    "offer",
                    "offers",
                    "expose",
                    "environments",
                    "facets",
                    "config",
                    "metadata_mappings",
                ])
            },
            "/*/program" => hashset! {
                PathOption::CollapseContainersOfOne(false),
                PathOption::PropertyNameOrder(vec![
                    "runner",
                    "binary",
                    "args",
                ]),
            },
            "/*/program/*" => hashset! {
                PathOption::SortArrayItems(false),
            },
            "/*/*/*" => hashset! {
                general_order.clone()
            },
            "/*/*/*/*/*" => hashset! {
                general_order
            },
        },
        ..Default::default()
    };

    json5format::format(buffer, file.map(|f| f.to_string_lossy().to_string()), Some(options))
        .map_err(|e| Error::json5(e, file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::document;
    use crate::types::environment::RunnerRegistration;
    use assert_matches::assert_matches;
    use serde_json::Value;
    use std::path::Path;

    // Exercise reference parsing tests on `OfferFromRef` because it contains every reference
    // subtype.

    #[test]
    fn test_parse_named_reference() {
        assert_matches!("#some-child".parse::<OfferFromRef>(), Ok(OfferFromRef::Named(name)) if name == "some-child");
        assert_matches!("#A".parse::<OfferFromRef>(), Ok(OfferFromRef::Named(name)) if name == "A");
        assert_matches!("#7".parse::<OfferFromRef>(), Ok(OfferFromRef::Named(name)) if name == "7");
        assert_matches!("#_".parse::<OfferFromRef>(), Ok(OfferFromRef::Named(name)) if name == "_");

        assert_matches!("#-".parse::<OfferFromRef>(), Err(_));
        assert_matches!("#.".parse::<OfferFromRef>(), Err(_));
        assert_matches!("#".parse::<OfferFromRef>(), Err(_));
        assert_matches!("some-child".parse::<OfferFromRef>(), Err(_));
    }

    #[test]
    fn test_parse_reference_test() {
        assert_matches!("parent".parse::<OfferFromRef>(), Ok(OfferFromRef::Parent));
        assert_matches!("framework".parse::<OfferFromRef>(), Ok(OfferFromRef::Framework));
        assert_matches!("self".parse::<OfferFromRef>(), Ok(OfferFromRef::Self_));
        assert_matches!("#child".parse::<OfferFromRef>(), Ok(OfferFromRef::Named(name)) if name == "child");

        assert_matches!("invalid".parse::<OfferFromRef>(), Err(_));
        assert_matches!("#invalid-child^".parse::<OfferFromRef>(), Err(_));
    }

    fn json_value_from_str(json: &str, filename: &Path) -> Result<Value, Error> {
        serde_json::from_str(json).map_err(|e| {
            Error::parse(
                format!("Couldn't read input as JSON: {}", e),
                Some(Location { line: e.line(), column: e.column() }),
                Some(filename),
            )
        })
    }

    fn parse_as_ref(input: &str) -> Result<OfferFromRef, Error> {
        serde_json::from_value::<OfferFromRef>(json_value_from_str(input, &Path::new("test.cml"))?)
            .map_err(|e| Error::parse(format!("{}", e), None, None))
    }

    #[test]
    fn test_deserialize_ref() -> Result<(), Error> {
        assert_matches!(parse_as_ref("\"self\""), Ok(OfferFromRef::Self_));
        assert_matches!(parse_as_ref("\"parent\""), Ok(OfferFromRef::Parent));
        assert_matches!(parse_as_ref("\"#child\""), Ok(OfferFromRef::Named(name)) if name == "child");

        assert_matches!(parse_as_ref(r#""invalid""#), Err(_));

        Ok(())
    }

    #[test]
    fn test_deny_unknown_fields() {
        assert_matches!(serde_json5::from_str::<Document>("{ unknown: \"\" }"), Err(_));
        assert_matches!(serde_json5::from_str::<Environment>("{ unknown: \"\" }"), Err(_));
        assert_matches!(serde_json5::from_str::<RunnerRegistration>("{ unknown: \"\" }"), Err(_));
        assert_matches!(serde_json5::from_str::<ResolverRegistration>("{ unknown: \"\" }"), Err(_));
        assert_matches!(serde_json5::from_str::<Use>("{ unknown: \"\" }"), Err(_));
        assert_matches!(serde_json5::from_str::<Expose>("{ unknown: \"\" }"), Err(_));
        assert_matches!(serde_json5::from_str::<Offer>("{ unknown: \"\" }"), Err(_));
        assert_matches!(serde_json5::from_str::<Capability>("{ unknown: \"\" }"), Err(_));
        assert_matches!(serde_json5::from_str::<Child>("{ unknown: \"\" }"), Err(_));
        assert_matches!(serde_json5::from_str::<Collection>("{ unknown: \"\" }"), Err(_));
    }

    #[test]
    fn test_context_pipeline_denies_unknown_fields() {
        let dummy_path = std::sync::Arc::from(std::path::Path::new("test.cml"));
        let bad_json = "{ unknown : \"\" }".to_string();

        let result = document::parse_and_hydrate(dummy_path, &bad_json);

        assert!(
            result.is_err(),
            "parse should fail because the underlying Document rejected unknown fields"
        );
    }

    #[test]
    fn test_format_cml_top_level_name() {
        let input = r#"{
    use: [
        {
            protocol: "fuchsia.logger.LogSink",
        },
    ],
    program: {
        runner: "elf",
    },
    composite_name: "my_composite",
    name: "my_component",
}"#;
        let formatted = format_cml(input, None).expect("failed to format");
        let formatted_str = std::str::from_utf8(&formatted).unwrap();
        let expected = r#"{
    name: "my_component",
    composite_name: "my_composite",
    program: {
        runner: "elf",
    },
    use: [
        { protocol: "fuchsia.logger.LogSink" },
    ],
}
"#;
        assert_eq!(formatted_str, expected);
    }
}
