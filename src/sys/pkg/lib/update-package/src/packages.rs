// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use fuchsia_url::fuchsia_pkg::PinnedAbsolutePackageUrl;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "version", content = "content", deny_unknown_fields)]
enum Packages {
    #[serde(rename = "1")]
    V1(Vec<PinnedAbsolutePackageUrl>),
}

/// ParsePackageError represents any error which might occur while reading
/// `packages.json` from an update package.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum ParsePackageError {
    #[error("could not open `packages.json`")]
    FailedToOpen(#[source] fuchsia_fs::node::OpenError),

    #[error("could not parse url from line: {0:?}")]
    URLParseError(String, #[source] fuchsia_url::errors::ParseError),

    #[error("error reading file `packages.json`")]
    ReadError(#[source] fuchsia_fs::file::ReadError),

    #[error("json parsing error while reading `packages.json`")]
    JsonError(#[source] serde_json::error::Error),
}

/// SerializePackageError represents any error which might occur while writing
/// `packages.json` for an update package.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum SerializePackageError {
    #[error("serialization error while constructing `packages.json`")]
    JsonError(#[source] serde_json::error::Error),
}

/// Returns structured `packages.json` data based on file contents string.
pub fn parse_packages_json(
    contents: &[u8],
) -> Result<Vec<PinnedAbsolutePackageUrl>, ParsePackageError> {
    match serde_json::from_slice(contents).map_err(ParsePackageError::JsonError)? {
        Packages::V1(packages) => Ok(packages),
    }
}

/// Returns serialized `packages.json` contents based package URLs.
pub fn serialize_packages_json(
    pkg_urls: &[PinnedAbsolutePackageUrl],
) -> Result<Vec<u8>, SerializePackageError> {
    serde_json::to_vec(&Packages::V1(pkg_urls.into())).map_err(SerializePackageError::JsonError)
}

/// Returns the list of package urls that go in the universe of this update package.
pub(crate) async fn packages(
    proxy: &fio::DirectoryProxy,
) -> Result<Vec<PinnedAbsolutePackageUrl>, ParsePackageError> {
    let file = fuchsia_fs::directory::open_file(proxy, "packages.json", fio::PERM_READABLE)
        .await
        .map_err(ParsePackageError::FailedToOpen)?;

    let contents = fuchsia_fs::file::read(&file).await.map_err(ParsePackageError::ReadError)?;
    parse_packages_json(&contents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestUpdatePackage;
    use assert_matches::assert_matches;
    use serde_json::json;

    fn pkg_urls<'a>(v: impl IntoIterator<Item = &'a str>) -> Vec<PinnedAbsolutePackageUrl> {
        v.into_iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn smoke_test_parse_packages_json() {
        let pkg_urls = pkg_urls([
            "fuchsia-pkg://fuchsia.com/ls/0?hash=71bad1a35b87a073f72f582065f6b6efec7b6a4a129868f37f6131f02107f1ea",
            "fuchsia-pkg://fuchsia.com/pkg-resolver/0?hash=26d43a3fc32eaa65e6981791874b6ab80fae31fbfca1ce8c31ab64275fd4e8c0",
        ]);
        let packages = Packages::V1(pkg_urls.clone());
        let packages_json = serde_json::to_vec(&packages).unwrap();
        assert_eq!(parse_packages_json(&packages_json).unwrap(), pkg_urls);
    }

    #[test]
    fn smoke_test_serialize_packages_json() {
        let input = pkg_urls([
            "fuchsia-pkg://fuchsia.com/ls/0?hash=71bad1a35b87a073f72f582065f6b6efec7b6a4a129868f37f6131f02107f1ea",
            "fuchsia-pkg://fuchsia.com/pkg-resolver/0?hash=26d43a3fc32eaa65e6981791874b6ab80fae31fbfca1ce8c31ab64275fd4e8c0",
        ]);
        let output =
            parse_packages_json(serialize_packages_json(input.as_slice()).unwrap().as_slice())
                .unwrap();
        assert_eq!(input, output);
    }

    #[test]
    fn expect_failure_parse_packages_json() {
        assert_matches!(parse_packages_json(&[]), Err(ParsePackageError::JsonError(_)));
    }

    #[fuchsia::test]
    async fn smoke_test_packages_json_version_string() {
        let pkg_list = [
            "fuchsia-pkg://fuchsia.com/ls/0?hash=71bad1a35b87a073f72f582065f6b6efec7b6a4a129868f37f6131f02107f1ea",
            "fuchsia-pkg://fuchsia.com/pkg-resolver/0?hash=26d43a3fc32eaa65e6981791874b6ab80fae31fbfca1ce8c31ab64275fd4e8c0",
        ];
        let packages = json!({
            "version": "1",
            "content": pkg_list,
        })
        .to_string();
        let update_pkg = TestUpdatePackage::new().add_file("packages.json", packages).await;
        assert_eq!(update_pkg.packages().await.unwrap(), pkg_urls(pkg_list));
    }

    #[fuchsia::test]
    async fn expect_failure_json() {
        let update_pkg = TestUpdatePackage::new();
        let packages = "{}";
        let update_pkg = update_pkg.add_file("packages.json", packages).await;
        assert_matches!(update_pkg.packages().await, Err(ParsePackageError::JsonError(_)))
    }

    #[fuchsia::test]
    async fn expect_failure_version_not_supported() {
        let pkg_list = vec![
            "fuchsia-pkg://fuchsia.com/ls/0?hash=71bad1a35b87a073f72f582065f6b6efec7b6a4a129868f37f6131f02107f1ea",
        ];
        let packages = json!({
            "version": "2",
            "content": pkg_list,
        })
        .to_string();
        let update_pkg = TestUpdatePackage::new().add_file("packages.json", packages).await;
        assert_matches!(
            update_pkg.packages().await,
            Err(ParsePackageError::JsonError(e))
                if e.to_string().contains("unknown variant `2`, expected `1`")
        );
    }

    #[fuchsia::test]
    async fn expect_failure_no_files() {
        let update_pkg = TestUpdatePackage::new();
        assert_matches!(update_pkg.packages().await, Err(ParsePackageError::FailedToOpen(_)))
    }

    #[test]
    fn reject_unpinned_urls() {
        assert_matches!(
            parse_packages_json(serde_json::json!({
                "version": "1",
                "content": ["fuchsia-pkg://fuchsia.example/unpinned"]
            })
            .to_string().as_bytes()),
            Err(ParsePackageError::JsonError(e)) if e.to_string().contains("missing hash")
        )
    }
}
