// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use crate::errors::ParseError;
use crate::{Scheme, UrlParts};

pub const SCHEME: &str = "fuchsia-builtin";

/// Decoded representation of a builtin URL.
///
/// fuchsia-builtin://#resource
///
/// Builtin component declarations are used to bootstrap the ELF runner.
/// They are never packaged.
///
/// The path in builtin URLs must be "/". Following that, they may contain a fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltinUrl {
    resource: Option<crate::Resource>,
}

impl BuiltinUrl {
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        Self::try_from_parts(UrlParts::parse(input)?)
    }

    fn try_from_parts(
        UrlParts { scheme, host, path, hash, resource }: UrlParts,
    ) -> Result<Self, ParseError> {
        if scheme.ok_or(ParseError::MissingScheme)? != Scheme::Builtin {
            return Err(ParseError::InvalidScheme);
        }

        if host.is_some() {
            return Err(ParseError::HostMustBeEmpty);
        }

        if hash.is_some() {
            return Err(ParseError::CannotContainHash);
        }

        if path.is_some() {
            return Err(ParseError::PathMustBeRoot);
        }

        Ok(Self { resource })
    }

    pub fn resource(&self) -> Option<&crate::Resource> {
        self.resource.as_ref()
    }
}

impl std::fmt::Display for BuiltinUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://", SCHEME)?;
        if let Some(ref resource) = self.resource {
            write!(f, "#{}", resource.percent_encode())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::PackagePathSegmentError;
    use crate::resource::ResourcePathError;
    use assert_matches::assert_matches;

    #[test]
    fn test_parse_ok() {
        assert_eq!(BuiltinUrl::parse("fuchsia-builtin://").unwrap().resource(), None);
        assert_eq!(
            BuiltinUrl::parse("fuchsia-builtin://#a").unwrap().resource().map(|r| r.as_ref()),
            Some("a")
        );
        assert_eq!(
            BuiltinUrl::parse("fuchsia-builtin://#elf_runner.cm")
                .unwrap()
                .resource()
                .map(|r| r.as_ref()),
            Some("elf_runner.cm")
        );
    }

    #[test]
    fn test_parse_error_wrong_scheme() {
        assert_matches!(BuiltinUrl::parse("foobar://").unwrap_err(), ParseError::InvalidScheme);
        assert_matches!(
            BuiltinUrl::parse("fuchsia-boot://").unwrap_err(),
            ParseError::InvalidScheme
        );
        assert_matches!(
            BuiltinUrl::parse("fuchsia-pkg://").unwrap_err(),
            ParseError::InvalidScheme
        );
    }

    #[test]
    fn test_parse_error_missing_scheme() {
        assert_matches!(BuiltinUrl::parse("package").unwrap_err(), ParseError::MissingScheme);
    }

    #[test]
    fn test_parse_error_invalid_path() {
        assert_matches!(
            BuiltinUrl::parse("fuchsia-builtin:////").unwrap_err(),
            ParseError::InvalidPathSegment(PackagePathSegmentError::Empty)
        );
    }

    #[test]
    fn test_parse_error_invalid_character() {
        assert_matches!(
            BuiltinUrl::parse("fuchsia-builtin:///package:1234").unwrap_err(),
            ParseError::InvalidPathSegment(PackagePathSegmentError::InvalidCharacter {
                character: ':'
            })
        );
    }

    #[test]
    fn test_parse_error_host_must_be_empty() {
        assert_matches!(
            BuiltinUrl::parse("fuchsia-builtin://hello").unwrap_err(),
            ParseError::HostMustBeEmpty
        );
    }

    #[test]
    fn test_parse_error_resource_cannot_be_slash() {
        assert_matches!(
            BuiltinUrl::parse("fuchsia-builtin://#/").unwrap_err(),
            ParseError::InvalidResourcePath(ResourcePathError::PathStartsWithSlash)
        );
    }
}
