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

use fidl_fuchsia_pkg as fpkg;
use fuchsia_async as fasync;
use futures::future::FutureExt as _;
use futures::stream::StreamExt as _;
use rand::{RngReader, SeedableRng as _};
use std::collections::HashMap;
use std::io::Read as _;
use std::sync::Arc;

// Creates a repo that contains `pkg`, resolves `pkg`, and verifies the resolved package directory
// against `pkg`.
async fn verify_resolution(pkg: &fuchsia_pkg_testing::Package) {
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());

    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(&repo_config, &[pkg]))
        .build()
        .await;

    let (package, _context) =
        env.resolve_ota("fuchsia-pkg://example.org/test-package").await.unwrap();

    let () = pkg.verify_contents(&package).await.unwrap();
}

#[fuchsia::test]
async fn metafar_only() {
    let pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package").build().await.unwrap();
    let () = verify_resolution(&pkg).await;
}

#[fuchsia::test]
async fn empty_blob() {
    let pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("empty-blob", "".as_bytes())
        .build()
        .await
        .unwrap();
    let () = verify_resolution(&pkg).await;
}

#[fuchsia::test]
async fn duplicate_blob() {
    let pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("small-blob", "blob-contents".as_bytes())
        .add_resource_at("duplicate-blob", "blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let () = verify_resolution(&pkg).await;
}

#[fuchsia::test]
async fn large_blob() {
    let pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at(
            "large-blob",
            RngReader(rand::rngs::StdRng::from_seed([0u8; 32])).take(1024 * 1024),
        )
        .build()
        .await
        .unwrap();
    let () = verify_resolution(&pkg).await;
}

#[fuchsia::test]
async fn many_blobs() {
    let mut pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package");
    let mut rng = rand::rngs::StdRng::from_seed([0u8; 32]);
    for i in 0..200 {
        pkg = pkg.add_resource_at(format!("blob-{i}"), RngReader(&mut rng).take(10));
    }
    let pkg = pkg.build().await.unwrap();
    let () = verify_resolution(&pkg).await;
}

#[fuchsia::test]
async fn merkle_pin_overrides_authority() {
    let pkg = fuchsia_pkg_testing::PackageBuilder::new("pin-me").build().await.unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());

    // The PkgAuthority will return the hash of zeroes, make sure this is not the package's actual
    // hash so that a successful resolve means that the authority's hash was overridden by the pin.
    assert_ne!(*pkg.hash(), [0; 32].into());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::new(HashMap::from([
            (
                "fuchsia-pkg://example.org/pin-me".to_owned(),
                Ok(([0; 32].into(), format!("{}/1", repo_config.mirrors()[0].blob_mirror_url()))),
            ),
            (
                "fuchsia-pkg://example.org/missing-package".to_owned(),
                Err(fpkg::AuthorityLookupError::PackageNotFound),
            ),
        ])))
        .build()
        .await;

    let (package, _context) = env
        .resolve_ota(&format!("fuchsia-pkg://example.org/pin-me?hash={}", pkg.hash()))
        .await
        .unwrap();

    let () = pkg.verify_contents(&package).await.unwrap();

    // Even if pinned, the package should still be known to the authority to be resolved.
    std::assert_matches!(
        env.resolve_ota(&format!("fuchsia-pkg://example.org/missing-package?hash={}", pkg.hash()))
            .await,
        Err(fpkg::ResolveError::PackageNotFound)
    );
}

#[fuchsia::test(logging_tags = ["RESOLVE_TEST"])]
async fn error_codes() {
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::new(HashMap::from([
            (
                "fuchsia-pkg://example.org/missing-repo".to_owned(),
                Err(fpkg::AuthorityLookupError::RepositoryNotFound),
            ),
            (
                "fuchsia-pkg://example.org/missing-package".to_owned(),
                Err(fpkg::AuthorityLookupError::PackageNotFound),
            ),
        ])))
        .build()
        .await;

    // Invalid URL
    std::assert_matches!(
        env.resolve_ota("fuchsia-pkg://test/bad-url!").await,
        Err(fidl_fuchsia_pkg::ResolveError::InvalidUrl)
    );

    // Nonexistent repo
    std::assert_matches!(
        env.resolve_ota("fuchsia-pkg://example.org/missing-repo").await,
        Err(fidl_fuchsia_pkg::ResolveError::RepoNotFound)
    );

    // Nonexistent package
    std::assert_matches!(
        env.resolve_ota("fuchsia-pkg://example.org/missing-package").await,
        Err(fidl_fuchsia_pkg::ResolveError::PackageNotFound)
    );
}

