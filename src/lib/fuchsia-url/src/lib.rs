// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use fuchsia_hash::{HASH_SIZE, Hash};

pub mod boot;
pub mod builtin_url;
pub mod errors;
mod fuchsia_pkg_absolute_component_url;
mod fuchsia_pkg_absolute_package_url;
mod fuchsia_pkg_component_url;
mod fuchsia_pkg_package_url;
mod fuchsia_pkg_pinned_absolute_package_url;
mod fuchsia_pkg_unpinned_absolute_package_url;
pub(crate) mod generic;
mod host;
mod parse;
mod path;
mod relative_component_url;
mod relative_package_url;
mod repository_url;
mod resource;
pub mod test;

pub use crate::errors::ParseError;
pub use crate::fuchsia_pkg_absolute_component_url::FuchsiaPkgAbsoluteComponentUrl;
pub use crate::fuchsia_pkg_absolute_package_url::FuchsiaPkgAbsolutePackageUrl;
pub use crate::fuchsia_pkg_component_url::FuchsiaPkgComponentUrl;
pub use crate::fuchsia_pkg_package_url::FuchsiaPkgPackageUrl;
pub use crate::fuchsia_pkg_pinned_absolute_package_url::FuchsiaPkgPinnedAbsolutePackageUrl;
pub use crate::fuchsia_pkg_unpinned_absolute_package_url::FuchsiaPkgUnpinnedAbsolutePackageUrl;
pub mod fuchsia_pkg {
    pub use crate::{
        FuchsiaPkgAbsoluteComponentUrl as AbsoluteComponentUrl,
        FuchsiaPkgAbsolutePackageUrl as AbsolutePackageUrl, FuchsiaPkgComponentUrl as ComponentUrl,
        FuchsiaPkgPackageUrl as PackageUrl,
        FuchsiaPkgPinnedAbsolutePackageUrl as PinnedAbsolutePackageUrl,
        FuchsiaPkgUnpinnedAbsolutePackageUrl as UnpinnedAbsolutePackageUrl,
    };
}
pub use crate::generic::{NoneHash, NoneHost};
pub use crate::parse::{MAX_PACKAGE_PATH_SEGMENT_BYTES, PackageName, PackageVariant};
pub use crate::path::Path;
pub(crate) use crate::path::parse_path_to_name_and_variant;
pub use crate::relative_component_url::RelativeComponentUrl;
pub use crate::relative_package_url::RelativePackageUrl;
pub use crate::repository_url::RepositoryUrl;
pub use crate::resource::{Resource, ResourcePathError};

use crate::host::Host;
use percent_encoding::{AsciiSet, CONTROLS};
use std::sync::LazyLock;

/// https://url.spec.whatwg.org/#fragment-percent-encode-set
const FRAGMENT: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'`');

const RELATIVE_SCHEME: &str = "relative";

/// A default base URL from which to parse relative component URL
/// components.
static RELATIVE_BASE: LazyLock<url::Url> =
    LazyLock::new(|| url::Url::parse(&format!("{RELATIVE_SCHEME}:///")).unwrap());

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scheme {
    Builtin,
    FuchsiaPkg,
    FuchsiaBoot,
}

impl std::fmt::Display for Scheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin => builtin_url::SCHEME,
            Self::FuchsiaPkg => repository_url::SCHEME,
            Self::FuchsiaBoot => boot::SCHEME_STR,
        }
        .fmt(f)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct UrlParts {
    scheme: Option<Scheme>,
    host: Option<Host>,
    path: Option<Path>,
    hash: Option<Hash>,
    resource: Option<Resource>,
}

