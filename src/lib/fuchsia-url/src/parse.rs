// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::PackagePathSegmentError;
use serde::{Deserialize, Serialize};
use std::convert::TryInto as _;

pub const MAX_PACKAGE_PATH_SEGMENT_BYTES: usize = 255;

/// Check if a string conforms to r"^[0-9a-z\-\._]{1,255}$" and is neither "." nor ".."
pub fn validate_package_path_segment(string: &str) -> Result<(), PackagePathSegmentError> {
    if string.is_empty() {
        return Err(PackagePathSegmentError::Empty);
    }
    if string.len() > MAX_PACKAGE_PATH_SEGMENT_BYTES {
        return Err(PackagePathSegmentError::TooLong(string.len()));
    }
    if let Some(invalid_byte) = string.bytes().find(|&b| {
        !(b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.' || b == b'_')
    }) {
        return Err(PackagePathSegmentError::InvalidCharacter { character: invalid_byte.into() });
    }
    if string == "." {
        return Err(PackagePathSegmentError::DotSegment);
    }
    if string == ".." {
        return Err(PackagePathSegmentError::DotDotSegment);
    }

    Ok(())
}

/// A Fuchsia Package Name. Package names are the first segment of the path.
/// https://fuchsia.dev/fuchsia-src/concepts/packages/package_url#package-name
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Hash, Serialize)]
pub struct PackageName(String);

impl std::str::FromStr for PackageName {
    type Err = PackagePathSegmentError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let () = validate_package_path_segment(s)?;
        Ok(Self(s.into()))
    }
}

impl TryFrom<String> for PackageName {
    type Error = PackagePathSegmentError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let () = validate_package_path_segment(&value)?;
        Ok(Self(value))
    }
}

impl TryFrom<&crate::Path> for PackageName {
    type Error = crate::ParseError;
    fn try_from(path: &crate::Path) -> Result<Self, Self::Error> {
        // A PackageName is a Path with a single segment.
        path.parse().map_err(Self::Error::InvalidName)
    }
}

impl From<PackageName> for String {
    fn from(name: PackageName) -> Self {
        name.0
    }
}

impl From<&PackageName> for String {
    fn from(name: &PackageName) -> Self {
        name.0.clone()
    }
}

impl std::fmt::Display for PackageName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for PackageName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PackageName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value
            .try_into()
            .map_err(|e| serde::de::Error::custom(format!("invalid package name: {}", e)))
    }
}

/// A Fuchsia Package Variant. Package variants are the optional second segment of the path.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Hash, Serialize)]
pub struct PackageVariant(String);

impl PackageVariant {
    /// The string representation, "0", of the zero package variant.
    pub const ZERO_STR: &str = "0";

    /// Create a `PackageVariant` of "0".
    pub fn zero() -> Self {
        Self::ZERO_STR.parse().expect("\"0\" is a valid variant")
    }

    /// Returns true iff the variant is "0".
    pub fn is_zero(&self) -> bool {
        self.0 == Self::ZERO_STR
    }
}

impl std::str::FromStr for PackageVariant {
    type Err = PackagePathSegmentError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let () = validate_package_path_segment(s)?;
        Ok(Self(s.into()))
    }
}

impl TryFrom<String> for PackageVariant {
    type Error = PackagePathSegmentError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let () = validate_package_path_segment(&value)?;
        Ok(Self(value))
    }
}

impl std::fmt::Display for PackageVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for PackageVariant {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PackageVariant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value
            .try_into()
            .map_err(|e| serde::de::Error::custom(format!("invalid package variant {}", e)))
    }
}

#[cfg(test)]
mod test_validate_package_path_segment {
    use super::*;
    use crate::test::random_package_segment;
    use proptest::prelude::*;

    #[test]
    fn reject_empty_segment() {
        assert_eq!(validate_package_path_segment(""), Err(PackagePathSegmentError::Empty));
    }

    #[test]
    fn reject_dot_segment() {
        assert_eq!(validate_package_path_segment("."), Err(PackagePathSegmentError::DotSegment));
    }

    #[test]
    fn reject_dot_dot_segment() {
        assert_eq!(
            validate_package_path_segment(".."),
            Err(PackagePathSegmentError::DotDotSegment)
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig{
            failure_persistence: None,
            ..Default::default()
        })]

        #[test]
        fn reject_segment_too_long(ref s in r"[-_0-9a-z\.]{256, 300}")
        {
            prop_assert_eq!(
                validate_package_path_segment(s),
                Err(PackagePathSegmentError::TooLong(s.len()))
            );
        }