#[fuchsia::test]
async fn retry_blob_fetch_network_error() {
    let pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package").build().await.unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo)
        .server()
        .response_overrider(fuchsia_pkg_testing::serve::responder::ForPathPrefix::new(
            "/blobs",
            fuchsia_pkg_testing::serve::responder::OncePerPath::new(
                fuchsia_pkg_testing::serve::responder::StaticResponseCode::server_error(),
            ),
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

    let (package, _context) =
        env.resolve_ota("fuchsia-pkg://example.org/test-package").await.unwrap();

    let () = pkg.verify_contents(&package).await.unwrap();
    let path = format!("/blobs/1/{}", pkg.hash());
    assert_eq!(
        *served_repository.history().lock(),
        vec![
            fuchsia_pkg_testing::serve::HistoryEntry {
                method: http::Method::GET,
                path: path.clone(),
                status: http::StatusCode::INTERNAL_SERVER_ERROR,
            },
            fuchsia_pkg_testing::serve::HistoryEntry {
                method: http::Method::GET,
                path: path.clone(),
                status: http::StatusCode::OK,
            }
        ]
    );
}

#[fuchsia::test]
async fn retry_blob_fetch_network_rate_limit() {
    let pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package").build().await.unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo)
        .server()
        .response_overrider(fuchsia_pkg_testing::serve::responder::ForPathPrefix::new(
            "/blobs",
            fuchsia_pkg_testing::serve::responder::OncePerPath::new(
                fuchsia_pkg_testing::serve::responder::StaticResponseCode::too_many_requests(),
            ),
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

    let (package, _context) =
        env.resolve_ota("fuchsia-pkg://example.org/test-package").await.unwrap();

    let () = pkg.verify_contents(&package).await.unwrap();
    let path = format!("/blobs/1/{}", pkg.hash());
    assert_eq!(
        *served_repository.history().lock(),
        vec![
            fuchsia_pkg_testing::serve::HistoryEntry {
                method: http::Method::GET,
                path: path.clone(),
                status: http::StatusCode::TOO_MANY_REQUESTS,
            },
            fuchsia_pkg_testing::serve::HistoryEntry {
                method: http::Method::GET,
                path: path.clone(),
                status: http::StatusCode::OK,
            }
        ]
    );
}

#[fuchsia::test]
async fn test_concurrent_blob_writes() {
    let duplicate_contents = "duplicate-contents".as_bytes();
    let pkg1 = fuchsia_pkg_testing::PackageBuilder::new("package1")
        .add_resource_at("duplicate-blob-1", duplicate_contents)
        .build()
        .await
        .unwrap();
    let pkg2 = fuchsia_pkg_testing::PackageBuilder::new("package2")
        .add_resource_at("duplicate-blob-2", duplicate_contents)
        .add_resource_at("unique-blob", "unique-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let duplicate_blob_merkle =
        pkg1.meta_contents().unwrap().contents()["duplicate-blob-1"].to_string();
    let unique_blob_merkle = pkg2.meta_contents().unwrap().contents()["unique-blob"];

    // A responder to block the download of the duplicate blob.
    let (blocking_responder, unblocking_closure_receiver) =
        fuchsia_pkg_testing::serve::responder::BlockResponseBodyOnce::new();
    let blocking_responder = fuchsia_pkg_testing::serve::responder::ForPath::new(
        format!("/blobs/1/{}", duplicate_blob_merkle),
        blocking_responder,
    );

    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&pkg1)
            .add_package(&pkg2)
            .build()
            .await
            .unwrap(),
    );
    let served_repository =
        Arc::clone(&repo).server().response_overrider(blocking_responder).start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&pkg1, &pkg2],
        ))
        .build()
        .await;

    // pkg-cache OTA resolver uses try_for_each_concurrent to handle its FIDL request stream, so the
    // concurrent requests can be made on the same channel.
    let (pkg1_dir, server_end) = fidl::endpoints::create_proxy();
    let pkg1_fut =
        env.proxies.ota_package_resolver.resolve("fuchsia-pkg://example.org/package1", server_end);

    // Wait for the GET request for the duplicate blob to be received by the blob server.
    let send_shared_blob_body = unblocking_closure_receiver.await.unwrap();

    // With the duplicate blob fetch active in the blob fetch queue, start the second resolve.
    let (pkg2_dir, server_end) = fidl::endpoints::create_proxy();
    let pkg2_fut =
        env.proxies.ota_package_resolver.resolve("fuchsia-pkg://example.org/package2", server_end);

    // Wait for the unique blob to exist in blobfs. This approximates waiting for the second resolve
    // to request the duplicate blob from the fetch queue.
    let blobfs_reader = env.blobfs.blob_reader_proxy().unwrap();
    while blobfs_reader
        .get_vmo(unique_blob_merkle.as_bytes().try_into().unwrap())
        .await
        .expect("Getting vmo")
        .is_err()
    {
        fasync::Timer::new(std::time::Duration::from_millis(10)).await;
    }

    // At this point, both package resolves should be blocked on the shared blob download. Unblock
    // the server and verify both packages resolve to valid directories.
    let () = send_shared_blob_body();
    let ((), ()) = futures::join!(
        async move {
            let _: fpkg::ResolutionContext = pkg1_fut.await.unwrap().unwrap();
        },
        async move {
            let _: fpkg::ResolutionContext = pkg2_fut.await.unwrap().unwrap();
        },
    );
    let () = pkg1.verify_contents(&pkg1_dir).await.unwrap();
    let () = pkg2.verify_contents(&pkg2_dir).await.unwrap();

    // The duplicate blob should be the second blob requested (the responder blocks the response
    // body, not the response header, and so will not shift the request down in the history) and it
    // should only have been requested once.
    assert_eq!(
        *served_repository.history().lock(),
        vec![
            fuchsia_pkg_testing::serve::HistoryEntry {
                method: http::Method::GET,
                path: format!("/blobs/1/{}", pkg1.hash()),
                status: http::StatusCode::OK,
            },
            fuchsia_pkg_testing::serve::HistoryEntry {
                method: http::Method::GET,
                path: format!("/blobs/1/{}", duplicate_blob_merkle),
                status: http::StatusCode::OK,
            },
            fuchsia_pkg_testing::serve::HistoryEntry {
                method: http::Method::GET,
                path: format!("/blobs/1/{}", pkg2.hash()),
                status: http::StatusCode::OK,
            },
            fuchsia_pkg_testing::serve::HistoryEntry {
                method: http::Method::GET,
                path: format!("/blobs/1/{}", unique_blob_merkle),
                status: http::StatusCode::OK,
            },
        ]
    );
}

