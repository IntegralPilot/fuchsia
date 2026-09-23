// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module tests the full component resolver. The full component resolver uses the same fidl
//! serving code as the base component resolver and otherwise forwards to the full package resolver,
//! so we just need to test that things are wired up correctly.

use fidl_fuchsia_component_decl as fcomponent_decl;
use std::sync::Arc;

#[fuchsia::test]
async fn resolve() {
    let manifest = fidl::persist(&fcomponent_decl::Component::default().clone()).unwrap();
    let subsubpackage = fuchsia_pkg_testing::PackageBuilder::new("subsubpackage")
        .add_resource_at("meta/manifest.cm", &*manifest)
        .add_resource_at("subsubpackage-blob", "subsubpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let subpackage = fuchsia_pkg_testing::PackageBuilder::new("subpackage")
        .add_resource_at("meta/manifest.cm", &*manifest)
        .add_subpackage("my-subsubpackage", &subsubpackage)
        .add_resource_at("subpackage-blob", "subpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let superpackage = fuchsia_pkg_testing::PackageBuilder::new("superpackage")
        .add_resource_at("meta/manifest.cm", &*manifest)
        .add_subpackage("my-subpackage", &subpackage)
        .add_resource_at("superpackage-blob", "superpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&superpackage)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&superpackage],
        ))
        .build()
        .await;

    let super_component = env
        .resolve_full_component("fuchsia-pkg://example.org/superpackage#meta/manifest.cm")
        .await
        .unwrap();
    let super_dir = super_component.package.unwrap().directory.unwrap().into_proxy();
    let () = superpackage.verify_contents(&super_dir).await.unwrap();

    let sub_component = env
        .resolve_with_context_full_component(
            "my-subpackage#meta/manifest.cm",
            &super_component.resolution_context.unwrap(),
        )
        .await
        .unwrap();
    let sub_dir = sub_component.package.unwrap().directory.unwrap().into_proxy();
    let () = subpackage.verify_contents(&sub_dir).await.unwrap();

    let subsub_component = env
        .resolve_with_context_full_component(
            "my-subsubpackage#meta/manifest.cm",
            &sub_component.resolution_context.unwrap(),
        )
        .await
        .unwrap();
    let subsub_dir = subsub_component.package.unwrap().directory.unwrap().into_proxy();
    let () = subsubpackage.verify_contents(&subsub_dir).await.unwrap();
}
