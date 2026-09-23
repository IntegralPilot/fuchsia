// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::ParseError;
use crate::parse::{PackageName, PackageVariant};
use crate::{FuchsiaPkgAbsolutePackageUrl, RepositoryUrl};

/// A URL locating a Fuchsia package. Cannot have a hash.
/// Has the form "fuchsia-pkg://<repository>/<name>[/variant]" where:
///   * "repository" is a valid hostname
///   * "name" is a valid package name
///   * "variant" is an optional valid package variant
/// https://fuchsia.dev/fuchsia-src/concepts/packages/package_url
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FuchsiaPkgUnpinnedAbsolutePackageUrl {
    repo: RepositoryUrl,
    name: PackageName,
    // TODO(https://fxbug.dev/335388895): Remove variant concept
    variant: Option<PackageVariant>,
}

impl FuchsiaPkgUnpinnedAbsolutePackageUrl {
    /// Create an FuchsiaPkgUnpinnedAbsolutePackageUrl from its component parts.
    pub fn new(repo: RepositoryUrl, name: PackageName, variant: Option<PackageVariant>) -> Self {
        Self { repo, name, variant }
    }

    /// Create an FuchsiaPkgUnpinnedAbsolutePackageUrl from a RepositoryUrl and a &str `path` that
    /// will be validated.
    pub fn new_with_path(repo: RepositoryUrl, path: &str) -> Result<Self, ParseError> {
        let (name, variant) = crate::parse_path_to_name_and_variant(path)?;
        Ok(Self::new(repo, name, variant))
    }

    /// Parse a "fuchsia-pkg://" URL that locates an unpinned (no hash query parameter) package.
    pub fn parse(url: &str) -> Result<Self, ParseError> {
        match FuchsiaPkgAbsolutePackageUrl::parse(url)? {
            FuchsiaPkgAbsolutePackageUrl::Unpinned(unpinned) => Ok(unpinned),
            FuchsiaPkgAbsolutePackageUrl::Pinned(_) => Err(ParseError::CannotContainHash),
        }
    }

    /// The Repository URL behind this URL (this URL without the path).
    pub fn repository(&self) -> &RepositoryUrl {
        &self.repo
    }

    /// The package name.
    pub fn name(&self) -> &PackageName {
        &self.name
    }

    /// The optional package variant.
    pub fn variant(&self) -> Option<&PackageVariant> {
        self.variant.as_ref()
    }

    /// The path ("name[/variant]").
    pub fn path(&self) -> String {
        match &self.variant {
            Some(variant) => format!("{}/{}", self.name, variant),
            None => self.name.to_string(),
        }
    }

    /// Change the repository to `repository`.
    pub fn set_repository(&mut self, repository: RepositoryUrl) -> &mut Self {
        self.repo = repository;
        self
    }

    /// Clear the variant if there is one.
    pub fn clear_variant(&mut self) -> &mut Self {
        self.variant = None;
        self
    }
}

// FuchsiaPkgUnpinnedAbsolutePackageUrl does not maintain any invariants on its `repo` field in
// addition to those already maintained by RepositoryUrl so this is safe.
impl std::ops::Deref for FuchsiaPkgUnpinnedAbsolutePackageUrl {
    type Target = RepositoryUrl;

    fn deref(&self) -> &Self::Target {
        &self.repo
    }
}

impl std::str::FromStr for FuchsiaPkgUnpinnedAbsolutePackageUrl {
    type Err = ParseError;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        Self::parse(url)
    }
}

impl std::convert::TryFrom<&str> for FuchsiaPkgUnpinnedAbsolutePackageUrl {
    type Error = ParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl std::fmt::Display for FuchsiaPkgUnpinnedAbsolutePackageUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let () = write!(f, "{}/{}", self.repo, self.name)?;
        if let Some(variant) = &self.variant {
            let () = write!(f, "/{}", variant)?;
        }
        Ok(())
    }
}

impl serde::Serialize for FuchsiaPkgUnpinnedAbsolutePackageUrl {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        self.to_string().serialize(ser)
    }
}

impl<'de> serde::Deserialize<'de> for FuchsiaPkgUnpinnedAbsolutePackageUrl {
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
    fn new_with_path_err() {
        for (path, err) in [
            ("", ParseError::MissingName),
            ("/", ParseError::InvalidName(PackagePathSegmentError::Empty)),
            ("name/variant/other", ParseError::ExtraPathSegments),
        ] {
            assert_matches!(
                FuchsiaPkgUnpinnedAbsolutePackageUrl::new_with_path(
                    "fuchsia-pkg://example.org".parse().unwrap(),
                    path.into(),
                ),
                Err(e) if e == err,
                "the path {:?}", path
            );
        }
    }

