// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A serde module for Inspect selectors, to be used in the "with" field
//! attribute.
//!
//! ```
//! #[derive(Serialize, Deserialize)]
//! struct Config {
//!     #[serde(with="selectors_ext::inspect")]
//!     selectors: Vec<fidl_fuchsia_diagnostics::Selector>,
//! }
//! ```

use serde::de::{Error, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserializer, Serializer};

const PREFIX: &str = "INSPECT:";

pub fn serialize<S>(
    selectors: &Vec<fidl_fuchsia_diagnostics::Selector>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut seq = serializer.serialize_seq(Some(selectors.len()))?;

    for selector in selectors {
        seq.serialize_element(&format!(
            "{PREFIX}{}",
            selectors::selector_to_string(selector, selectors::SelectorDisplayOptions::default())
                .map_err(serde::ser::Error::custom)?
        ))?;
    }

    seq.end()
}

pub fn deserialize<'de, D>(
    deserializer: D,
) -> Result<Vec<fidl_fuchsia_diagnostics::Selector>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_seq(SelectorsVisitor)
}

struct SelectorsVisitor;

impl<'de> Visitor<'de> for SelectorsVisitor {
    type Value = Vec<fidl_fuchsia_diagnostics::Selector>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "should be a list of string selectors starting with {}", PREFIX)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut selectors = Vec::new();
        while let Some(v) = seq.next_element::<String>()? {
            let Some(rest) = v.strip_prefix(PREFIX) else {
                return Err(A::Error::custom(format!(
                    "Expected selector with prefix \"{}\", got \"{v}\"",
                    PREFIX
                )));
            };
            selectors.push(
                selectors::parse_selector::<selectors::VerboseError>(rest)
                    .map_err(|e| A::Error::custom(e))?,
            );
        }
        Ok(selectors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_diagnostics::Selector;
    use pretty_assertions::assert_eq;
    use serde::{Deserialize, Serialize};
    use test_case::test_case;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    #[serde(transparent)]
    struct TestConfig {
        #[serde(with = "crate::inspect")]
        selectors: Vec<Selector>,
    }

    #[test_case("a/b:c:d" ; "with_basic_full_selector")]
    #[test_case("a/b:c" ; "with_basic_partial_selector")]
    #[test_case(r"a/b:c/d\/e:f" ; "with_escaped_forward_slash")]
    #[test_case(r"a/b:[name=cd-e]f:g" ; "with_non_default_name")]
    #[test_case(r#"a:[name="bc-d"]e:f"# ; "with_unneeded_name_quotes")]
    #[test_case(r#"a:[name="b[]c"]d:e"# ; "with_needed_name_quotes")]
    #[test_case("a/b:[...]c:d" ; "with_all_names")]
    #[test_case(r#"a/b:[name=c, name="d", name="f[]g"]h:i"# ; "with_name_list")]
    #[test_case(r"a\:b/c:d:e" ; "with_collection")]
    #[test_case(r"a/b/c*d:e:f" ; "with_wildcard_component")]
    #[test_case(r"a/b:c*/d:e" ; "with_wildcard_tree")]
    #[test_case(r"a/b:c\*/d:e" ; "with_escaped_wildcard_tree")]
    #[test_case(r"a/b/c/d:e/f:g*" ; "with_wildcard_property")]
    #[test_case("a/b/c/d:e/f/g/h:k" ; "with_deep_nesting")]
    #[fuchsia::test]
    fn test_serialization(selector: &str) {
        // Escape double quotes and forward slashes then wrap in double quotes.
        let selector_escaped = serde_json::to_string(&format!("{PREFIX}{selector}")).unwrap();

        let deserialized: TestConfig =
            serde_json::from_str(&format!("[{selector_escaped}]")).unwrap();
        let serialized = serde_json::to_string(&deserialized).unwrap();
        let deserialized_again: TestConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, deserialized_again);
        assert_eq!(deserialized.selectors, vec![selectors::parse_verbose(selector).unwrap()]);
    }

    #[test_case("🦀a🦀" ; "unicode_split_at_boundary_emoji")]
    #[test_case("日本語" ; "unicode_split_at_boundary_cjk")]
    #[test_case("test:🔥" ; "unicode_split_at_boundary_short_prefix")]
    #[fuchsia::test]
    fn test_deserialization_invalid_prefix_unicode(input: &str) {
        let json = serde_json::to_string(&vec![input]).unwrap();
        let result = serde_json::from_str::<TestConfig>(&json);
        assert!(result.is_err());
    }
}