impl UrlParts {
    fn parse(input: &str) -> Result<Self, ParseError> {
        match url::Url::parse(input) {
            Ok(url) => Self::from_url(&url),
            Err(url::ParseError::RelativeUrlWithoutBase) => {
                Self::from_scheme_and_url(None, &RELATIVE_BASE.join(input)?)
            }
            Err(e) => Err(e)?,
        }
    }
    fn from_url(url: &url::Url) -> Result<Self, ParseError> {
        Self::from_scheme_and_url(
            Some(match url.scheme() {
                builtin_url::SCHEME => Scheme::Builtin,
                repository_url::SCHEME => Scheme::FuchsiaPkg,
                boot::SCHEME_STR => Scheme::FuchsiaBoot,
                _ => return Err(ParseError::InvalidScheme),
            }),
            url,
        )
    }
    fn from_scheme_and_url(scheme: Option<Scheme>, url: &url::Url) -> Result<Self, ParseError> {
        if url.port().is_some() {
            return Err(ParseError::CannotContainPort);
        }

        if !url.username().is_empty() {
            return Err(ParseError::CannotContainUsername);
        }

        if url.password().is_some() {
            return Err(ParseError::CannotContainPassword);
        }

        let host = url
            .host_str()
            .filter(|s| !s.is_empty())
            .map(|s| Host::parse(s.to_string()))
            .transpose()?;

        // When parsing URLs, there are three kinds of scheme that affect the parsed host and path:
        //   * File: file
        //   * SpecialNotFile: http, https, ws, wss, ftp
        //   * NotSpecial: <anything else>
        // https://cs.opensource.google/fuchsia/fuchsia/+/main:third_party/rust_crates/vendor/url-2.3.1/src/parser.rs;l=152-168;drc=5d413711388939e4a532a0ab8bfb331e6044ee02
        //
        // | input         | host | path   | cannot-be-a-base | stringified   |
        // |---------------+------+--------+------------------+---------------|
        // | file:text     |      | /text  | false            | file:///text  |
        // | file:/text    |      | /text  | false            | file:///text  |
        // | file://text   | text | /      | false            | file://text/  |
        // | file://text/  | text | /      | false            | file://text/  |
        // | file://text// | text | /      | false            | file://text/  |
        // | file:///text  |      | /text  | false            | file:///text  |
        // | file:///text/ |      | /text/ | false            | file:///text/ |
        // |               |      |        |                  |               |
        // | http:text     | text | /      | false            | http://text/  |
        // | http:/text    | text | /      | false            | http://text/  |
        // | http://text   | text | /      | false            | http://text/  |
        // | http://text/  | text | /      | false            | http://text/  |
        // | http://text// | text | //     | false            | http://text// |
        // | http:///text  | text | /      | false            | http://text/  |
        // | http:///text/ | text | /      | false            | http://text/  |
        // |               |      |        |                  |               |
        // | else:text     |      | text   | true             | else:text     |
        // | else:/text    |      | /text  | false            | else:/text    |
        // | else://text   | text |        | false            | else://text   |
        // | else://text/  | text | /      | false            | else://text/  |
        // | else://text// | text | //     | false            | else://text// |
        // | else:///text  |      | /text  | false            | else:///text  |
        // | else:///text/ |      | /text/ | false            | else:///text/ |
        //
        // Fuchsia uses URLs to encode:
        //   * scheme: which resolver should CM use
        //   * host: which package store should the resolver use
        //   * path: which package from the package store
        //   * hash: exactly specify the package instead of using the store's path -> hash mapping
        //   * resource: which file in the package
        //
        // Because the path is used to identify a package (some named thing in a collection) and
        // we have control over how the things are allowed to be named, we simplify the situation
        // by ignoring the leading slash (i.e. if the path is just a slash we treat the path as
        // absent and if the path starts with a slash we trim the slash) and requiring that packages
        // do not start with a slash.
        let path = if url.path().is_empty() || url.path() == "/" {
            None
        } else {
            Some(url.path().strip_prefix("/").unwrap_or_else(|| url.path()).parse()?)
        };

        let hash = parse_query_pairs(url.query_pairs())?;

        let resource = if let Some(resource) = url.fragment() {
            let resource = percent_encoding::percent_decode(resource.as_bytes())
                .decode_utf8()
                .map_err(ParseError::ResourcePathPercentDecode)?;
            if resource.is_empty() {
                None
            } else {
                Some(
                    Resource::try_from(resource.into_owned())
                        .map_err(ParseError::InvalidResourcePath)?,
                )
            }
        } else {
            None
        };

        Ok(Self { scheme, host, path, hash, resource })
    }
}

