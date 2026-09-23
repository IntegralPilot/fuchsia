// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const SCHEME_STR: &str = "fuchsia-boot";

use crate::generic;

/// A component URL for the boot resolver. Either an [`AbsoluteComponentUrl`] or a
/// [`RelativeComponentUrl`](crate::RelativeComponentUrl).
pub type ComponentUrl =
    generic::ComponentUrl<Scheme, generic::NoneHost, Option<crate::Path>, generic::NoneHash>;

/// An absolute component URL for the boot resolver, composed of:
///   * scheme == "fuchsia-boot"
///   * no host
///   * an optional [`Path`](crate::Path). Unpackaged bootfs components, of which there are few
///     remaining, do not have a `Path`.
///   * no hash
///   * a [`Resource`](crate::Resource)
pub type AbsoluteComponentUrl = generic::AbsoluteComponentUrl<
    Scheme,
    generic::NoneHost,
    Option<crate::Path>,
    generic::NoneHash,
>;

impl AbsoluteComponentUrl {
    pub fn new(path: Option<crate::Path>, resource: crate::Resource) -> Self {
        Self::from_parts(Scheme, crate::NoneHost, path, crate::NoneHash, resource)
    }
}

/// A package URL for the boot resolver. Either an [`AbsolutePackageUrl`] or a
/// [`RelativePackageUrl`](crate::RelativePackageUrl).
pub type PackageUrl =
    generic::PackageUrl<Scheme, generic::NoneHost, Option<crate::Path>, generic::NoneHash>;

/// An absolute package URL for the boot resolver, composed of:
///   * scheme == "fuchsia-boot"
///   * no host
///   * an optional [`Path`](crate::Path). Unpackaged bootfs components, of which there are few
///     remaining, do not have a `Path` (since they don't have a package), and so their package URL
///     is the degenerate pathless URL.
///   * no hash
pub type AbsolutePackageUrl =
    generic::AbsolutePackageUrl<Scheme, generic::NoneHost, Option<crate::Path>, generic::NoneHash>;

/// Scheme type for fuchsia-boot URLs.
#[derive(Debug, Clone)]
pub struct Scheme;
impl crate::Sealer for Scheme {}
impl generic::SchemeTrait for Scheme {
    fn try_from_part(scheme: crate::Scheme) -> Result<Self, crate::ParseError> {
        match scheme {
            crate::Scheme::FuchsiaBoot => Ok(Self),
            _ => Err(crate::ParseError::InvalidScheme),
        }
    }
}
impl std::fmt::Display for Scheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(SCHEME_STR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_url_round_trip() {
        let abs = "fuchsia-boot:///my/path#my-resource";
        assert_eq!(ComponentUrl::parse(abs).unwrap().to_string(), abs);

        let abs_pathless = "fuchsia-boot://#my-resource";
        assert_eq!(ComponentUrl::parse(abs_pathless).unwrap().to_string(), abs_pathless);

        let rel = "my-path#my-resource";
        assert_eq!(ComponentUrl::parse(rel).unwrap().to_string(), rel);
    }

    #[test]
    fn package_url_round_trip() {
        let abs = "fuchsia-boot:///my/path";
        assert_eq!(PackageUrl::parse(abs).unwrap().to_string(), abs);

        let rel = "my-path";
        assert_eq!(PackageUrl::parse(rel).unwrap().to_string(), rel);
    }
}
