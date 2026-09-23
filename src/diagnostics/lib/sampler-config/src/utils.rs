// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_diagnostics::{Selector, StringSelector, TreeNames};
use selectors::FastError;
use serde::{Deserialize, Deserializer, de};
use std::fmt;
use std::marker::PhantomData;
use std::sync::LazyLock;
use thiserror::Error;

pub fn greater_than_zero<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = i64::deserialize(deserializer)?;
    if value <= 0 {
        return Err(de::Error::custom(format!("i64 out of range: {value}")));
    }
    Ok(value)
}

pub fn one_or_many_selectors<'de, D>(deserializer: D) -> Result<Vec<Selector>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(OneOrMany(PhantomData::<Selector>))
}

pub(crate) struct OneOrMany<T>(pub PhantomData<T>);

trait ParseString {
    fn parse_string(value: &str) -> Result<Self, Error>
    where
        Self: Sized;
}

impl ParseString for String {
    fn parse_string(value: &str) -> Result<Self, Error> {
        Ok(value.into())
    }
}

impl ParseString for Selector {
    fn parse_string(value: &str) -> Result<Self, Error> {
        parse_selector(value)
    }
}

impl<'de, T> de::Visitor<'de> for OneOrMany<T>
where
    T: ParseString,
{
    type Value = Vec<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("either a single string or an array of strings")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        let result = T::parse_string(value).map_err(E::custom)?;
        Ok(vec![result])
    }

    fn visit_seq<S>(self, mut visitor: S) -> Result<Self::Value, S::Error>
    where
        S: de::SeqAccess<'de>,
    {
        let mut out = vec![];
        while let Some(s) = visitor.next_element::<String>()? {
            use serde::de::Error;
            let selector = T::parse_string(&s).map_err(S::Error::custom)?;
            out.push(selector);
        }
        if out.is_empty() {
            Err(de::Error::invalid_length(0, &"expected at least one string"))
        } else {
            Ok(out)
        }
    }
}

pub fn parse_selector(selector_str: &str) -> Result<Selector, Error> {
    let selector = selectors::parse_selector::<FastError>(selector_str)?;
    verify_wildcard_restrictions(&selector, selector_str)?;
    Ok(selector)
}

struct WildcardRestriction {
    segments: Vec<StringSelector>,
    must_have_tree_name: bool,
}

static WILDCARD_RESTRICTIONS: LazyLock<Vec<WildcardRestriction>> = LazyLock::new(|| {
    vec![
        WildcardRestriction {
            segments: vec![
                StringSelector::ExactMatch("core".into()),
                StringSelector::ExactMatch("bluetooth-core".into()),
                StringSelector::StringPattern("bt-host-collection:bt-host_*".into()),
            ],
            must_have_tree_name: false,
        },
        WildcardRestriction {
            segments: vec![
                StringSelector::ExactMatch("bootstrap".into()),
                StringSelector::StringPattern("*-drivers:*".into()),
            ],
            must_have_tree_name: true,
        },
        WildcardRestriction {
            segments: vec![
                StringSelector::ExactMatch("bootstrap".into()),
                StringSelector::ExactMatch("fshost".into()),
                StringSelector::ExactMatch("fvm2".into()),
                StringSelector::StringPattern("blobfs-collection*".into()),
            ],
            must_have_tree_name: false,
        },
        WildcardRestriction {
            segments: vec![
                StringSelector::ExactMatch("bootstrap".into()),
                StringSelector::ExactMatch("fshost".into()),
                StringSelector::ExactMatch("fvm2".into()),
                StringSelector::StringPattern("fs-collection*".into()),
            ],
            must_have_tree_name: false,
        },
    ]
});

