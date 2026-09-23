// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::{Error, bail};
use glob::glob;
use regex_lite::Regex;
use serde::Serialize;
use serde_derive::Deserialize;
use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt::Display;
use std::ops::Deref;
use std::sync::LazyLock;

/// The outer map is service_name; the inner is tag.
pub type Config = HashMap<ServiceName, HashMap<Tag, TagConfig>>;

/// Schema for config-file entries. Each config file is a JSON array of these.
#[derive(Deserialize, Default, Debug, PartialEq)]
#[cfg_attr(test, derive(Clone))]
#[serde(deny_unknown_fields)]
struct TaggedPersist {
    /// The Inspect data defined here will be published under this tag.
    /// Tags must not be duplicated within a service, even between files.
    /// Tags must conform to /[a-z][a-z-]*/.
    tag: String,
    /// Tags are organized under a named service. Multiple tags can use the same
    /// service name. Service names must conform to /[a-z][a-z-]*/.
    service_name: String,
    #[serde(flatten)]
    tag_config: TagConfig,
}

/// Configuration for a single tag for a single service.
///
/// See [`TaggedPersist`] for the meaning of corresponding fields.
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct TagConfig {
    /// These selectors will be fetched and stored for publication on the next boot.
    #[serde(with = "selectors_ext::inspect")]
    pub selectors: Vec<fidl_fuchsia_diagnostics::Selector>,
    /// This is the max size of the file saved, which is the JSON-serialized version
    /// of the selectors' data.
    pub max_bytes: usize,
    /// Persistence requests will be throttled to this. Requests received early will be delayed.
    pub min_seconds_between_fetch: i64,
    /// Should this tag persist across multiple reboots?
    #[serde(default)]
    pub persist_across_boot: bool,
}

/// Wrapper class for a valid tag name.
///
/// This is a witness class that can only be constructed from a `String` that
/// matches [`NAME_PATTERN`].
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct Tag(String);

// Necessary to support the hashbrown::HashMap::entry_ref API.
impl From<&Tag> for Tag {
    fn from(value: &Self) -> Self {
        value.clone()
    }
}

/// Wrapper class for a valid service name.
///
/// This is a witness class that can only be constructed from a `String` that
/// matches [`NAME_PATTERN`].
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ServiceName(String);

// Necessary to support the hashbrown::HashMap::entry_ref API.
impl From<&ServiceName> for ServiceName {
    fn from(value: &Self) -> Self {
        value.clone()
    }
}

/// A regular expression corresponding to a valid tag or service name.
const NAME_PATTERN: &str = r"^[a-z][a-z-]*$";

static NAME_VALIDATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(NAME_PATTERN).unwrap());

impl Tag {
    pub fn new(tag: impl Into<String>) -> Result<Self, Error> {
        let tag = tag.into();
        if !NAME_VALIDATOR.is_match(&tag) {
            bail!("Invalid tag {} must match [a-z][a-z-]*", tag);
        }
        Ok(Self(tag))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

impl ServiceName {
    pub fn new(name: String) -> Result<Self, Error> {
        if !NAME_VALIDATOR.is_match(&name) {
            bail!("Invalid service name {} must match [a-z][a-z-]*", name);
        }
        Ok(Self(name))
    }
}

/// Allow `Tag` to be treated like a `&str` for display, etc.
impl Deref for Tag {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

/// Allow `ServiceName` to be treated like a `&str` for display, etc.
impl Deref for ServiceName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        let Self(tag) = self;
        tag
    }
}

/// Allow treating `Tag` as a `&str` for, e.g., HashMap indexing operations.
impl Borrow<str> for Tag {
    fn borrow(&self) -> &str {
        self
    }
}

/// Allow treating `ServiceName` as a `&str` for, e.g., HashMap indexing
/// operations.
impl Borrow<str> for ServiceName {
    fn borrow(&self) -> &str {
        self
    }
}

impl Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(name) = self;
        name.fmt(f)
    }
}

impl PartialEq<str> for Tag {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl Display for ServiceName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(name) = self;
        name.fmt(f)
    }
}

impl From<ServiceName> for String {
    fn from(ServiceName(value): ServiceName) -> Self {
        value
    }
}

const CONFIG_GLOB: &str = "/config/data/*.persist";

fn try_insert_items(config: &mut Config, config_text: &str) -> Result<(), Error> {
    let items: Vec<TaggedPersist> = serde_json5::from_str(config_text)?;
    for item in items {
        let TaggedPersist { tag, service_name, mut tag_config } = item;
        let tag = Tag::new(tag)?;
        let name = ServiceName::new(service_name)?;
        let mut name_filter = SameTreeNameFilter::default();
        tag_config.selectors.retain(|s| name_filter.check(s));
        if let Some(existing) = config.entry(name).or_default().insert(tag, tag_config) {
            bail!("Duplicate TagConfig found: {:?}", existing);
        }
    }
    Ok(())
}

/// A stateful filter that verifies selectors have the same tree name.
#[derive(Default)]
struct SameTreeNameFilter {
    tree_names: Option<Option<fidl_fuchsia_diagnostics::TreeNames>>,
}

