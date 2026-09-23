// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const MAX_RESOURCE_PATH_SEGMENT_BYTES: usize = 255;

/// Fuchsia package resource paths are Fuchsia object relative paths without the limit on maximum
/// path length.
/// https://fuchsia.dev/fuchsia-src/concepts/packages/package_url#resource-path
#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord, Hash)]
pub struct Resource(String);

impl Resource {
    /// Percent encode the path for inclusion in URLs. Escapes ' ', '"', '<', '>', and '`'.
    /// https://url.spec.whatwg.org/#fragment-percent-encode-set
    pub fn percent_encode(&self) -> impl std::fmt::Display {
        percent_encoding::utf8_percent_encode(&self.0, crate::FRAGMENT)
    }

    /// Checks if `input` is a valid resource path for a Fuchsia Package URL.
    /// Fuchsia package resource paths are Fuchsia object relative paths without the limit on
    /// maximum path length.
    /// https://fuchsia.dev/fuchsia-src/concepts/packages/package_url#resource-path
    ///
    /// Percent decoding should be performed before calling this, if necessary.
    pub fn validate_str(input: &str) -> Result<(), ResourcePathError> {
        if input.is_empty() {
            return Err(ResourcePathError::PathIsEmpty);
        }
        if input.starts_with('/') {
            return Err(ResourcePathError::PathStartsWithSlash);
        }
        if input.ends_with('/') {
            return Err(ResourcePathError::PathEndsWithSlash);
        }
        for segment in input.split('/') {
            if segment.contains('\0') {
                return Err(ResourcePathError::NameContainsNull);
            }
            if segment == "." {
                return Err(ResourcePathError::NameIsDot);
            }
            if segment == ".." {
                return Err(ResourcePathError::NameIsDotDot);
            }
            if segment.is_empty() {
                return Err(ResourcePathError::NameEmpty);
            }
            if segment.len() > MAX_RESOURCE_PATH_SEGMENT_BYTES {
                return Err(ResourcePathError::NameTooLong);
            }
            // TODO(https://fxbug.dev/42096516) allow newline once meta/contents supports it in blob
            // paths.
            if segment.contains('\n') {
                return Err(ResourcePathError::NameContainsNewline);
            }
        }
        Ok(())
    }
}

impl TryFrom<String> for Resource {
    type Error = ResourcePathError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let () = Self::validate_str(&value)?;
        Ok(Self(value))
    }
}

impl std::str::FromStr for Resource {
    type Err = ResourcePathError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let () = Self::validate_str(s)?;
        Ok(Self(s.to_owned()))
    }
}

impl std::fmt::Display for Resource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for Resource {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for Resource {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        self.0.as_str()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResourcePathError {
    #[error("object names must be at least 1 byte")]
    NameEmpty,

    #[error("object names must be at most {} bytes", MAX_RESOURCE_PATH_SEGMENT_BYTES)]
    NameTooLong,

    #[error("object names cannot contain the NULL byte")]
    NameContainsNull,

    #[error("object names cannot be '.'")]
    NameIsDot,

    #[error("object names cannot be '..'")]
    NameIsDotDot,

    #[error("object paths cannot start with '/'")]
    PathStartsWithSlash,

    #[error("object paths cannot end with '/'")]
    PathEndsWithSlash,

    #[error("object paths must be at least 1 byte")]
    PathIsEmpty,

    // TODO(https://fxbug.dev/42096516) allow newline once meta/contents supports it in blob paths
    #[error(r"object names cannot contain the newline character '\n'")]
    NameContainsNewline,
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test::*;
    use proptest::prelude::*;

    // Tests for invalid paths
    #[test]
    fn test_empty_string() {
        assert_eq!(Resource::validate_str(""), Err(ResourcePathError::PathIsEmpty));
    }

    proptest! {
        #![proptest_config(ProptestConfig{
            failure_persistence: None,
            ..Default::default()
        })]

        #[test]
        fn test_reject_empty_object_name(
            ref s in random_resource_path_with_regex_segment_str(5, "")) {
            prop_assume!(!s.starts_with('/') && !s.ends_with('/'));
            prop_assert_eq!(Resource::validate_str(s), Err(ResourcePathError::NameEmpty));
        }

        #[test]
        fn test_reject_long_object_name(
            ref s in random_resource_path_with_regex_segment_str(
                5,
                r"[[[:ascii:]]--\.--/--\x00]{256}"
            )) {
            prop_assert_eq!(Resource::validate_str(s), Err(ResourcePathError::NameTooLong));
        }