/// After all other checks, ensure the input string does not change when joined
/// with the `RELATIVE_BASE` URL, and then removing the base (inverse-join()).
fn validate_inverse_relative_url(input: &str) -> Result<(), ParseError> {
    let relative_url = RELATIVE_BASE.join(input)?;
    let unbased = RELATIVE_BASE.make_relative(&relative_url);
    if Some(input) == unbased.as_deref() {
        Ok(())
    } else {
        Err(ParseError::InvalidRelativePath(input.to_string(), unbased))?
    }
}

fn parse_query_pairs(pairs: url::form_urlencoded::Parse<'_>) -> Result<Option<Hash>, ParseError> {
    let mut query_hash = None;
    for (key, value) in pairs {
        if key == "hash" {
            if query_hash.is_some() {
                return Err(ParseError::MultipleHashes);
            }
            query_hash = Some(value.parse().map_err(ParseError::InvalidHash)?);
            // fuchsia-pkg URLs require lowercase hex characters, but fuchsia_hash::Hash::parse
            // accepts uppercase A-F.
            if !value.bytes().all(|b| (b >= b'0' && b <= b'9') || (b >= b'a' && b <= b'f')) {
                return Err(ParseError::UpperCaseHash);
            }
        } else {
            return Err(ParseError::ExtraQueryParameters);
        }
    }
    Ok(query_hash)
}

/// A URL locating a Fuchsia component. Can be either absolute or relative.
/// See [`AbsoluteComponentUrl`] and [`RelativeComponentUrl`] for more details.
pub type ComponentUrl =
    generic::ComponentUrl<Scheme, generic::OptionHost, Option<Path>, Option<Hash>>;

/// A URL locating a Fuchsia component.
/// Has the form "<scheme>://[host][/<path>][?hash=<hash>]#<resource>" where:
///   * "scheme" is a [`Scheme`]
///   * "host" is an optional [`Host`]
///   * "path" is an optional [`Path`]
///   * "hash" is an optional package [`Hash`]
///   * "resource" is a [`Resource`]
pub type AbsoluteComponentUrl =
    generic::AbsoluteComponentUrl<Scheme, generic::OptionHost, Option<Path>, Option<Hash>>;

/// A URL locating a Fuchsia package. Can be either absolute or relative.
/// See [`AbsolutePackageUrl`] and [`RelativePackageUrl`] for more details.
pub type PackageUrl = generic::PackageUrl<Scheme, generic::OptionHost, Option<Path>, Option<Hash>>;

/// A URL locating a Fuchsia package.
/// Has the form "<scheme>://[host][/<path>][?hash=<hash>]" where:
///   * "scheme" is a [`Scheme`](crate::Scheme)
///   * "host" is an optional [`Host`](crate::Host)
///   * "path" is an optional [`Path`](crate::Path)
///   * "hash" is an optional package [`Hash`](fuchsia_hash::Hash)
pub type AbsolutePackageUrl =
    generic::AbsolutePackageUrl<Scheme, generic::OptionHost, Option<Path>, Option<Hash>>;

// Supertrait for sealing traits in this crate.
trait Sealer {}

#[cfg(test)]
mod test_validate_inverse_relative_url {
    use super::*;
    use assert_matches::assert_matches;