// TODO(https://fxbug.dev/308158482): re-enable when ring works on riscv64
#[cfg(not(target_arch = "riscv64"))]
async fn test_https_endpoint(bind_addr: impl Into<std::net::IpAddr>) {
    let pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package")
        .add_resource_at("blob", "blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo)
        .server()
        .use_https_domain(fuchsia_pkg_testing::serve::Domain::TestFuchsiaCom)
        .bind_to_addr(bind_addr)
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

    let (package, _context) =
        env.resolve_ota("fuchsia-pkg://example.org/test-package").await.unwrap();
    let () = pkg.verify_contents(&package).await.unwrap();
}

// TODO(https://fxbug.dev/308158482): re-enable when ring works on riscv64
#[cfg(not(target_arch = "riscv64"))]
#[fuchsia::test]
async fn https_endpoint_ipv6_only() {
    test_https_endpoint(std::net::Ipv6Addr::LOCALHOST).await
}

// TODO(https://fxbug.dev/308158482): re-enable when ring works on riscv64
#[cfg(not(target_arch = "riscv64"))]
#[fuchsia::test]
async fn https_endpoint_ipv4_only() {
    test_https_endpoint(std::net::Ipv4Addr::LOCALHOST).await
}

// Tests subpackage recursion, including:
//   1. if a recursed subpackage meta.far was already fetched as a content blob.
//   2. duplicate subpackages
#[fuchsia::test]
async fn subsubpackage() {
    let subsubpackage = fuchsia_pkg_testing::PackageBuilder::new("subsubpackage")
        .add_resource_at("subsubpackage-blob", "subsubpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let subpackage = fuchsia_pkg_testing::PackageBuilder::new("subpackage")
        .add_subpackage("my-subsubpackage", &subsubpackage)
        .add_subpackage("duplicate-subpackage", &subsubpackage)
        .add_resource_at("subpackage-blob", "subpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let superpackage = fuchsia_pkg_testing::PackageBuilder::new("superpackage")
        .add_subpackage("my-subpackage", &subpackage)
        .add_resource_at("superpackage-blob", "superpackage-blob-contents".as_bytes())
        .add_resource_at("subsubpackage-meta-far", subsubpackage.contents().0.contents.as_slice())
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

    let (super_dir, context) =
        env.resolve_ota("fuchsia-pkg://example.org/superpackage").await.unwrap();
    let () = superpackage.verify_contents(&super_dir).await.unwrap();

    let (sub_dir, context) = env.resolve_with_context_ota("my-subpackage", &context).await.unwrap();
    let () = subpackage.verify_contents(&sub_dir).await.unwrap();

    let (subsub_dir, _context) =
        env.resolve_with_context_ota("my-subsubpackage", &context).await.unwrap();
    let () = subsubpackage.verify_contents(&subsub_dir).await.unwrap();

    let (subsub_dir, _context) =
        env.resolve_with_context_ota("duplicate-subpackage", &context).await.unwrap();
    let () = subsubpackage.verify_contents(&subsub_dir).await.unwrap();
}

#[fuchsia::test]
async fn resolve_of_already_cached_package_is_not_blocked_by_in_progress_blob_fetches() {
    let blocking_pkg =
        fuchsia_pkg_testing::PackageBuilder::new("blocking-package").build().await.unwrap();
    let cached_subpackage = fuchsia_pkg_testing::PackageBuilder::new("cached-subpackage")
        .add_resource_at("subpackage-blob", "subpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let cached_superpackage = fuchsia_pkg_testing::PackageBuilder::new("cached-superpackage")
        .add_subpackage("my-subpackage", &cached_subpackage)
        .add_resource_at("superpackage-blob", "superpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&blocking_pkg)
            .add_package(&cached_superpackage)
            .build()
            .await
            .unwrap(),
    );
    let (blocker, mut blocked_fetches) =
        fuchsia_pkg_testing::serve::responder::BlockResponseHeaders::new();
    let responder = fuchsia_pkg_testing::serve::responder::ForPathPrefix::new("/blobs/1/", blocker);
    let served_repository =
        Arc::clone(&repo).server().response_overrider(responder).start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&blocking_pkg, &cached_superpackage],
        ))
        .blob_fetch_concurrency_limit(1)
        .build()
        .await;

    let () = cached_superpackage.write_to_blobfs(&env.blobfs).await;

    // Wait for the meta.far to be blocked.
    let (blocking_pkg_dir, server_end) = fidl::endpoints::create_proxy();
    let mut blocking_pkg_fut = std::pin::pin!(
        env.proxies
            .ota_package_resolver
            .resolve("fuchsia-pkg://example.org/blocking-package", server_end)
            .fuse()
    );
    let blocked_meta_far = blocked_fetches.next().await.unwrap();

    // The already cached package should resolve even with a full blob fetch queue, and the blocking
    // resolve should not complete.
    futures::select_biased! {
        res  = blocking_pkg_fut => panic!("blocking resolve should not complete {res:?}"),
        already_cached_resolve = env
            .resolve_ota("fuchsia-pkg://example.org/cached-superpackage").fuse() => {
            let (dir, _context) = already_cached_resolve.unwrap();
            let () = cached_superpackage.verify_contents(&dir).await.unwrap();
        }
    }

    // Finish the blocked resolve to make sure it completes successfully and didn't get canceled by
    // timeout.
    let () = blocked_meta_far.unblock();
    let _: fpkg::ResolutionContext = blocking_pkg_fut.await.unwrap().unwrap();
    let () = blocking_pkg.verify_contents(&blocking_pkg_dir).await.unwrap();
}

#[fuchsia::test]
// TODO(https://fxbug.dev/519687989): Enable when Fuchsia system releases are managed in the repo.
#[ignore]
async fn mismatched_pinned_merkle_resolution_fails() {
    let pkg1 = fuchsia_pkg_testing::PackageBuilder::new("pkg1").build().await.unwrap();
    let pkg2 = fuchsia_pkg_testing::PackageBuilder::new("pkg2").build().await.unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&pkg1)
            .add_package(&pkg2)
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
            &[&pkg1],
        ))
        .build()
        .await;

    // Resolve "pkg1" but with the hash of "pkg2"
    assert_ne!(pkg1.hash(), pkg2.hash());
    let pkg1_url_with_pkg2_merkle = format!("fuchsia-pkg://example.org/pkg1?hash={}", pkg2.hash());

    std::assert_matches!(
        env.resolve_ota(&pkg1_url_with_pkg2_merkle).await,
        Err(fpkg::ResolveError::PackageNotFound)
    );
}