        #[test]
        fn test_reject_contains_null(
            ref s in random_resource_path_with_regex_segment_string(
                5, format!(r"{}{{0,3}}\x00{}{{0,3}}",
                           ANY_UNICODE_EXCEPT_SLASH_NULL_DOT_OR_NEWLINE,
                           ANY_UNICODE_EXCEPT_SLASH_NULL_DOT_OR_NEWLINE))) {
            prop_assert_eq!(Resource::validate_str(s), Err(ResourcePathError::NameContainsNull));
        }

        #[test]
        fn test_reject_name_is_dot(
            ref s in random_resource_path_with_regex_segment_str(5, r"\.")) {
            prop_assert_eq!(Resource::validate_str(s), Err(ResourcePathError::NameIsDot));
        }

        #[test]
        fn test_reject_name_is_dot_dot(
            ref s in random_resource_path_with_regex_segment_str(5, r"\.\.")) {
            prop_assert_eq!(Resource::validate_str(s), Err(ResourcePathError::NameIsDotDot));
        }

        #[test]
        fn test_reject_starts_with_slash(
            ref s in format!(
                "/{}{{1,5}}",
                ANY_UNICODE_EXCEPT_SLASH_NULL_DOT_OR_NEWLINE).as_str()) {
            prop_assert_eq!(Resource::validate_str(s), Err(ResourcePathError::PathStartsWithSlash));
        }

        #[test]
        fn test_reject_ends_with_slash(
            ref s in format!(
                "{}{{1,5}}/",
                ANY_UNICODE_EXCEPT_SLASH_NULL_DOT_OR_NEWLINE).as_str()) {
            prop_assert_eq!(Resource::validate_str(s), Err(ResourcePathError::PathEndsWithSlash));
        }

        #[test]
        fn test_reject_contains_newline(
            ref s in random_resource_path_with_regex_segment_string(
                5, format!(r"{}{{0,3}}\x0a{}{{0,3}}",
                           ANY_UNICODE_EXCEPT_SLASH_NULL_DOT_OR_NEWLINE,
                           ANY_UNICODE_EXCEPT_SLASH_NULL_DOT_OR_NEWLINE))) {
            prop_assert_eq!(Resource::validate_str(s), Err(ResourcePathError::NameContainsNewline));
        }
    }

    // Tests for valid paths
    proptest! {
        #![proptest_config(ProptestConfig{
            failure_persistence: None,
            ..Default::default()
        })]

        #[test]
        fn test_name_contains_dot(
            ref s in random_resource_path_with_regex_segment_string(
                5, format!(r"{}{{1,4}}\.{}{{1,4}}",
                           ANY_UNICODE_EXCEPT_SLASH_NULL_DOT_OR_NEWLINE,
                           ANY_UNICODE_EXCEPT_SLASH_NULL_DOT_OR_NEWLINE)))
        {
            prop_assert_eq!(Resource::validate_str(s), Ok(()));
        }

        #[test]
        fn test_name_contains_dot_dot(
            ref s in random_resource_path_with_regex_segment_string(
                5, format!(r"{}{{1,4}}\.\.{}{{1,4}}",
                           ANY_UNICODE_EXCEPT_SLASH_NULL_DOT_OR_NEWLINE,
                           ANY_UNICODE_EXCEPT_SLASH_NULL_DOT_OR_NEWLINE)))
        {
            prop_assert_eq!(Resource::validate_str(s), Ok(()));
        }

        #[test]
        fn test_single_segment(ref s in always_valid_resource_path_chars(1, 4)) {
            prop_assert_eq!(Resource::validate_str(s), Ok(()));
        }

        #[test]
        fn test_multi_segment(
            ref s in prop::collection::vec(always_valid_resource_path_chars(1, 4), 1..5))
        {
            let path = s.join("/");
            prop_assert_eq!(Resource::validate_str(&path), Ok(()));
        }

        #[test]
        fn test_long_name(
            // TODO(https://fxbug.dev/42096516) allow newline once meta/contents supports it in blob
            // paths.
            ref s in random_resource_path_with_regex_segment_str(
                5, "[[[:ascii:]]--\0--/--\n]{255}"))
        {
            prop_assert_eq!(Resource::validate_str(s), Ok(()));
        }
    }
}
