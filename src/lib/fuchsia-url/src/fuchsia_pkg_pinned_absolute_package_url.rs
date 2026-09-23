// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::ParseError;
use crate::parse::{PackageName, PackageVariant};
use crate::{FuchsiaPkgAbsolutePackageUrl, FuchsiaPkgUnpinnedAbsolutePackageUrl, RepositoryUrl};
use fuchsia_hash::Hash;

/// A URL locating a Fuchsia package. Must have a hash.
/// Has the form "fuchsia-pkg://<repository>/<name>[/variant]?hash=<hash>" where:
///   * "repository" is a valid hostname
///   * "name" is a valid package name
///   * "variant" is an optional valid package variant
///   * "hash" is a valid package hash
/// https://fuchsia.dev/fuchsia-src/concepts/packages/package_url
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FuchsiaPkgPinnedAbsolutePackageUrl {
    unpinned: FuchsiaPkgUnpinnedAbsolutePackageUrl,
    hash: Hash,
}

impl FuchsiaPkgPinnedAbsolutePackageUrl {
    /// Create a `FuchsiaPkgPinnedAbsolutePackageUrl` from its component parts.
    pub fn new(
        repo: RepositoryUrl,
        name: PackageName,
        variant: Option<PackageVariant>,
        hash: Hash,
    ) -> Self {
        Self { unpinned: FuchsiaPkgUnpinnedAbsolutePackageUrl::new(repo, name, variant), hash }
    }

    /// Create a FuchsiaPkgPinnedAbsolutePackageUrl from its component parts and a &str `path` that
    /// will be validated.
    pub fn new_with_path(repo: RepositoryUrl, path: &str, hash: Hash) -> Result<Self, ParseError> {
        Ok(Self {
            unpinned: FuchsiaPkgUnpinnedAbsolutePackageUrl::new_with_path(repo, path)?,
            hash,
        })
    }

    /// Parse a "fuchsia-pkg://" URL that locates a pinned (has a hash query parameter) package.
    pub fn parse(url: &str) -> Result<Self, ParseError> {
        match FuchsiaPkgAbsolutePackageUrl::parse(url)? {
            FuchsiaPkgAbsolutePackageUrl::Unpinned(_) => Err(ParseError::MissingHash),
            FuchsiaPkgAbsolutePackageUrl::Pinned(pinned) => Ok(pinned),
        }
    }

    /// Create a `FuchsiaPkgPinnedAbsolutePackageUrl` from an unpinned url and a hash.
    pub fn from_unpinned(unpinned: FuchsiaPkgUnpinnedAbsolutePackageUrl, hash: Hash) -> Self {
        Self { unpinned, hash }
    }

    /// Split this URL into an unpinned URL and hash.
    pub fn into_unpinned_and_hash(self) -> (FuchsiaPkgUnpinnedAbsolutePackageUrl, Hash) {
        let Self { unpinned, hash } = self;
        (unpinned, hash)
    }

    /// The URL without the hash.
    pub fn as_unpinned(&self) -> &FuchsiaPkgUnpinnedAbsolutePackageUrl {
        &self.unpinned
    }

    /// The URL's hash.
    pub fn hash(&self) -> Hash {
        self.hash
    }

    /// Change the repository to `repository`.
    pub fn set_repository(&mut self, repository: RepositoryUrl) -> &mut Self {
        self.unpinned.set_repository(repository);
        self
    }
}

// FuchsiaPkgPinnedAbsolutePackageUrl does not maintain any invariants on its `unpinned` field in
// addition to those already maintained by FuchsiaPkgUnpinnedAbsolutePackageUrl so this is safe.
impl std::ops::Deref for FuchsiaPkgPinnedAbsolutePackageUrl {
    type Target = FuchsiaPkgUnpinnedAbsolutePackageUrl;

    fn deref(&self) -> &Self::Target {
        &self.unpinned
    }
}

// FuchsiaPkgPinnedAbsolutePackageUrl does not maintain any invariants on its `unpinned` field in
// addition to those already maintained by FuchsiaPkgUnpinnedAbsolutePackageUrl so this is safe.
impl std::ops::DerefMut for FuchsiaPkgPinnedAbsolutePackageUrl {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.unpinned
    }
}

impl std::str::FromStr for FuchsiaPkgPinnedAbsolutePackageUrl {
    type Err = ParseError;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        Self::parse(url)
    }
}

impl std::convert::TryFrom<&str> for FuchsiaPkgPinnedAbsolutePackageUrl {
    type Error = ParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl std::fmt::Display for FuchsiaPkgPinnedAbsolutePackageUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}?hash={}", self.unpinned, self.hash)
    }
}