        #[test]
        fn reject_invalid_character(ref s in r"[-_0-9a-z\.]{0, 48}[^-_0-9a-z\.][-_0-9a-z\.]{0, 48}")
        {
            let pass = matches!(
                validate_package_path_segment(s),
                Err(PackagePathSegmentError::InvalidCharacter{..})
            );
            prop_assert!(pass);
        }

        #[test]
        fn valid_segment(ref s in random_package_segment())
        {
            prop_assert_eq!(
                validate_package_path_segment(s),
                Ok(())
            );
        }
    }
}

#[cfg(test)]
mod test_package_name {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn from_str_rejects_invalid() {
        assert_eq!(
            "?".parse::<PackageName>(),
            Err(PackagePathSegmentError::InvalidCharacter { character: '?'.into() })
        );
    }

    #[test]
    fn from_str_succeeds() {
        "package-name".parse::<PackageName>().unwrap();
    }

    #[test]
    fn try_from_rejects_invalid() {
        assert_eq!(
            PackageName::try_from("?".to_string()),
            Err(PackagePathSegmentError::InvalidCharacter { character: '?'.into() })
        );
    }

    #[test]
    fn try_from_succeeds() {
        PackageName::try_from("valid-name".to_string()).unwrap();
    }

    #[test]
    fn from_succeeds() {
        assert_eq!(
            String::from("package-name".parse::<PackageName>().unwrap()),
            "package-name".to_string()
        );
    }

    #[test]
    fn display() {
        let path: PackageName = "package-name".parse().unwrap();
        assert_eq!(format!("{}", path), "package-name");
    }

    #[test]
    fn as_ref() {
        let path: PackageName = "package-name".parse().unwrap();
        assert_eq!(path.as_ref(), "package-name");
    }

    #[test]
    fn deserialize_success() {
        let actual_value =
            serde_json::from_str::<PackageName>("\"package-name\"").expect("json to deserialize");
        assert_eq!(actual_value, "package-name".parse::<PackageName>().unwrap());
    }

    #[test]
    fn deserialize_rejects_invalid() {
        let msg = serde_json::from_str::<PackageName>("\"pack!age-name\"").unwrap_err().to_string();
        assert!(msg.contains("invalid package name"), r#"Bad error message: "{}""#, msg);
    }

    #[test]
    fn try_from_path_ref_success() {
        let path: crate::Path = "valid-name".parse().unwrap();
        assert_eq!(PackageName::try_from(&path).unwrap().as_ref(), "valid-name");
    }

    #[test]
    fn try_from_path_ref_error() {
        let path: crate::Path = "in/valid/name".parse().unwrap();
        assert_matches!(
            PackageName::try_from(&path),
            Err(crate::ParseError::InvalidName(PackagePathSegmentError::InvalidCharacter {
                character: '/'
            }))
        );
    }
}

#[cfg(test)]
mod test_package_variant {
    use super::*;

    #[test]
    fn zero() {
        assert_eq!(PackageVariant::zero().as_ref(), "0");
        assert!(PackageVariant::zero().is_zero());
        assert_eq!("1".parse::<PackageVariant>().unwrap().is_zero(), false);
    }

    #[test]
    fn from_str_rejects_invalid() {
        assert_eq!(
            "?".parse::<PackageVariant>(),
            Err(PackagePathSegmentError::InvalidCharacter { character: '?'.into() })
        );
    }

    #[test]
    fn from_str_succeeds() {
        "package-variant".parse::<PackageVariant>().unwrap();
    }

    #[test]
    fn try_from_rejects_invalid() {
        assert_eq!(
            PackageVariant::try_from("?".to_string()),
            Err(PackagePathSegmentError::InvalidCharacter { character: '?'.into() })
        );
    }

    #[test]
    fn try_from_succeeds() {
        PackageVariant::try_from("valid-variant".to_string()).unwrap();
    }

    #[test]
    fn display() {
        let path: PackageVariant = "package-variant".parse().unwrap();
        assert_eq!(format!("{}", path), "package-variant");
    }

    #[test]
    fn as_ref() {
        let path: PackageVariant = "package-variant".parse().unwrap();
        assert_eq!(path.as_ref(), "package-variant");
    }

    #[test]
    fn deserialize_success() {
        let actual_value = serde_json::from_str::<PackageVariant>("\"package-variant\"")
            .expect("json to deserialize");
        assert_eq!(actual_value, "package-variant".parse::<PackageVariant>().unwrap());
    }

    #[test]
    fn deserialize_rejects_invalid() {
        let msg =
            serde_json::from_str::<PackageVariant>("\"pack!age-variant\"").unwrap_err().to_string();
        assert!(msg.contains("invalid package variant"), r#"Bad error message: "{}""#, msg);
    }
}