#[fuchsia::test]
async fn download_blob_header_timeout() {
    let pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package").build().await.unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo)
        .server()
        .response_overrider(fuchsia_pkg_testing::serve::responder::Hang)
        .start()
        .unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&pkg],
        ))
        .blob_network_header_timeout_seconds(0)
        .build()
        .await;

    std::assert_matches!(
        env.resolve_ota("fuchsia-pkg://example.org/test-package").await,
        Err(fpkg::ResolveError::UnavailableBlob)
    );
}

#[fuchsia::test]
async fn download_blob_body_timeout() {
    let pkg = fuchsia_pkg_testing::PackageBuilder::new("test-package").build().await.unwrap();
    let repo = Arc::new(
        fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo)
        .server()
        .response_overrider(fuchsia_pkg_testing::serve::responder::HangBody)
        .start()
        .unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let env = crate::TestEnv::builder()
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&pkg],
        ))
        .blob_network_body_timeout_seconds(0)
        .build()
        .await;

    std::assert_matches!(
        env.resolve_ota("fuchsia-pkg://example.org/test-package").await,
        Err(fpkg::ResolveError::UnavailableBlob)
    );
}

#[test_case::test_case(blobfs_ramdisk::Implementation::CppBlobfs; "blobfs")]
#[test_case::test_case(blobfs_ramdisk::Implementation::Fxblob; "fxblob")]
#[fuchsia::test]
async fn does_not_fetch_up_to_date_blobs(blobfs_impl: blobfs_ramdisk::Implementation) {
    let subpackage = fuchsia_pkg_testing::PackageBuilder::new("subpackage")
        .add_resource_at("subpackage-blob", "subpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    let superpackage = fuchsia_pkg_testing::PackageBuilder::new("superpackage")
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
        .blobfs_impl(blobfs_impl)
        .build()
        .await;

    // First resolve should download all 4 blobs.
    let (_dir, _context) = env.resolve_ota("fuchsia-pkg://example.org/superpackage").await.unwrap();
    assert_eq!(std::mem::take(&mut *served_repository.history().lock()).len(), 4);

    // Second resolve should not download any blobs.
    let (dir, _context) = env.resolve_ota("fuchsia-pkg://example.org/superpackage").await.unwrap();
    assert_eq!(std::mem::take(&mut *served_repository.history().lock()).len(), 0);
    let () = superpackage.verify_contents(&dir).await.unwrap();

    // Delete some non-root blobs, only they should be downloaded.
    let blobfs_client = env.blobfs.client();
    let () = blobfs_client.delete_blob(subpackage.hash()).await.unwrap();
    let () = blobfs_client
        .delete_blob(&superpackage.content_blob_files().next().unwrap().merkle)
        .await
        .unwrap();
    let (dir, _context) = env.resolve_ota("fuchsia-pkg://example.org/superpackage").await.unwrap();
    assert_eq!(served_repository.history().lock().len(), 2);
    let () = superpackage.verify_contents(&dir).await.unwrap();
}