impl serde::Serialize for FuchsiaPkgPinnedAbsolutePackageUrl {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        self.to_string().serialize(ser)
    }
}

impl<'de> serde::Deserialize<'de> for FuchsiaPkgPinnedAbsolutePackageUrl {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let url = String::deserialize(de)?;
        Ok(Self::parse(&url).map_err(|err| serde::de::Error::custom(err))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::PackagePathSegmentError;
    use assert_matches::assert_matches;
    use std::convert::TryFrom as _;

    #[test]
    fn parse_err() {
        for (url, err) in [
            (
                "fuchsia-boot://example.org/name?hash=0000000000000000000000000000000000000000000000000000000000000000",
                ParseError::InvalidScheme,
            ),
            (
                "fuchsia-pkg://?hash=0000000000000000000000000000000000000000000000000000000000000000",
                ParseError::MissingHost,
            ),
            (
                "fuchsia-pkg://exaMple.org?hash=0000000000000000000000000000000000000000000000000000000000000000",
                ParseError::InvalidHost,
            ),
            (
                "fuchsia-pkg://example.org/?hash=0000000000000000000000000000000000000000000000000000000000000000",
                ParseError::MissingName,
            ),
            (
                "fuchsia-pkg://example.org//?hash=0000000000000000000000000000000000000000000000000000000000000000",
                ParseError::InvalidPathSegment(PackagePathSegmentError::Empty),
            ),
            (
                "fuchsia-pkg://example.org/name/variant/extra?hash=0000000000000000000000000000000000000000000000000000000000000000",
                ParseError::ExtraPathSegments,
            ),
            (
                "fuchsia-pkg://example.org/name?hash=0000000000000000000000000000000000000000000000000000000000000000#resource",
                ParseError::CannotContainResource,
            ),
        ] {
            assert_matches!(
                FuchsiaPkgPinnedAbsolutePackageUrl::parse(url),
                Err(e) if e == err,
                "the url {:?}", url
            );
            assert_matches!(
                url.parse::<FuchsiaPkgPinnedAbsolutePackageUrl>(),
                Err(e) if e == err,
                "the url {:?}", url
            );
            assert_matches!(
                FuchsiaPkgPinnedAbsolutePackageUrl::try_from(url),
                Err(e) if e == err,
                "the url {:?}", url
            );
            assert_matches!(
                serde_json::from_str::<FuchsiaPkgPinnedAbsolutePackageUrl>(url),
                Err(_),
                "the url {:?}",
                url
            );
        }
    }

    #[test]
    fn parse_ok() {
        for (url, variant, path) in [
            (
                "fuchsia-pkg://example.org/name?hash=0000000000000000000000000000000000000000000000000000000000000000",
                None,
                "name",
            ),
            (
                "fuchsia-pkg://example.org/name/variant?hash=0000000000000000000000000000000000000000000000000000000000000000",
                Some("variant"),
                "name/variant",
            ),
        ] {
            let json_url = format!("\"{url}\"");
            let host = "example.org";
            let name = "name";
            let hash = "0000000000000000000000000000000000000000000000000000000000000000"
                .parse::<Hash>()
                .unwrap();

            // Creation
            let name = name.parse::<crate::PackageName>().unwrap();
            let variant = variant.map(|v| v.parse::<crate::PackageVariant>().unwrap());
            let validate = |parsed: &FuchsiaPkgPinnedAbsolutePackageUrl| {
                assert_eq!(parsed.host(), host);
                assert_eq!(parsed.name(), &name);
                assert_eq!(parsed.variant(), variant.as_ref());
                assert_eq!(parsed.path(), path);
                assert_eq!(parsed.hash(), hash);
            };
            validate(&FuchsiaPkgPinnedAbsolutePackageUrl::parse(url).unwrap());
            validate(&url.parse::<FuchsiaPkgPinnedAbsolutePackageUrl>().unwrap());
            validate(&FuchsiaPkgPinnedAbsolutePackageUrl::try_from(url).unwrap());
            validate(
                &serde_json::from_str::<FuchsiaPkgPinnedAbsolutePackageUrl>(&json_url).unwrap(),
            );

            // Stringification
            assert_eq!(
                FuchsiaPkgPinnedAbsolutePackageUrl::parse(url).unwrap().to_string(),
                url,
                "the url {:?}",
                url
            );
            assert_eq!(
                serde_json::to_string(&FuchsiaPkgPinnedAbsolutePackageUrl::parse(url).unwrap())
                    .unwrap(),
                json_url,
                "the url {:?}",
                url
            );
        }
    }
}