    #[test]
    fn new_with_path_ok() {
        let repo = "fuchsia-pkg://example.org".parse::<RepositoryUrl>().unwrap();
        let url = FuchsiaPkgUnpinnedAbsolutePackageUrl::new_with_path(repo.clone(), "name".into())
            .unwrap();
        assert_eq!(url.name().as_ref(), "name");
        assert_eq!(url.variant(), None);

        let url = FuchsiaPkgUnpinnedAbsolutePackageUrl::new_with_path(
            repo.clone(),
            "name/variant".into(),
        )
        .unwrap();
        assert_eq!(url.name().as_ref(), "name");
        assert_eq!(url.variant().map(|v| v.as_ref()), Some("variant"));
    }

    #[test]
    fn parse_err() {
        for (url, err) in [
            ("fuchsia-boot://example.org/name", ParseError::InvalidScheme),
            ("fuchsia-pkg://", ParseError::MissingHost),
            ("fuchsia-pkg://exaMple.org", ParseError::InvalidHost),
            ("fuchsia-pkg://example.org/", ParseError::MissingName),
            (
                "fuchsia-pkg://example.org//",
                ParseError::InvalidPathSegment(PackagePathSegmentError::Empty),
            ),
            ("fuchsia-pkg://example.org/name/variant/extra", ParseError::ExtraPathSegments),
            ("fuchsia-pkg://example.org/name#resource", ParseError::CannotContainResource),
            (
                "fuchsia-pkg://example.org/name?hash=0000000000000000000000000000000000000000000000000000000000000000",
                ParseError::CannotContainHash,
            ),
        ] {
            assert_matches!(
                FuchsiaPkgUnpinnedAbsolutePackageUrl::parse(url),
                Err(e) if e == err,
                "the url {:?}", url
            );
            assert_matches!(
                url.parse::<FuchsiaPkgUnpinnedAbsolutePackageUrl>(),
                Err(e) if e == err,
                "the url {:?}", url
            );
            assert_matches!(
                FuchsiaPkgUnpinnedAbsolutePackageUrl::try_from(url),
                Err(e) if e == err,
                "the url {:?}", url
            );
            assert_matches!(
                serde_json::from_str::<FuchsiaPkgUnpinnedAbsolutePackageUrl>(url),
                Err(_),
                "the url {:?}",
                url
            );
        }
    }

    #[test]
    fn parse_ok() {
        for (url, host, name, variant, path) in [
            ("fuchsia-pkg://example.org/name", "example.org", "name", None, "name"),
            (
                "fuchsia-pkg://example.org/name/variant",
                "example.org",
                "name",
                Some("variant"),
                "name/variant",
            ),
        ] {
            let json_url = format!("\"{url}\"");

            // Creation
            let name = name.parse::<crate::PackageName>().unwrap();
            let variant = variant.map(|v| v.parse::<crate::PackageVariant>().unwrap());
            let validate = |parsed: &FuchsiaPkgUnpinnedAbsolutePackageUrl| {
                assert_eq!(parsed.host(), host);
                assert_eq!(parsed.name(), &name);
                assert_eq!(parsed.variant(), variant.as_ref());
                assert_eq!(parsed.path(), path);
            };
            validate(&FuchsiaPkgUnpinnedAbsolutePackageUrl::parse(url).unwrap());
            validate(&url.parse::<FuchsiaPkgUnpinnedAbsolutePackageUrl>().unwrap());
            validate(&FuchsiaPkgUnpinnedAbsolutePackageUrl::try_from(url).unwrap());
            validate(
                &serde_json::from_str::<FuchsiaPkgUnpinnedAbsolutePackageUrl>(&json_url).unwrap(),
            );

            // Stringification
            assert_eq!(
                FuchsiaPkgUnpinnedAbsolutePackageUrl::parse(url).unwrap().to_string(),
                url,
                "the url {:?}",
                url
            );
            assert_eq!(
                serde_json::to_string(&FuchsiaPkgUnpinnedAbsolutePackageUrl::parse(url).unwrap())
                    .unwrap(),
                json_url,
                "the url {:?}",
                url
            );
        }
    }

    #[test]
    fn set_repository() {
        let mut url =
            FuchsiaPkgUnpinnedAbsolutePackageUrl::parse("fuchsia-pkg://example.org/name").unwrap();

        url.set_repository("fuchsia-pkg://example.com".parse().unwrap());

        assert_eq!(url.host(), "example.com");
    }

    #[test]
    fn clear_variant() {
        let mut url =
            FuchsiaPkgUnpinnedAbsolutePackageUrl::parse("fuchsia-pkg://example.org/name/variant")
                .unwrap();
        url.clear_variant();
        assert_eq!(url.variant(), None);

        let mut url =
            FuchsiaPkgUnpinnedAbsolutePackageUrl::parse("fuchsia-pkg://example.org/name").unwrap();
        url.clear_variant();
        assert_eq!(url.variant(), None);
    }
}
