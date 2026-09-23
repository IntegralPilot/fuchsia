// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::ParseError;
use crate::parse::{PackageName, PackageVariant};
use crate::{FuchsiaPkgAbsolutePackageUrl, RepositoryUrl, Resource, UrlParts};
use fuchsia_hash::Hash;

/// A URL locating a Fuchsia component.
/// Has the form "fuchsia-pkg://<repository>/<name>[/variant][?hash=<hash>]#<resource>" where:
///   * "repository" is a valid hostname
///   * "name" is a valid package name
///   * "variant" is an optional valid package variant
///   * "hash" is an optional valid package hash
///   * "resource" is a valid resource path
/// https://fuchsia.dev/fuchsia-src/concepts/packages/package_url
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FuchsiaPkgAbsoluteComponentUrl {
    package: FuchsiaPkgAbsolutePackageUrl,
    resource: Resource,
}

impl FuchsiaPkgAbsoluteComponentUrl {
    /// Create an FuchsiaPkgAbsoluteComponentUrl from its component parts.
    pub fn new(
        repo: RepositoryUrl,
        name: PackageName,
        variant: Option<PackageVariant>,
        hash: Option<Hash>,
        resource: String,
    ) -> Result<Self, ParseError> {
        let resource = Resource::try_from(resource).map_err(ParseError::InvalidResourcePath)?;
        Ok(Self { package: FuchsiaPkgAbsolutePackageUrl::new(repo, name, variant, hash), resource })
    }

    pub(crate) fn from_parts(parts: UrlParts) -> Result<Self, ParseError> {
        let UrlParts { scheme, host, path, hash, resource } = parts;
        let repo = RepositoryUrl::new(
            scheme.ok_or(ParseError::MissingScheme)?,
            host.ok_or(ParseError::MissingHost)?,
        )?;
        let Some(path) = path else {
            return Err(ParseError::MissingName)?;
        };
        let package = FuchsiaPkgAbsolutePackageUrl::new_with_path(repo, path.as_ref(), hash)?;
        let resource = resource.ok_or(ParseError::MissingResource)?;
        Ok(Self { package, resource })
    }

    /// Parse a "fuchsia-pkg://" URL that locates a component.
    pub fn parse(url: &str) -> Result<Self, ParseError> {
        Self::from_parts(UrlParts::parse(url)?)
    }

    /// Create an `FuchsiaPkgAbsoluteComponentUrl` from a package URL and a resource path.
    pub fn from_package_url_and_resource(
        package: FuchsiaPkgAbsolutePackageUrl,
        resource: String,
    ) -> Result<Self, ParseError> {
        let resource = Resource::try_from(resource).map_err(ParseError::InvalidResourcePath)?;
        Ok(Self { package, resource })
    }

    /// The resource path of this URL.
    pub fn resource(&self) -> &crate::Resource {
        &self.resource
    }

    /// The package URL of this URL (this URL without the resource path).
    pub fn package_url(&self) -> &FuchsiaPkgAbsolutePackageUrl {
        &self.package
    }

    /// Split this component URL into a package URL and a resource.
    pub fn into_package_and_resource(self) -> (FuchsiaPkgAbsolutePackageUrl, Resource) {
        let Self { package, resource } = self;
        (package, resource)
    }
}

// FuchsiaPkgAbsoluteComponentUrl does not maintain any invariants on its `package` field in
// addition to those already maintained by FuchsiaPkgAbsolutePackageUrl so this is safe.
impl std::ops::Deref for FuchsiaPkgAbsoluteComponentUrl {
    type Target = FuchsiaPkgAbsolutePackageUrl;

    fn deref(&self) -> &Self::Target {
        &self.package
    }
}

// FuchsiaPkgAbsoluteComponentUrl does not maintain any invariants on its `package` field in
// addition to those already maintained by FuchsiaPkgAbsolutePackageUrl so this is safe.
impl std::ops::DerefMut for FuchsiaPkgAbsoluteComponentUrl {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.package
    }
}

impl std::str::FromStr for FuchsiaPkgAbsoluteComponentUrl {
    type Err = ParseError;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        Self::parse(url)
    }
}

impl std::convert::TryFrom<&str> for FuchsiaPkgAbsoluteComponentUrl {
    type Error = ParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl std::fmt::Display for FuchsiaPkgAbsoluteComponentUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.package, self.resource.percent_encode())
    }
}

impl serde::Serialize for FuchsiaPkgAbsoluteComponentUrl {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        self.to_string().serialize(ser)
    }
}

impl<'de> serde::Deserialize<'de> for FuchsiaPkgAbsoluteComponentUrl {
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
    use crate::ResourcePathError;
    use crate::errors::PackagePathSegmentError;
    use assert_matches::assert_matches;
    use std::convert::TryFrom as _;

