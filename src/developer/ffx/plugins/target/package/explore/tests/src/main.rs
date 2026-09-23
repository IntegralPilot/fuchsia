// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_e2e_emu::IsolatedEmulator;
use log::info;

#[fuchsia::test(logging = true)]
async fn cat_file_from_package_and_subpackages_using_full_resolver() {
    let emu = IsolatedEmulator::start("test-ffx-explore-package-full").await.unwrap();

    info!("resolving [sub]packages and cat'ing file");
    let output = emu
        .ffx_output(&[
            "target",
            "package",
            "explore",
            "fuchsia-pkg://fuchsia.com/verify_ffx_target_package_explore_superpackage",
            "-c",
            "cat /pkg/data/package_file.txt",
        ])
        .await
        .unwrap();
    assert_eq!(output, include_str!("../testdata/package_file.txt"));

    let output = emu
        .ffx_output(&[
            "target",
            "package",
            "explore",
            "fuchsia-pkg://fuchsia.com/verify_ffx_target_package_explore_superpackage",
            "--subpackage",
            "verify_ffx_target_package_explore_subpackage",
            "-c",
            "cat /pkg/data/subpackage_file.txt",
        ])
        .await
        .unwrap();
    assert_eq!(output, include_str!("../testdata/subpackage_file.txt"));

    let output = emu
        .ffx_output(&[
            "target",
            "package",
            "explore",
            "fuchsia-pkg://fuchsia.com/verify_ffx_target_package_explore_superpackage",
            "--subpackage",
            "verify_ffx_target_package_explore_subpackage",
            "--subpackage",
            "verify_ffx_target_package_explore_subsubpackage",
            "-c",
            "cat /pkg/data/subsubpackage_file.txt",
        ])
        .await
        .unwrap();
    assert_eq!(output, include_str!("../testdata/subsubpackage_file.txt"));
}

#[fuchsia::test]
async fn cat_file_from_package_using_base_resolver() {
    let emu = IsolatedEmulator::start("test-ffx-explore-package-base").await.unwrap();

    info!("resolving base package and cat'ing file");
    let output = emu
        .ffx_output(&[
            "target",
            "package",
            "explore",
            "fuchsia-pkg://fuchsia.com/system_image",
            "--fuchsia-pkg-resolver",
            "base",
            "-c",
            "cat /pkg/meta/package",
        ])
        .await
        .unwrap();
    assert_eq!(output, r#"{"name":"system_image","version":"0"}"#);
}
