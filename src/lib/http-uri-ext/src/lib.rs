// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use http::uri::{self, Uri};

pub trait HttpUriExt {
    /// Appends `/` to `self`'s path if it does not already end with one, then appends `path`,
    /// preserving any query parameters. Does nothing if `path` is the empty string. Note that
    /// empty paths on URIs with authorities are implicitly normalized to start with `/` by
    /// `http::Uri`.
    ///
    /// Will error if `path` contains invalid URI characters, or if the resulting URI parts are
    /// invalid (e.g., combining an authority and path without a scheme).
    fn extend_dir_with_path(self, path: &str) -> Result<Uri, Error>;

    /// Append the given query parameter `key`=`value` to the URI, preserving existing query
    /// parameters if any, `key` and `value` should already be URL-encoded (if necessary).
    ///
    /// Will only error if `key` or `value` contains invalid URI characters.
    fn append_query_parameter(self, key: &str, value: &str) -> Result<Uri, Error>;

    /// Joins a relative URI or path to this base URI.
    /// Similar to `url::Url::join` but for `http::Uri`.
    fn join(&self, relative: &str) -> Result<Uri, Error>;
}

impl HttpUriExt for Uri {
    fn extend_dir_with_path(self, path: &str) -> Result<Uri, Error> {
        if path.is_empty() {
            return Ok(self);
        }
        let mut base_parts = self.into_parts();
        let (base_path, query) = match &base_parts.path_and_query {
            Some(path_and_query) => (path_and_query.path(), path_and_query.query()),
            None => ("/", None),
        };
        let new_path_and_query = if base_path.ends_with('/') {
            if let Some(query) = query {
                format!("{base_path}{path}?{query}")
            } else {
                format!("{base_path}{path}")
            }
        } else {
            if let Some(query) = query {
                format!("{base_path}/{path}?{query}")
            } else {
                format!("{base_path}/{path}")
            }
        };
        base_parts.path_and_query = Some(new_path_and_query.parse()?);
        Ok(Uri::from_parts(base_parts)?)
    }

    fn append_query_parameter(self, key: &str, value: &str) -> Result<Uri, Error> {
        let mut base_parts = self.into_parts();
        let new_path_and_query = match &base_parts.path_and_query {
            Some(path_and_query) => {
                if let Some(query) = path_and_query.query() {
                    format!("{}?{query}&{key}={value}", path_and_query.path())
                } else {
                    format!("{}?{key}={value}", path_and_query.path())
                }
            }
            None => format!("?{key}={value}"),
        };
        base_parts.path_and_query = Some(new_path_and_query.parse()?);
        Ok(Uri::from_parts(base_parts)?)
    }