    #[test]
    fn parse_err() {
        for (url, err) in [
            ("example.org/name#resource", ParseError::MissingScheme),
            ("//example.org/name#resource", ParseError::MissingScheme),
            ("///name#resource", ParseError::MissingScheme),
            ("/name#resource", ParseError::MissingScheme),
            ("name#resource", ParseError::MissingScheme),
            ("fuchsia-boot://example.org/name#resource", ParseError::InvalidScheme),
            ("fuchsia-pkg:///name#resource", ParseError::MissingHost),
            ("fuchsia-pkg://exaMple.org/name#resource", ParseError::InvalidHost),
            ("fuchsia-pkg://example.org#resource", ParseError::MissingName),
            (
                "fuchsia-pkg://example.org//#resource",
                ParseError::InvalidPathSegment(PackagePathSegmentError::Empty),
            ),
            (
                "fuchsia-pkg://example.org/name/variant/extra#resource",
                ParseError::ExtraPathSegments,
            ),
            ("fuchsia-pkg://example.org/name#", ParseError::MissingResource),
            (
                "fuchsia-pkg://example.org/name#/",
                ParseError::InvalidResourcePath(ResourcePathError::PathStartsWithSlash),
            ),
            (
                "fuchsia-pkg://example.org/name#resource/",
                ParseError::InvalidResourcePath(ResourcePathError::PathEndsWithSlash),
            ),
        ] {
            assert_matches!(
                FuchsiaPkgAbsoluteComponentUrl::parse(url),
                Err(e) if e == err,
                "the url {:?}", url
            );
            assert_matches!(
                url.parse::<FuchsiaPkgAbsoluteComponentUrl>(),
                Err(e) if e == err,
                "the url {:?}", url
            );
            assert_matches!(
                FuchsiaPkgAbsoluteComponentUrl::try_from(url),
                Err(e) if e == err,
                "the url {:?}", url
            );
            assert_matches!(
                serde_json::from_str::<FuchsiaPkgAbsoluteComponentUrl>(url),
                Err(_),
                "the url {:?}",
                url
            );
        }
    }

    #[test]
    fn parse_ok() {
        for (url, variant, hash, resource) in [
            ("fuchsia-pkg://example.org/name#resource", None, None, "resource"),
            ("fuchsia-pkg://example.org/name/variant#resource", Some("variant"), None, "resource"),
            (
                "fuchsia-pkg://example.org/name?hash=0000000000000000000000000000000000000000000000000000000000000000#resource",
                None,
                Some("0000000000000000000000000000000000000000000000000000000000000000"),
                "resource",
            ),
            ("fuchsia-pkg://example.org/name#%E2%98%BA", None, None, "☺"),
        ] {
            let json_url = format!("\"{url}\"");
            let host = "example.org";
            let name = "name";

            // Creation
            let name = name.parse::<crate::PackageName>().unwrap();
            let variant = variant.map(|v| v.parse::<crate::PackageVariant>().unwrap());
            let hash = hash.map(|h| h.parse::<Hash>().unwrap());
            let resource = resource.parse::<crate::Resource>().unwrap();
            let validate = |parsed: &FuchsiaPkgAbsoluteComponentUrl| {
                assert_eq!(parsed.host(), host);
                assert_eq!(parsed.name(), &name);
                assert_eq!(parsed.variant(), variant.as_ref());
                assert_eq!(parsed.hash(), hash);
                assert_eq!(parsed.resource(), &resource);
            };
            validate(&FuchsiaPkgAbsoluteComponentUrl::parse(url).unwrap());
            validate(&url.parse::<FuchsiaPkgAbsoluteComponentUrl>().unwrap());
            validate(&FuchsiaPkgAbsoluteComponentUrl::try_from(url).unwrap());
            validate(&serde_json::from_str::<FuchsiaPkgAbsoluteComponentUrl>(&json_url).unwrap());

            // Stringification
            assert_eq!(
                FuchsiaPkgAbsoluteComponentUrl::parse(url).unwrap().to_string(),
                url,
                "the url {:?}",
                url
            );
            assert_eq!(
                serde_json::to_string(&FuchsiaPkgAbsoluteComponentUrl::parse(url).unwrap())
                    .unwrap(),
                json_url,
                "the url {:?}",
                url
            );
        }
    }

    #[test]
    // Verify that resource path is validated at all, exhaustive testing of resource path
    // validation is performed by the tests on the `Resource` type.
    fn from_package_url_and_resource_err() {
        for (resource, err) in [
            ("", ParseError::InvalidResourcePath(ResourcePathError::PathIsEmpty)),
            ("/", ParseError::InvalidResourcePath(ResourcePathError::PathStartsWithSlash)),
        ] {
            let package =
                "fuchsia-pkg://example.org/name".parse::<FuchsiaPkgAbsolutePackageUrl>().unwrap();
            assert_eq!(
                FuchsiaPkgAbsoluteComponentUrl::from_package_url_and_resource(
                    package,
                    resource.into()
                ),
                Err(err),
                "the resource {:?}",
                resource
            );
        }
    }

    #[test]
    fn from_package_url_and_resource_ok() {
        let package =
            "fuchsia-pkg://example.org/name".parse::<FuchsiaPkgAbsolutePackageUrl>().unwrap();

        let component = FuchsiaPkgAbsoluteComponentUrl::from_package_url_and_resource(
            package.clone(),
            "resource".into(),
        )
        .unwrap();
        assert_eq!(component.resource().as_ref(), "resource");

        let component = FuchsiaPkgAbsoluteComponentUrl::from_package_url_and_resource(
            package.clone(),
            "☺".into(),
        )
        .unwrap();
        assert_eq!(component.resource().as_ref(), "☺");
    }
}