// `selector` must be validated.
fn verify_wildcard_restrictions(selector: &Selector, raw_selector: &str) -> Result<(), Error> {
    // Safety: assuming that the selector was parsed by selectors::parse_selectors, it has
    // been validated, and these unwraps are safe
    let actual_segments =
        selector.component_selector.as_ref().unwrap().moniker_segments.as_ref().unwrap();
    if !actual_segments.iter().any(|segment| matches!(segment, StringSelector::StringPattern(_))) {
        return Ok(());
    }
    for restriction in &*WILDCARD_RESTRICTIONS {
        if restriction.segments.len() != actual_segments.len() {
            continue;
        }
        if restriction
            .segments
            .iter()
            .zip(actual_segments.iter())
            .any(|(expected_segment, actual_segment)| expected_segment != actual_segment)
        {
            continue;
        }
        if restriction.must_have_tree_name {
            let Some(TreeNames::Some(_)) = selector.tree_names else {
                return Err(Error::InvalidWildcardedSelector(raw_selector.to_string()));
            };
        }
        return Ok(());
    }
    Err(Error::InvalidWildcardedSelector(raw_selector.to_string()))
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("wildcarded component not allowlisted: '{0}'")]
    InvalidWildcardedSelector(String),

    #[error(transparent)]
    ParseSelector(#[from] selectors::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Test(#[serde(deserialize_with = "super::one_or_many_selectors")] Vec<Selector>);

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    struct TestInt(#[serde(deserialize_with = "super::greater_than_zero")] i64);

    #[fuchsia::test]
    fn parse_valid_single_selector() {
        let json = "\"core/bar:root/twig:leaf\"";
        let data: Test = serde_json5::from_str(json).expect("deserialize");
        assert_eq!(
            data,
            Test(vec![selectors::parse_selector::<FastError>("core/bar:root/twig:leaf").unwrap()])
        );
    }

    #[fuchsia::test]
    fn parse_valid_multiple_selectors() {
        let json = "[ \"core/foo:some/branch:leaf\", \"core/bar:root/twig:leaf\"]";
        let data: Test = serde_json5::from_str(json).expect("deserialize");
        assert_eq!(
            data,
            Test(vec![
                selectors::parse_selector::<FastError>("core/foo:some/branch:leaf").unwrap(),
                selectors::parse_selector::<FastError>("core/bar:root/twig:leaf").unwrap()
            ])
        );
    }

    #[fuchsia::test]
    fn refuse_invalid_selectors() {
        let not_string = "42";
        let bad_list = "[ 42, \"core/bar:not:a:selector:root/twig:leaf\"]";
        serde_json5::from_str::<Test>(not_string).expect_err("should fail");
        serde_json5::from_str::<Test>(bad_list).expect_err("should fail");
    }

    #[fuchsia::test]
    fn test_greater_than_zero() {
        let data: TestInt = serde_json5::from_str("1").unwrap();
        assert_eq!(data, TestInt(1));
        serde_json5::from_str::<Test>("0").expect_err("0 is not greater than 0");
        serde_json5::from_str::<Test>("-1").expect_err("-1 is not greater than 0");
    }

    #[fuchsia::test]
    fn wild_card_selectors() {
        let good_selector = r#"["bootstrap/*-drivers\\:*:[name=fvm]root:field"]"#;
        assert_matches!(serde_json5::from_str::<Test>(good_selector), Ok(_));

        let good_selector = r#"["core/bluetooth-core/bt-host-collection\\:bt-host_*:root:field"]"#;
        assert_matches!(serde_json5::from_str::<Test>(good_selector), Ok(_));

        let bad_selector = r#"["not_bootstrap/*-drivers\\:*:[name=fvm]root:field"]"#;
        assert_matches!(serde_json5::from_str::<Test>(bad_selector), Err(_));

        let not_exact_collection_match = r#"["bootstrap/*-drivers*:[name=fvm]root:field"]"#;
        assert_matches!(serde_json5::from_str::<Test>(not_exact_collection_match), Err(_));

        let missing_filter = r#"["not_bootstrap/*-drivers\\:*:root:field"]"#;
        assert_matches!(serde_json5::from_str::<Test>(missing_filter), Err(_));
    }
}