    fn join(&self, relative: &str) -> Result<Uri, Error> {
        if let Ok(rel_uri) = relative.parse::<Uri>() {
            if rel_uri.scheme().is_some() {
                return Ok(rel_uri);
            }
        }

        if relative.starts_with("//") {
            let temp_uri = format!("http:{relative}").parse::<Uri>()?;
            let mut temp_parts = temp_uri.into_parts();
            temp_parts.scheme = self.scheme().cloned();
            return Ok(Uri::from_parts(temp_parts)?);
        }

        let (base_path, query) = match self.path_and_query() {
            Some(path_and_query) => (path_and_query.path(), path_and_query.query()),
            None => ("/", None),
        };

        let new_path = if relative.starts_with('/') {
            relative.to_string()
        } else {
            if let Some((base_dir, _)) = base_path.rsplit_once('/') {
                normalize_path(&format!("{base_dir}/{relative}"))
            } else {
                normalize_path(&format!("/{relative}"))
            }
        };

        let new_path_and_query =
            if let Some(query) = query { format!("{new_path}?{query}") } else { new_path };

        let mut base_parts = self.clone().into_parts();
        base_parts.path_and_query = Some(new_path_and_query.parse()?);
        Ok(Uri::from_parts(base_parts)?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid uri")]
    InvalidUri(#[from] uri::InvalidUri),

    #[error("invalid uri parts")]
    InvalidUriParts(#[from] uri::InvalidUriParts),
}

fn normalize_path(path: &str) -> String {
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            _ => {
                segments.push(segment);
            }
        }
    }
    let mut joined = segments.join("/");
    if path.starts_with('/') {
        joined.insert(0, '/');
    }
    if path.ends_with('/') {
        joined.push('/');
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    fn make_uri_from_path_and_query(path_and_query: Option<&str>) -> Uri {
        let mut parts = uri::Parts::default();
        parts.path_and_query = path_and_query.map(|p| p.parse().unwrap());
        Uri::from_parts(parts).unwrap()
    }

    fn assert_expected_path(base: Option<&str>, added: &str, expected: Option<&str>) {
        let uri = make_uri_from_path_and_query(base).extend_dir_with_path(added).unwrap();
        assert_eq!(
            uri.into_parts().path_and_query.map(|p| p.to_string()),
            expected.map(|s| s.to_string())
        );
    }

    #[test]
    fn no_query_empty_argument() {
        assert_expected_path(None, "", None);
        assert_expected_path(Some("/"), "", Some("/"));
        assert_expected_path(Some("/a"), "", Some("/a"));
        assert_expected_path(Some("/a/"), "", Some("/a/"));
    }

    #[test]
    fn has_query_empty_argument() {
        assert_expected_path(Some("?k=v"), "", Some("/?k=v"));
        assert_expected_path(Some("/?k=v"), "", Some("/?k=v"));
        assert_expected_path(Some("/a?k=v"), "", Some("/a?k=v"));
        assert_expected_path(Some("/a/?k=v"), "", Some("/a/?k=v"));
    }

    #[test]
    fn no_query_has_argument() {
        assert_expected_path(None, "c", Some("/c"));
        assert_expected_path(Some("/"), "c", Some("/c"));
        assert_expected_path(Some("/a"), "c", Some("/a/c"));
        assert_expected_path(Some("/a/"), "c", Some("/a/c"));
    }

    #[test]
    fn has_query_has_argument() {
        assert_expected_path(Some("?k=v"), "c", Some("/c?k=v"));
        assert_expected_path(Some("/?k=v"), "c", Some("/c?k=v"));
        assert_expected_path(Some("/a?k=v"), "c", Some("/a/c?k=v"));
        assert_expected_path(Some("/a/?k=v"), "c", Some("/a/c?k=v"));
    }

    #[test]
    fn extend_dir_with_authority() {
        let uri = "http://example.com".parse::<Uri>().unwrap();
        let extended = uri.extend_dir_with_path("foo").unwrap();
        assert_eq!(extended.to_string(), "http://example.com/foo");

        let uri = "http://example.com/".parse::<Uri>().unwrap();
        let extended = uri.extend_dir_with_path("foo").unwrap();
        assert_eq!(extended.to_string(), "http://example.com/foo");

        let uri = "http://example.com?k=v".parse::<Uri>().unwrap();
        let extended = uri.extend_dir_with_path("foo").unwrap();
        assert_eq!(extended.to_string(), "http://example.com/foo?k=v");
    }

    fn assert_expected_param(base: Option<&str>, key: &str, value: &str, expected: Option<&str>) {
        let uri = make_uri_from_path_and_query(base).append_query_parameter(key, value).unwrap();
        assert_eq!(
            uri.into_parts().path_and_query.map(|p| p.to_string()),
            expected.map(|s| s.to_string())
        );
    }

    #[test]
    fn new_query() {
        assert_expected_param(None, "k", "v", Some("/?k=v"));
        assert_expected_param(Some("/"), "k", "v", Some("/?k=v"));
        assert_expected_param(Some("/a"), "k", "v", Some("/a?k=v"));
        assert_expected_param(Some("/a/"), "k", "v", Some("/a/?k=v"));
    }

    #[test]
    fn append_query() {
        assert_expected_param(Some("?k=v"), "k2", "v2", Some("/?k=v&k2=v2"));
        assert_expected_param(Some("/?k=v"), "k2", "v2", Some("/?k=v&k2=v2"));
        assert_expected_param(Some("/a?k=v"), "k2", "v2", Some("/a?k=v&k2=v2"));
        assert_expected_param(Some("/a/?k=v"), "k2", "v2", Some("/a/?k=v&k2=v2"));
    }

    #[test_case("https://[fe80::1%25eth0]:8080/update/manifest", "http://example.com/foo", "http://example.com/foo"; "absolute")]
    #[test_case("https://[fe80::1%25eth0]:8080/update/manifest", "blobs", "https://[fe80::1%25eth0]:8080/update/blobs"; "relative_path")]
    #[test_case("https://[fe80::1%25eth0]:8080/update/manifest", "./blobs", "https://[fe80::1%25eth0]:8080/update/blobs"; "relative_starts_with_dot_slash")]
    #[test_case("https://[fe80::1%25eth0]:8080/update/manifest", "../blobs", "https://[fe80::1%25eth0]:8080/blobs"; "relative_parent")]
    #[test_case("https://[fe80::1%25eth0]:8080/update/manifest", "/blobs", "https://[fe80::1%25eth0]:8080/blobs"; "absolute_path")]
    #[test_case("https://[fe80::1%25eth0]:8080/update/manifest", "//fuchsia.com/blobs/1", "https://fuchsia.com/blobs/1"; "network_path")]
    #[test_case("https://[fe80::1%25eth0]:8080/update/", "blobs", "https://[fe80::1%25eth0]:8080/update/blobs"; "base_ends_in_slash")]
    #[test_case("https://[fe80::1%25eth0]:8080/update/", "./blobs", "https://[fe80::1%25eth0]:8080/update/blobs"; "base_ends_in_slash_relative_starts_with_dot_slash")]
    #[test_case("https://[fe80::1%25eth0]:8080/update/", "../blobs", "https://[fe80::1%25eth0]:8080/blobs"; "base_ends_in_slash_relative_parent")]
    fn test_join(base: &str, relative: &str, expected: &str) {
        let base = base.parse::<Uri>().unwrap();
        let joined = base.join(relative).unwrap();
        assert_eq!(joined.to_string(), expected);
    }
}
