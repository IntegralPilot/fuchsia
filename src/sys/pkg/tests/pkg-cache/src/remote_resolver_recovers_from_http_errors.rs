// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module tests shared properties of the package resolution capabilities that use an external
//! package authority and fetch remote blobs (capabilities that use the queued_tuf_resolver, i.e.
//! the -full and -ota resolvers).
//!
//! Uses the -ota resolver capability because it just uses the queued_tuf_resolver with retained
//! GC protection, unlike the -full resolver which has additional logic for base package short
//! circuiting and cache fallback, etc.

use fuchsia_pkg_testing::serve::{HttpResponder, responder};
use fuchsia_pkg_testing::{Package, PackageBuilder};
use std::sync::Arc;

async fn make_pkg_with_extra_blobs(s: &str, n: u32) -> fuchsia_pkg_testing::Package {
    let mut pkg = fuchsia_pkg_testing::PackageBuilder::new(s)
        .add_resource_at(format!("bin/{s}"), &test_package_bin(s)[..])
        .add_resource_at(format!("meta/{s}.cml"), &test_package_cml(s)[..]);
    for i in 0..n {
        pkg = pkg.add_resource_at(format!("data/{s}-{i}"), extra_blob_contents(s, i).as_slice());
    }
    pkg.build().await.unwrap()
}

fn test_package_bin(s: &str) -> Vec<u8> {
    format!("!/boot/bin/sh\n{s}").as_bytes().to_owned()
}

fn test_package_cml(s: &str) -> Vec<u8> {
    format!("{{program:{{runner:\"elf\",binary:\"bin/{s}\"}}}}").as_bytes().to_owned()
}

fn extra_blob_contents(s: &str, i: u32) -> Vec<u8> {
    format!("contents of file {s}-{i}").as_bytes().to_owned()
}

async fn verify_resolve_fails_then_succeeds<H: HttpResponder>(
    pkg: Package,
    responder: H,
    failure_error: fidl_fuchsia_pkg::ResolveError,
) {
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let should_fail = responder::AtomicToggle::new(true);
    let served_repository = Arc::clone(&repo)
        .server()
        .response_overrider(responder::Toggleable::new(&should_fail, responder))
        .response_overrider(responder::Filter::new(
            responder::is_range_request,
            responder::StaticResponseCode::server_error(),
        ))
        .start()
        .unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&pkg],
        ))
        .build()
        .await;

    // First resolve fails with the expected error.
    let pkg_url = format!("fuchsia-pkg://example.org/{}", pkg.name());
    std::assert_matches!(env.resolve_ota(&pkg_url).await, Err(error) if error == failure_error);

    // Disabling the custom responder allows the subsequent resolves to succeed.
    should_fail.unset();
    let (package_dir, _resolved_context) = env.resolve_ota(&pkg_url).await.unwrap();
    let () = pkg.verify_contents(&package_dir).await.unwrap();
}

#[fuchsia::test]
async fn second_resolve_succeeds_when_far_404() {
    let pkg = make_pkg_with_extra_blobs("second_resolve_succeeds_when_far_404", 1).await;
    let path_to_override = format!("/blobs/1/{}", pkg.hash());

    verify_resolve_fails_then_succeeds(
        pkg,
        responder::ForPath::new(path_to_override, responder::StaticResponseCode::not_found()),
        fidl_fuchsia_pkg::ResolveError::UnavailableBlob,
    )
    .await
}

#[fuchsia::test]
async fn second_resolve_succeeds_when_blob_404() {
    let pkg = make_pkg_with_extra_blobs("second_resolve_succeeds_when_blob_404", 1).await;
    let path_to_override = format!(
        "/blobs/1/{}",
        fuchsia_merkle::root_from_slice(extra_blob_contents(
            "second_resolve_succeeds_when_blob_404",
            0
        ))
    );

    verify_resolve_fails_then_succeeds(
        pkg,
        responder::ForPath::new(path_to_override, responder::StaticResponseCode::not_found()),
        fidl_fuchsia_pkg::ResolveError::UnavailableBlob,
    )
    .await
}