    macro_rules! test_err {
        (
            $(
                $test_name:ident => {
                    path = $path:expr,
                    some_unbased = $some_unbased:expr,
                }
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    let err = ParseError::InvalidRelativePath(
                        $path.to_string(),
                        $some_unbased.map(|s: &str| s.to_string()),
                    );
                    assert_matches!(
                        validate_inverse_relative_url($path),
                        Err(e) if e == err,
                        "the url {:?}; expected = {:?}",
                        $path, err
                    );
                }
            )+
        }
    }

    test_err! {
        err_slash_prefix => {
            path = "/name",
            some_unbased = Some("name"),
        }
        err_three_slashes_prefix => {
            path = "///name",
            some_unbased = Some("name"),
        }
        err_slash_prefix_with_resource => {
            path = "/name#resource",
            some_unbased = Some("name#resource"),
        }
        err_three_slashes_prefix_and_resource => {
            path = "///name#resource",
            some_unbased = Some("name#resource"),
        }
        err_masks_host_must_be_empty_err => {
            path = "//example.org/name",
            some_unbased = None,
        }
        err_dot_masks_missing_name_err => {
            path = ".",
            some_unbased = Some(""),
        }
        err_dot_dot_masks_missing_name_err => {
            path = "..",
            some_unbased = Some(""),
        }
    }

    #[test]
    fn success() {
        for path in ["name", "other3-name", "name#resource", "name#reso%09urce"] {
            let () = validate_inverse_relative_url(path).unwrap();
        }
    }
}

#[cfg(test)]
mod test_url_parts {
    use super::*;
    use assert_matches::assert_matches;