impl SameTreeNameFilter {
    fn check(&mut self, s: &fidl_fuchsia_diagnostics::Selector) -> bool {
        let tree_names = match &self.tree_names {
            Some(names) => names,
            None => {
                self.tree_names = Some(s.tree_names.clone());
                return true;
            }
        };
        match (tree_names, &s.tree_names) {
            (None, None) => true,
            (
                Some(fidl_fuchsia_diagnostics::TreeNames::All(_)),
                Some(fidl_fuchsia_diagnostics::TreeNames::All(_)),
            ) => true,
            (
                Some(fidl_fuchsia_diagnostics::TreeNames::Some(a)),
                Some(fidl_fuchsia_diagnostics::TreeNames::Some(b)),
            ) if a == b => true,
            _ => {
                log::warn!(
                    "Only selectors targeting the same tree are allowed: \"{}\"",
                    selectors::selector_to_string(s, selectors::SelectorDisplayOptions::default())
                        .unwrap_or_else(|e| format!("<INVALID: {e}>"))
                );
                false
            }
        }
    }
}

pub fn load_configuration_files() -> Result<Config, Error> {
    load_configuration_files_from(CONFIG_GLOB)
}

pub fn load_configuration_files_from(path: &str) -> Result<Config, Error> {
    let mut config = HashMap::new();
    for file_path in glob(path)? {
        try_insert_items(&mut config, &std::fs::read_to_string(file_path?)?)?;
    }
    Ok(config)
}

#[cfg(test)]
mod test {
    use assert_matches::assert_matches;

    use super::*;
    use test_case::test_case;

    #[fuchsia::test]
    fn verify_insert_logic() {
        let mut config = HashMap::new();
        let taga_servab = r#"[
            {
                service_name: 'serv-a',
                tag: 'tag-a',
                max_bytes: 10,
                min_seconds_between_fetch: 31,
                selectors: ['INSPECT:a:b', 'INSPECT:b:c']
            },
            {
                service_name: 'serv-b',
                tag: 'tag-a',
                max_bytes: 20,
                min_seconds_between_fetch: 32,
                selectors: ['INSPECT:c:d'],
                persist_across_boot: true
            }
        ]"#;

        let tagb_servb = r#"[
            {
                service_name: 'serv-b',
                tag: 'tag-b',
                max_bytes: 30,
                min_seconds_between_fetch: 33,
                selectors: ['INSPECT:d:e']
            }
        ]"#;

        assert_matches!(try_insert_items(&mut config, taga_servab), Ok(()));
        assert_matches!(try_insert_items(&mut config, tagb_servb), Ok(()));

        assert_eq!(
            config,
            HashMap::from([
                (
                    ServiceName("serv-a".to_string()),
                    HashMap::from([(
                        Tag("tag-a".to_string()),
                        TagConfig {
                            max_bytes: 10,
                            min_seconds_between_fetch: 31,
                            selectors: vec![
                                selectors::parse_verbose("a:b").unwrap(),
                                selectors::parse_verbose("b:c").unwrap(),
                            ],
                            persist_across_boot: false,
                        }
                    )])
                ),
                (
                    ServiceName("serv-b".to_string()),
                    HashMap::from([
                        (
                            Tag("tag-a".to_string()),
                            TagConfig {
                                max_bytes: 20,
                                min_seconds_between_fetch: 32,
                                selectors: vec![selectors::parse_verbose("c:d").unwrap()],
                                persist_across_boot: true,
                            }
                        ),
                        (
                            Tag("tag-b".to_string()),
                            TagConfig {
                                max_bytes: 30,
                                min_seconds_between_fetch: 33,
                                selectors: vec![selectors::parse_verbose("d:e").unwrap()],
                                persist_across_boot: false,
                            }
                        )
                    ])
                )
            ])
        );

        // Can't duplicate tags in the same service
        assert_matches!(try_insert_items(&mut config, tagb_servb), Err(_));
    }

    #[fuchsia::test]
    fn test_tag_equals_str() {
        assert_eq!(&Tag::new("foo").unwrap(), "foo");
    }

    #[test_case(
        r#"[{
            tag: 'tag',
            service_name: 'bad-service-1',
            max_bytes: 10,
            min_seconds_between_fetch: 10,
            selectors: ['INSPECT:a:b']
        }]"#
        ; "numbers_in_name"
    )]
    #[test_case(
        r#"[{
            tag: 'tag',
            service_name: 'bad_service',
            max_bytes: 10,
            min_seconds_between_fetch: 10,
            selectors: ['INSPECT:a:b']
        }]"#
        ; "underscores_in_name"
    )]
    #[test_case(
        r#"[{
            tag: 'tag',
            service_name: 'service',
            max_bytes: 10,
            min_seconds_between_fetch: 10,
            selectors: ['a:b']
        }]"#
        ; "selector_source_not_specified"
    )]
    #[test_case(
        r#"[{
            tag: 'tag',
            service_name: 'service',
            max_bytes: 10,
            min_seconds_between_fetch: 10,
            selectors: [
                'INSPECT:a:b'
                'INSPECT:a:[name=custom_tree]c'
            ]
        }]"#
        ; "different_tree_names"
    )]
    #[fuchsia::test]
    fn rejects_invalid_config(config_text: &str) {
        let mut config = HashMap::new();
        assert_matches!(try_insert_items(&mut config, config_text), Err(_));
    }

    #[test_case(
        r#"[{
            tag: 'tag',
            service_name: 'service',
            max_bytes: 10,
            min_seconds_between_fetch: 10,
            selectors: [
                'INSPECT:a:[name=custom_tree]b',
                'INSPECT:a:[name=custom_tree]c'
            ]
        }]"#
        ; "same_custom_tree_names"
    )]
    #[fuchsia::test]
    fn valid_config(config_text: &str) {
        let mut config = HashMap::new();
        assert_matches!(try_insert_items(&mut config, config_text), Ok(()));
    }
}