#[fuchsia::test]
async fn second_resolve_succeeds_when_far_errors_mid_download() {
    let pkg = PackageBuilder::new("second_resolve_succeeds_when_far_errors_mid_download")
        .add_resource_at(
            "meta/large_file",
            vec![0; crate::FILE_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING].as_slice(),
        )
        .build()
        .await
        .unwrap();
    let path_to_override = format!("/blobs/1/{}", pkg.hash());

    verify_resolve_fails_then_succeeds(
        pkg,
        responder::ForPath::new(path_to_override, responder::OneByteShortThenError),
        fidl_fuchsia_pkg::ResolveError::UnavailableBlob,
    )
    .await
}

#[fuchsia::test]
async fn second_resolve_succeeds_when_blob_errors_mid_download() {
    let blob = vec![0; crate::FILE_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING];
    let pkg = PackageBuilder::new("second_resolve_succeeds_when_blob_errors_mid_download")
        .add_resource_at("blobbity/blob", blob.as_slice())
        .build()
        .await
        .unwrap();
    let path_to_override = format!("/blobs/1/{}", fuchsia_merkle::root_from_slice(&blob));

    verify_resolve_fails_then_succeeds(
        pkg,
        responder::ForPath::new(path_to_override, responder::OneByteShortThenError),
        fidl_fuchsia_pkg::ResolveError::UnavailableBlob,
    )
    .await
}

#[fuchsia::test]
async fn second_resolve_succeeds_disconnect_before_far_complete() {
    let pkg = PackageBuilder::new("second_resolve_succeeds_disconnect_before_far_complete")
        .add_resource_at(
            "meta/large_file",
            vec![0; crate::FILE_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING].as_slice(),
        )
        .build()
        .await
        .unwrap();
    let path_to_override = format!("/blobs/1/{}", pkg.hash());

    verify_resolve_fails_then_succeeds(
        pkg,
        responder::ForPath::new(path_to_override, responder::OneByteShortThenDisconnect),
        fidl_fuchsia_pkg::ResolveError::UnavailableBlob,
    )
    .await
}

#[fuchsia::test]
async fn second_resolve_succeeds_disconnect_before_blob_complete() {
    let blob = vec![0; crate::FILE_SIZE_LARGE_ENOUGH_TO_TRIGGER_HYPER_BATCHING];
    let pkg = PackageBuilder::new("second_resolve_succeeds_disconnect_before_blob_complete")
        .add_resource_at("blobbity/blob", blob.as_slice())
        .build()
        .await
        .unwrap();
    let path_to_override = format!("/blobs/1/{}", fuchsia_merkle::root_from_slice(&blob));

    verify_resolve_fails_then_succeeds(
        pkg,
        responder::ForPath::new(path_to_override, responder::OneByteShortThenDisconnect),
        fidl_fuchsia_pkg::ResolveError::UnavailableBlob,
    )
    .await
}

#[fuchsia::test]
async fn second_resolve_succeeds_when_far_corrupted() {
    let pkg = make_pkg_with_extra_blobs("second_resolve_succeeds_when_far_corrupted", 1).await;
    let path_to_override = format!("/blobs/1/{}", pkg.hash());

    verify_resolve_fails_then_succeeds(
        pkg,
        responder::ForPath::new(path_to_override, responder::OneByteFlipped),
        fidl_fuchsia_pkg::ResolveError::Io,
    )
    .await
}

#[fuchsia::test]
async fn second_resolve_succeeds_when_blob_corrupted() {
    let pkg = make_pkg_with_extra_blobs("second_resolve_succeeds_when_blob_corrupted", 1).await;
    let blob = extra_blob_contents("second_resolve_succeeds_when_blob_corrupted", 0);
    let path_to_override = format!("/blobs/1/{}", fuchsia_merkle::root_from_slice(&blob));

    verify_resolve_fails_then_succeeds(
        pkg,
        responder::ForPath::new(path_to_override, responder::OneByteFlipped),
        fidl_fuchsia_pkg::ResolveError::Io,
    )
    .await
}

// TODO(b/308158482): re-enable when ring works on riscv64
#[cfg(not(target_arch = "riscv64"))]
use {fuchsia_pkg_testing::serve::Domain, std::net::Ipv4Addr};