    macro_rules! test_parse_err {
        (
            $(
                $test_name:ident => {
                    url = $url:expr,
                    err = $err:pat,
                }
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    assert_matches!(
                        UrlParts::parse($url),
                        Err($err)
                    );
                }
            )+
        }
    }

    test_parse_err! {
        err_invalid_scheme => {
            url = "bad-scheme://example.org",
            err = ParseError::InvalidScheme,
        }
        err_port => {
            url = "fuchsia-pkg://example.org:1",
            err = ParseError::CannotContainPort,
        }
        err_username => {
            url = "fuchsia-pkg://user@example.org",
            err = ParseError::CannotContainUsername,
        }
        err_password => {
            url = "fuchsia-pkg://:password@example.org",
            err = ParseError::CannotContainPassword,
        }
        err_invalid_host => {
            url = "fuchsia-pkg://exa$mple.org",
            err = ParseError::InvalidHost,
        }
        // Path validation covered by test_validate_path, this just checks that the path is
        // validated at all.
        err_invalid_path => {
            url = "fuchsia-pkg://example.org//",
            err = ParseError::InvalidPathSegment(_),
        }
        err_empty_hash => {
            url = "fuchsia-pkg://example.org/?hash=",
            err = ParseError::InvalidHash(_),
        }
        err_invalid_hash => {
            url = "fuchsia-pkg://example.org/?hash=INVALID_HASH",
            err = ParseError::InvalidHash(_),
        }
        err_uppercase_hash => {
            url = "fuchsia-pkg://example.org/?hash=A000000000000000000000000000000000000000000000000000000000000000",
            err = ParseError::UpperCaseHash,
        }
        err_hash_too_long => {
            url = "fuchsia-pkg://example.org/?hash=00000000000000000000000000000000000000000000000000000000000000001",
            err = ParseError::InvalidHash(_),
        }
        err_hash_too_short => {
            url = "fuchsia-pkg://example.org/?hash=000000000000000000000000000000000000000000000000000000000000000",
            err = ParseError::InvalidHash(_),
        }
        err_multiple_hashes => {
            url = "fuchsia-pkg://example.org/?hash=0000000000000000000000000000000000000000000000000000000000000000&\
            hash=0000000000000000000000000000000000000000000000000000000000000000",
            err = ParseError::MultipleHashes,
        }
        err_non_hash_query_parameter => {
            url = "fuchsia-pkg://example.org/?invalid-key=invalid-value",
            err = ParseError::ExtraQueryParameters,
        }
        err_resource_slash => {
            url = "fuchsia-pkg://example.org/name#/",
            err = ParseError::InvalidResourcePath(ResourcePathError::PathStartsWithSlash),
        }
        err_resource_leading_slash => {
            url = "fuchsia-pkg://example.org/name#/resource",
            err = ParseError::InvalidResourcePath(ResourcePathError::PathStartsWithSlash),
        }
        err_resource_trailing_slash => {
            url = "fuchsia-pkg://example.org/name#resource/",
            err = ParseError::InvalidResourcePath(ResourcePathError::PathEndsWithSlash),
        }
        err_resource_empty_segment => {
            url = "fuchsia-pkg://example.org/name#resource//other",
            err = ParseError::InvalidResourcePath(ResourcePathError::NameEmpty),
        }
        err_resource_bad_segment => {
            url = "fuchsia-pkg://example.org/name#resource/./other",
            err = ParseError::InvalidResourcePath(ResourcePathError::NameIsDot),
        }
        err_resource_percent_encoded_null => {
            url = "fuchsia-pkg://example.org/name#resource%00",
            err = ParseError::InvalidResourcePath(ResourcePathError::NameContainsNull),
        }
        err_resource_unencoded_null => {
            url =  "fuchsia-pkg://example.org/name#reso\x00urce",
            err = ParseError::InvalidResourcePath(ResourcePathError::NameContainsNull),
        }
    }

    macro_rules! test_parse_ok {
        (
            $(
                $test_name:ident => {
                    url = $url:expr,
                    scheme = $scheme:expr,
                    host = $host:expr,
                    path = $path:expr,
                    hash = $hash:expr,
                    resource = $resource:expr,
                }
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    assert_eq!(
                        UrlParts::parse($url).unwrap(),
                        UrlParts {
                            scheme: $scheme,
                            host: $host,
                            path: $path.map(|s| s.parse::<Path>().unwrap()),
                            hash: $hash,
                            resource: $resource,
                        }
                    )
                }
            )+
        }
    }

    test_parse_ok! {
        ok_fuchsia_pkg_scheme => {
            url =  "fuchsia-pkg://",
            scheme = Some(Scheme::FuchsiaPkg),
            host = None,
            path = Option::<&str>::None,
            hash = None,
            resource = None,
        }
        ok_fuchsia_boot_scheme => {
            url =  "fuchsia-boot://",
            scheme = Some(Scheme::FuchsiaBoot),
            host = None,
            path = Option::<&str>::None,
            hash = None,
            resource = None,
        }
        ok_host => {
            url =  "fuchsia-pkg://example.org",
            scheme = Some(Scheme::FuchsiaPkg),
            host = Some(Host::parse("example.org".into()).unwrap()),
            path = Option::<&str>::None,
            hash = None,
            resource = None,
        }
        ok_path_single_segment => {
            url =  "fuchsia-pkg:///name",
            scheme = Some(Scheme::FuchsiaPkg),
            host = None,
            path = Some("name"),
            hash = None,
            resource = None,
        }
        ok_path_multiple_segment => {
            url =  "fuchsia-pkg:///name/variant/other",
            scheme = Some(Scheme::FuchsiaPkg),
            host = None,
            path = Some("name/variant/other"),
            hash = None,
            resource = None,
        }
        ok_hash => {
            url =  "fuchsia-pkg://?hash=0000000000000000000000000000000000000000000000000000000000000000",
            scheme = Some(Scheme::FuchsiaPkg),
            host = None,
            path = Option::<&str>::None,
            hash = Some(
                "0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap()
            ),
            resource = None,
        }
        ok_resource_single_segment => {
            url =  "fuchsia-pkg://#resource",
            scheme = Some(Scheme::FuchsiaPkg),
            host = None,
            path = Option::<&str>::None,
            hash = None,
            resource = Some("resource".parse().unwrap()),
        }
        ok_resource_multiple_segment => {
            url =  "fuchsia-pkg://#resource/again/third",
            scheme = Some(Scheme::FuchsiaPkg),
            host = None,
            path = Option::<&str>::None,
            hash = None,
            resource = Some("resource/again/third".parse().unwrap()),
        }
        ok_resource_encoded_control_character => {
            url =  "fuchsia-pkg://#reso%09urce",
            scheme = Some(Scheme::FuchsiaPkg),
            host = None,
            path = Option::<&str>::None,
            hash = None,
            resource = Some("reso\turce".parse().unwrap()),
        }
        ok_all_fields => {
            url =  "fuchsia-pkg://example.org/name\
            ?hash=0000000000000000000000000000000000000000000000000000000000000000\
            #resource",
            scheme = Some(Scheme::FuchsiaPkg),
            host = Some(Host::parse("example.org".into()).unwrap()),
            path = Some("name"),
            hash = Some(
                "0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap()
            ),
            resource = Some("resource".parse().unwrap()),
        }
        ok_relative_path_single_segment => {
            url =  "name",
            scheme = None,
            host = None,
            path = Some("name"),
            hash = None,
            resource = None,
        }
        ok_relative_path_single_segment_leading_slash => {
            url =  "/name",
            scheme = None,
            host = None,
            path = Some("name"),
            hash = None,
            resource = None,
        }
        ok_relative_path_multiple_segment => {
            url =  "name/variant/other",
            scheme = None,
            host = None,
            path = Some("name/variant/other"),
            hash = None,
            resource = None,
        }
        ok_relative_path_multiple_segment_leading_slash => {
            url =  "/name/variant/other",
            scheme = None,
            host = None,
            path = Some("name/variant/other"),
            hash = None,
            resource = None,
        }
        ok_relative_hash => {
            url =  "?hash=0000000000000000000000000000000000000000000000000000000000000000",
            scheme = None,
            host = None,
            path = Option::<&str>::None,
            hash = Some(
                "0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap()
            ),
            resource = None,
        }
        ok_relative_resource_single_segment => {
            url =  "#resource",
            scheme = None,
            host = None,
            path = Option::<&str>::None,
            hash = None,
            resource = Some("resource".parse().unwrap()),
        }
        ok_relative_resource_multiple_segment => {
            url =  "#resource/again/third",
            scheme = None,
            host = None,
            path = Option::<&str>::None,
            hash = None,
            resource = Some("resource/again/third".parse().unwrap()),
        }
        ok_relative_all_fields => {
            url =  "name\
            ?hash=0000000000000000000000000000000000000000000000000000000000000000\
            #resource",
            scheme = None,
            host = None,
            path = Some("name"),
            hash = Some(
                "0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap()
            ),
            resource = Some("resource".parse().unwrap()),
        }
    }
}

#[cfg(test)]
mod test_url_type_aliases {
    use super::*;

    #[test]
    fn component_url_round_trip() {
        let abs_empty = "fuchsia-boot://#my-resource";
        assert_eq!(ComponentUrl::parse(abs_empty).unwrap().to_string(), abs_empty);

        let abs_present = "fuchsia-boot://my-host/my/path\
            ?hash=0000000000000000000000000000000000000000000000000000000000000000#my-resource";
        assert_eq!(ComponentUrl::parse(abs_present).unwrap().to_string(), abs_present);

        let rel = "my-path#my-resource";
        assert_eq!(ComponentUrl::parse(rel).unwrap().to_string(), rel);
    }

    #[test]
    fn package_url_round_trip() {
        let abs_empty = "fuchsia-boot://";
        assert_eq!(PackageUrl::parse(abs_empty).unwrap().to_string(), abs_empty);

        let abs_present = "fuchsia-boot://my-host/my/path\
            ?hash=0000000000000000000000000000000000000000000000000000000000000000";
        assert_eq!(PackageUrl::parse(abs_present).unwrap().to_string(), abs_present);

        let rel = "my-path";
        assert_eq!(PackageUrl::parse(rel).unwrap().to_string(), rel);
    }
}
