// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// This module tests the fuchsia.pkg.PackageResolver.GetHash FIDL method
use {
    assert_matches::assert_matches,
    fuchsia_pkg_testing::RepositoryBuilder,
    lib::{EMPTY_REPO_PATH, TestEnvBuilder, make_pkg_with_extra_blobs},
    std::sync::Arc,
    zx::Status,
};

#[fuchsia::test]
async fn succeeds_if_package_present() {
    let env = TestEnvBuilder::new().build().await;
    let pkg_name = "a-fake-pkg-name";
    let pkg = make_pkg_with_extra_blobs(pkg_name, 0).await;
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_url = "fuchsia-pkg://test".parse().unwrap();
    let repo_config = served_repository.make_repo_config(repo_url);
    let () = env.proxies.repo_manager.add(&repo_config.into()).await.unwrap().unwrap();

    let package = env.get_hash("fuchsia-pkg://test/a-fake-pkg-name").await;

    assert_eq!(package.unwrap(), pkg.hash().clone().into());

    env.stop().await;
}

#[fuchsia::test]
async fn fails_if_package_absent() {
    let env = TestEnvBuilder::new().build().await;
    let repo =
        Arc::new(RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH).build().await.unwrap());
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_url = "fuchsia-pkg://test".parse().unwrap();
    let repo_config = served_repository.make_repo_config(repo_url);
    let () = env.proxies.repo_manager.add(&repo_config.into()).await.unwrap().unwrap();

    let package = env.get_hash("fuchsia-pkg://test/b-fake-pkg-name").await;

    assert_matches!(package, Err(status) if status == Status::NOT_FOUND);

    env.stop().await;
}
