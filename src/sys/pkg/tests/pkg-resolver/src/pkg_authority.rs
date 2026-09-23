// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module tests the fuchsia.pkg/Authority FIDL protocol.

use fidl_fuchsia_pkg as fpkg;
use fidl_fuchsia_pkg_rewrite_ext::{Rule, RuleConfig};
use fuchsia_pkg_testing::{PackageBuilder, RepositoryBuilder};
use http_uri_ext::HttpUriExt as _;
use lib::{EMPTY_REPO_PATH, MountsBuilder, TestEnvBuilder};
use std::sync::Arc;

#[fuchsia::test]
async fn lookup() {
    let env = TestEnvBuilder::new()
        .mounts(
            MountsBuilder::new()
                .dynamic_rewrite_rules(RuleConfig::Version1(vec![
                    Rule::new("rewrite-me", "example.org", "/", "/").unwrap(),
                ]))
                .enable_dynamic_config(lib::EnableDynamicConfig {
                    enable_dynamic_configuration: true,
                })
                .build(),
        )
        .build()
        .await;
    let pkg = PackageBuilder::new("test-package").build().await.unwrap();
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_url = "fuchsia-pkg://example.org".parse().unwrap();
    let repo_config = served_repository.make_repo_config(repo_url);
    let () = env.proxies.repo_manager.add(&repo_config.clone().into()).await.unwrap().unwrap();

    let expected_blob_url_dir =
        repo_config.mirrors()[0].blob_mirror_url().clone().extend_dir_with_path("1").unwrap();

    // Lookup without variant.
    let (blob_id, blob_url_dir) =
        env.lookup("fuchsia-pkg://example.org/test-package").await.unwrap();
    assert_eq!(blob_id, (*pkg.hash()).into());
    assert_eq!(blob_url_dir, expected_blob_url_dir);

    // Lookup with variant.
    let (blob_id, blob_url_dir) =
        env.lookup("fuchsia-pkg://example.org/test-package/0").await.unwrap();
    assert_eq!(blob_id, (*pkg.hash()).into());
    assert_eq!(blob_url_dir, expected_blob_url_dir);

    // Lookup matches rewrite rule.
    let (blob_id, blob_url_dir) =
        env.lookup("fuchsia-pkg://rewrite-me/test-package").await.unwrap();
    assert_eq!(blob_id, (*pkg.hash()).into());
    assert_eq!(blob_url_dir, expected_blob_url_dir);

    // Lookup rejects pinned URLs.
    std::assert_matches!(
        env.lookup(&format!("fuchsia-pkg://example.org/test-package?hash={}", pkg.hash())).await,
        Err(fpkg::AuthorityLookupError::PinnedUrlNotAllowed)
    );

    // Lookup missing package.
    std::assert_matches!(
        env.lookup("fuchsia-pkg://example.org/missing-package").await,
        Err(fpkg::AuthorityLookupError::PackageNotFound)
    );

    // Lookup missing repo.
    std::assert_matches!(
        env.lookup("fuchsia-pkg://missing-repo/test-package").await,
        Err(fpkg::AuthorityLookupError::RepositoryNotFound)
    );

    // Invalid URL.
    std::assert_matches!(env.lookup("bad-url").await, Err(fpkg::AuthorityLookupError::InvalidUrl));

    env.stop().await;
}