// The hyper clients that download blobs (currently created by the http-client component) sometimes
// end up waiting on operations on their TCP connections that will never return (e.g. because of an
// upstream network partition). To detect this, the hyper client response futures are wrapped with
// timeout futures. To recover from this, the future is dropped when the timeouts are hit. This
// recovery plan requires that dropping the hyper response future causes hyper to close the
// underlying TCP connection and create a new one the next time hyper is asked to perform a network
// operation. This assumption holds for http1, but not for http2.
//
// This test verifies the "dropping a hyper response future prevents the underlying TCP connection
// from being reused" requirement. It does so by verifying that if a resolve fails due to a blob
// download timeout and the resolve is retried, the retry will cause a new TCP connection to be made
// to the blob mirror.
//
// This test uses https because the test exists to catch changes to the Fuchsia hyper client that
// would cause the hyper client to use http2 before the Fuchsia hyper client is able to recover from
// bad TCP connections when using http2. The http-client component does not explicitly enable http2
// on its hyper clients, so the way this change would sneak in is if the hyper client is changed to
// use ALPN to prefer http2. The blob server used in this test has ALPN configured to prefer http2.
// TODO(b/308158482): re-enable when ring works on riscv64.
#[cfg(not(target_arch = "riscv64"))]
#[fuchsia::test]
async fn blob_timeout_causes_new_tcp_connection() {
    // Test with a package that has just 1 blob, the meta.far, so we can assert the exact number of
    // connections made (a package with multiple blobs could have a different number of connections
    // depending on whether the successful downloads reuse connections).
    let pkg_1_blob = fuchsia_pkg_testing::PackageBuilder::new("pkg-1").build().await.unwrap();
    // Test with a package that has 11 blobs (including the meta.far), so we can catch if the hyper
    // client has a TCP connection pool of size 10.
    let mut pkg_11_blobs = fuchsia_pkg_testing::PackageBuilder::new("pkg-11");
    for i in 0..10 {
        pkg_11_blobs = pkg_11_blobs
            .add_resource_at(format!("blob-{i}"), format!("blob-{i}-contents").as_bytes());
    }
    let pkg_11_blobs = pkg_11_blobs.build().await.unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&pkg_1_blob)
            .add_package(&pkg_11_blobs)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo)
        .server()
        .response_overrider(responder::OncePerPath::new(responder::HangBody))
        .use_https_domain(Domain::TestFuchsiaCom)
        .bind_to_addr(Ipv4Addr::LOCALHOST)
        .start()
        .unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&pkg_1_blob, &pkg_11_blobs],
        ))
        .blob_network_body_timeout_seconds(0)
        .blob_fetch_concurrency_limit(11)
        .build()
        .await;

    assert_eq!(served_repository.connection_attempts(), 0);
    // The resolve request may not succeed despite the retry because the 0 timeout on the blob body
    // future can fire prior to the body being downloaded on the retry. However, we expect to
    // observe 2 connections, one for the initial fetch that timed out and one for the retry.
    match env.resolve_ota("fuchsia-pkg://example.org/pkg-1").await {
        Ok(_) | Err(fidl_fuchsia_pkg::ResolveError::UnavailableBlob) => {}
        Err(e) => {
            panic!("unexpected error: {e:?}");
        }
    };
    assert_eq!(served_repository.connection_attempts(), 2);

    match env.resolve_ota("fuchsia-pkg://example.org/pkg-11").await {
        Ok(_) => {
            // There should be more than 12 connection attempts:
            //   1. 2 from the first test
            //   2. (11-1): 11 hung requests from this resolve, except the second connection from
            //      the first test can be reused
            //   3. at least one more to successfully resolve all the blobs (depending on how the
            //      concurrent blob fetches of the content blobs are scheduled it could be from 1 to
            //      10
            std::assert_matches!(served_repository.connection_attempts(), x if x > 12, "Ok arm");
        }
        Err(fidl_fuchsia_pkg::ResolveError::UnavailableBlob) => {
            // The earliest the resolve could fail is on the meta.far, so at a minimum there should
            // be more than 2 connection attempts:
            //   1. 2 from the first test
            //   2. (2-1): 2 again like the first test, except the second connection from the first
            //      can be reused
            std::assert_matches!(served_repository.connection_attempts(), x if x > 2, "Err arm");
        }
        Err(e) => {
            panic!("unexpected error: {e:?}");
        }
    };
}
