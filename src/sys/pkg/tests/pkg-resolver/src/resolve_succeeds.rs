// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// This module tests the property that pkg_resolver successfully
/// services fuchsia.pkg.PackageResolver.Resolve FIDL requests for
/// different types of packages when blobfs is in various intermediate states.
use {
    assert_matches::assert_matches,
    diagnostics_assertions::{assert_data_tree, tree_assertion},
    fidl_fuchsia_io as fio, fidl_fuchsia_pkg_ext as pkg, fuchsia_async as fasync,
    fuchsia_pkg_testing::{Package, PackageBuilder, RepositoryBuilder, serve::responder},
    futures::{join, prelude::*},
    http_uri_ext::HttpUriExt as _,
    lib::{
        EMPTY_REPO_PATH, TestEnv, TestEnvBuilder, extra_blob_contents, make_pkg_with_extra_blobs,
        resolve_package, test_package_bin, test_package_cml,
    },
    rand::RngReader,
    rand::prelude::*,
    std::{
        io::{self, Read},
        sync::Arc,
        time::Duration,
    },
};

// TODO(b/308158482): re-enable when ring works on riscv64
#[cfg(not(target_arch = "riscv64"))]
use {
    fuchsia_pkg_testing::serve::Domain,
    std::net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn package_resolution() {
    let env = TestEnvBuilder::new().build().await;
    let mut startup_blobs = env.blobfs.list_blobs().unwrap();

    let s = "package_resolution";
    let pkg = PackageBuilder::new(s)
        .add_resource_at(format!("bin/{}", s), &test_package_bin(s)[..])
        .add_resource_at(format!("meta/{}.cml", s), &test_package_cml(s)[..])
        .add_resource_at("data/duplicate_a", "same contents".as_bytes())
        .add_resource_at("data/duplicate_b", "same contents".as_bytes())
        .build()
        .await
        .unwrap();
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

    let (package, resolved_context) = env
        .resolve_package(format!("fuchsia-pkg://test/{}", s).as_str())
        .await
        .expect("package to resolve without error");

    // Verify the served package directory contains the exact expected contents.
    pkg.verify_contents(&package).await.unwrap();

    // Make sure repo also has the blobs that were in blobfs on startup.
    let mut repo_blobs = repo.list_blobs().unwrap();
    repo_blobs.append(&mut startup_blobs);

    // All blobs in the repository should now be present in blobfs.
    assert_eq!(env.blobfs.list_blobs().unwrap(), repo_blobs);

    assert_eq!(resolved_context.blob_id().unwrap(), &pkg::BlobId::from(*pkg.hash()));

    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority with blob mirror URL.
#[fuchsia::test]
async fn separate_blobs_url() {
    let env = TestEnvBuilder::new().build().await;

    let mut startup_blobs = env.blobfs.list_blobs().unwrap();

    let pkg_name = "separate_blobs_url";
    let pkg = make_pkg_with_extra_blobs(pkg_name, 3).await;
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();

    // Rename the blobs directory so the blobs can't be found in the usual place.
    // The package resolver currently requires Content-Length headers when downloading content
    // blobs. "pm serve" will gzip compress paths that aren't prefixed with "/blobs", which removes
    // the Content-Length header. To ensure "pm serve" does not compress the blobs stored at this
    // alternate path, its name must start with "blobs".
    let repo_root = repo.path();
    std::fs::rename(repo_root.join("blobs"), repo_root.join("blobsbolb")).unwrap();

    // Configure the repo manager with different TUF and blobs URLs.
    let repo_url = "fuchsia-pkg://test".parse().unwrap();
    let mut repo_config = served_repository.make_repo_config(repo_url);
    let mirror = &repo_config.mirrors()[0];
    let mirror = pkg::MirrorConfigBuilder::new(mirror.mirror_url().to_owned())
        .unwrap()
        .subscribe(mirror.subscribe())
        .blob_mirror_url(mirror.mirror_url().to_owned().extend_dir_with_path("blobsbolb").unwrap())
        .unwrap()
        .build();
    repo_config.insert_mirror(mirror).unwrap();
    let () = env.proxies.repo_manager.add(&repo_config.into()).await.unwrap().unwrap();

    // Verify package installation from the split repo succeeds.
    let (package, _resolved_context) = env
        .resolve_package(format!("fuchsia-pkg://test/{}", pkg_name).as_str())
        .await
        .expect("package to resolve without error");
    pkg.verify_contents(&package).await.unwrap();
    std::fs::rename(repo_root.join("blobsbolb"), repo_root.join("blobs")).unwrap();

    let mut repo_blobs = repo.list_blobs().unwrap();
    repo_blobs.append(&mut startup_blobs);

    assert_eq!(env.blobfs.list_blobs().unwrap(), repo_blobs);

    env.stop().await;
}

// `alter_env` is called immediately before resolving `pkg`.
// The backing blobfs is empty before the resolve.
async fn verify_resolve_with_altered_env(
    pkg: Package,
    alter_env: impl AsyncFnOnce(&TestEnv, &Package),
) -> () {
    let env = TestEnvBuilder::new()
        .blobfs_and_system_image_hash(blobfs_ramdisk::BlobfsRamdisk::start().await.unwrap(), None)
        .build()
        .await;

    let mut startup_blobs = env.blobfs.list_blobs().unwrap();

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

    // Verify nothing in the setup added any blobs to blobfs.
    assert_eq!(
        env.blobfs
            .root_dir()
            .unwrap()
            .list_dir(".")
            .unwrap()
            .map(|e| e.unwrap().file_name().to_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        Vec::<String>::new()
    );

    alter_env(&env, &pkg).await;

    let pkg_url = format!("fuchsia-pkg://test/{}", pkg.name());
    let (package_dir, _resolved_context) = env.resolve_package(&pkg_url).await.unwrap();

    pkg.verify_contents(&package_dir).await.unwrap();

    let mut repo_blobs = repo.list_blobs().unwrap();
    repo_blobs.append(&mut startup_blobs);

    assert_eq!(env.blobfs.list_blobs().unwrap(), repo_blobs);

    env.stop().await;
}

// The backing blobfs is empty before the resolve.
fn verify_resolve(pkg: Package) -> impl Future<Output = ()> {
    verify_resolve_with_altered_env(pkg, async |_, _| {})
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn meta_far_only() {
    verify_resolve(PackageBuilder::new("uniblob").build().await.unwrap()).await
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn meta_far_and_empty_blob() {
    verify_resolve(
        PackageBuilder::new("emptyblob")
            .add_resource_at("data/empty", "".as_bytes())
            .build()
            .await
            .unwrap(),
    )
    .await
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn large_compressible_blobs() {
    let s = "large-compressible-blobs";
    verify_resolve(
        PackageBuilder::new(s)
            .add_resource_at("bin/numbers", &test_package_bin(s)[..])
            .add_resource_at("data/ones", io::repeat(1).take(1 * 1024 * 1024))
            .add_resource_at("data/twos", io::repeat(2).take(2 * 1024 * 1024))
            .add_resource_at("data/threes", io::repeat(3).take(3 * 1024 * 1024))
            .build()
            .await
            .unwrap(),
    )
    .await
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn large_uncompressible_blobs() {
    let s = "large-uncompressible-blobs";

    let mut rng = StdRng::from_seed([0u8; 32]);

    verify_resolve(
        PackageBuilder::new(s)
            .add_resource_at("data/1mb/1", RngReader(&mut rng).take(1 * 1024 * 1024))
            .add_resource_at("data/1mb/2", RngReader(&mut rng).take(1 * 1024 * 1024))
            .add_resource_at("data/1mb/3", RngReader(&mut rng).take(1 * 1024 * 1024))
            .add_resource_at("data/2mb", RngReader(&mut rng).take(2 * 1024 * 1024))
            .add_resource_at("data/3mb", RngReader(&mut rng).take(3 * 1024 * 1024))
            .build()
            .await
            .unwrap(),
    )
    .await
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn many_blobs() {
    verify_resolve(make_pkg_with_extra_blobs("many_blobs", 200).await).await
}

// TODO(https://fxbug.dev/542690944): Covered by pinned URLs forbidden.
#[fuchsia::test]
async fn pinned_merkle_resolution() {
    let env = TestEnvBuilder::new().build().await;

    // Since our test harness doesn't yet include a way to update a package, we generate two
    // separate packages to test resolution with a pinned merkle root.
    // We can do this with two packages because the TUF metadata doesn't currently contain
    // package names, only the latest known merkle root for a given name.
    // So, generate two packages, and then resolve one package with the merkle of the other
    // to test resolution with a pinned merkle.
    let pkg1 = PackageBuilder::new("pinned-merkle-foo")
        .add_resource_at("data/foo", "foo".as_bytes())
        .build()
        .await
        .unwrap();
    let pkg2 = PackageBuilder::new("pinned-merkle-bar")
        .add_resource_at("data/bar", "bar".as_bytes())
        .build()
        .await
        .unwrap();

    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg1)
            .add_package(&pkg2)
            .build()
            .await
            .unwrap(),
    );

    let served_repository = repo.server().start().unwrap();
    env.register_repo(&served_repository).await;

    let pkg1_url_with_pkg2_merkle =
        format!("fuchsia-pkg://test/pinned-merkle-foo?hash={}", pkg2.hash());

    let (package_dir, _resolved_context) =
        env.resolve_package(&pkg1_url_with_pkg2_merkle).await.unwrap();
    pkg2.verify_contents(&package_dir).await.unwrap();

    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority variant.
#[fuchsia::test]
async fn variant_resolution() {
    let env = TestEnvBuilder::new().build().await;
    let pkg = PackageBuilder::new("variant-foo")
        .add_resource_at("data/foo", "foo".as_bytes())
        .build()
        .await
        .unwrap();

    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );

    let served_repository = repo.server().start().unwrap();
    env.register_repo(&served_repository).await;

    let pkg_url = &"fuchsia-pkg://test/variant-foo/0";

    let (package_dir, _resolved_context) = env.resolve_package(pkg_url).await.unwrap();
    pkg.verify_contents(&package_dir).await.unwrap();

    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Migrate this to PkgAuthority.
#[fuchsia::test(logging_tags = ["RESOLVE_TEST"])]
async fn error_codes() {
    let env = TestEnvBuilder::new().build().await;
    let pkg = PackageBuilder::new("error-foo")
        .add_resource_at("data/foo", "foo".as_bytes())
        .build()
        .await
        .unwrap();

    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );

    let served_repository = repo.server().start().unwrap();
    env.register_repo(&served_repository).await;

    // Invalid URL
    assert_matches!(
        env.resolve_package("fuchsia-pkg://test/bad-url!").await,
        Err(fidl_fuchsia_pkg::ResolveError::InvalidUrl)
    );

    // Nonexistant repo
    assert_matches!(
        env.resolve_package("fuchsia-pkg://nonexistent-repo/a").await,
        Err(fidl_fuchsia_pkg::ResolveError::RepoNotFound)
    );

    // Nonexistant package
    assert_matches!(
        env.resolve_package("fuchsia-pkg://test/nonexistent").await,
        Err(fidl_fuchsia_pkg::ResolveError::PackageNotFound)
    );

    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn retries() {
    let env = TestEnvBuilder::new().build().await;

    let pkg = PackageBuilder::new("try-hard")
        .add_resource_at("data/foo", "bar".as_bytes())
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = repo
        .server()
        .response_overrider(responder::ForPathPrefix::new(
            "/blobs",
            responder::OncePerPath::new(responder::StaticResponseCode::server_error()),
        ))
        .start()
        .unwrap();
    env.register_repo(&served_repository).await;
    let (package_dir, _resolved_context) =
        env.resolve_package("fuchsia-pkg://test/try-hard").await.unwrap();
    pkg.verify_contents(&package_dir).await.unwrap();

    let hierarchy = env.pkg_resolver_inspect_hierarchy().await;
    let repo_blob_url = format!("{}/blobs/1", served_repository.local_url());
    let repo_blob_url = &repo_blob_url;
    assert_data_tree!(
        hierarchy,
        root: contains {
            repository_manager: contains {
                stats: contains {
                    mirrors: {
                        var repo_blob_url: {
                            network_blips: 2u64,
                            network_rate_limits: 0u64,
                        },
                    },
                },
            },
        }
    );

    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn handles_429_responses() {
    let env = TestEnvBuilder::new().build().await;

    let pkg1 = PackageBuilder::new("rate-limit-far")
        .add_resource_at("data/foo", "foo".as_bytes())
        .build()
        .await
        .unwrap();
    let pkg2 = PackageBuilder::new("rate-limit-content")
        .add_resource_at("data/bar", "bar".as_bytes())
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg1)
            .add_package(&pkg2)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = repo
        .server()
        .response_overrider(responder::ForPath::new(
            format!("/blobs/1/{}", pkg1.hash()),
            responder::ForRequestCount::new(2, responder::StaticResponseCode::too_many_requests()),
        ))
        .response_overrider(responder::ForPath::new(
            format!("/blobs/1/{}", pkg2.meta_contents().unwrap().contents()["data/bar"]),
            responder::ForRequestCount::new(2, responder::StaticResponseCode::too_many_requests()),
        ))
        .start()
        .unwrap();
    env.register_repo(&served_repository).await;

    // Simultaneously resolve both packages to minimize the time spent waiting for timeouts in
    // these tests.
    let proxy1 = env.connect_to_resolver();
    let proxy2 = env.connect_to_resolver();
    let pkg1_fut = resolve_package(&proxy1, "fuchsia-pkg://test/rate-limit-far");
    let pkg2_fut = resolve_package(&proxy2, "fuchsia-pkg://test/rate-limit-content");

    // The packages should resolve successfully.
    let (pkg1_res, pkg2_res) = join!(pkg1_fut, pkg2_fut);
    let (pkg1_dir, _resolved_context) = pkg1_res.unwrap();
    let (pkg2_dir, _resolved_context) = pkg2_res.unwrap();
    pkg1.verify_contents(&pkg1_dir).await.unwrap();
    pkg2.verify_contents(&pkg2_dir).await.unwrap();

    // And the inspect data for the package resolver should indicate that it handled 429 responses.
    let hierarchy = env.pkg_resolver_inspect_hierarchy().await;

    let repo_blob_url = format!("{}/blobs/1", served_repository.local_url());
    let repo_blob_url = &repo_blob_url;
    assert_data_tree!(
        hierarchy,
        root: contains {
            repository_manager: contains {
                stats: contains {
                    mirrors: {
                        var repo_blob_url: {
                            network_blips: 0u64,
                            network_rate_limits: 4u64,
                        },
                    },
                },
            },
        }
    );

    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn use_cached_package() {
    let env = TestEnvBuilder::new().build().await;

    let pkg = PackageBuilder::new("resolve-twice")
        .add_resource_at("data/foo", "bar".as_bytes())
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let fail_requests = responder::AtomicToggle::new(true);
    let served_repository = repo
        .server()
        .response_overrider(responder::Toggleable::new(
            &fail_requests,
            responder::StaticResponseCode::server_error(),
        ))
        .start()
        .unwrap();

    // the package can't be resolved before the repository is configured.
    assert_matches!(
        env.resolve_package("fuchsia-pkg://test/resolve-twice").await,
        Err(fidl_fuchsia_pkg::ResolveError::RepoNotFound)
    );

    env.register_repo(&served_repository).await;

    // the package can't be resolved before the repository can be updated without error.
    assert_matches!(
        env.resolve_package("fuchsia-pkg://test/resolve-twice").await,
        Err(fidl_fuchsia_pkg::ResolveError::UnavailableRepoMetadata)
    );

    // package resolves as expected.
    fail_requests.unset();
    let (package_dir, _resolved_context) =
        env.resolve_package("fuchsia-pkg://test/resolve-twice").await.unwrap();
    pkg.verify_contents(&package_dir).await.unwrap();

    // if no mirrors are accessible, the cached package is returned.
    fail_requests.set();
    let (package_dir, _resolved_context) =
        env.resolve_package("fuchsia-pkg://test/resolve-twice").await.unwrap();
    pkg.verify_contents(&package_dir).await.unwrap();

    env.stop().await;
    served_repository.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn meta_far_already_in_blobfs() {
    verify_resolve_with_altered_env(
        make_pkg_with_extra_blobs("meta_far_already_in_blobfs", 3).await,
        async |env, pkg| {
            env.add_file_with_hash_to_blobfs(pkg.meta_far().unwrap(), pkg.hash()).await
        },
    )
    .await
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn all_blobs_already_in_blobfs() {
    let s = "all_blobs_already_in_blobfs";
    verify_resolve_with_altered_env(make_pkg_with_extra_blobs(s, 3).await, async |env, pkg| {
        env.add_file_with_hash_to_blobfs(pkg.meta_far().unwrap(), pkg.hash()).await;
        env.add_slice_to_blobfs(&test_package_bin(s)[..]).await;
        for i in 0..3 {
            env.add_slice_to_blobfs(extra_blob_contents(s, i).as_slice()).await;
        }
    })
    .await
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn meta_far_and_one_content_blob_already_in_blobfs() {
    let s = "meta_far_and_one_content_blob_in_blobfs";
    verify_resolve_with_altered_env(make_pkg_with_extra_blobs(s, 3).await, async |env, pkg| {
        env.add_file_with_hash_to_blobfs(pkg.meta_far().unwrap(), pkg.hash()).await;
        env.add_slice_to_blobfs(&test_package_bin(s)[..]).await;
    })
    .await
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn test_concurrent_blob_writes() {
    // Create our test packages and find out the merkle of the duplicate blob
    let duplicate_blob_path = "blob/duplicate";
    let duplicate_blob_contents = &b"I am the duplicate"[..];
    let unique_blob_path = "blob/unique";
    let pkg1 = PackageBuilder::new("package1")
        .add_resource_at(duplicate_blob_path, duplicate_blob_contents)
        .build()
        .await
        .unwrap();
    let pkg2 = PackageBuilder::new("package2")
        .add_resource_at(duplicate_blob_path, duplicate_blob_contents)
        .add_resource_at(unique_blob_path, &b"I am unique"[..])
        .build()
        .await
        .unwrap();
    let duplicate_blob_merkle = pkg1.meta_contents().expect("extracted contents").contents()
        [duplicate_blob_path]
        .to_string();
    let unique_blob_merkle =
        pkg2.meta_contents().expect("extracted contents").contents()[unique_blob_path];

    // Create the responder and the channel to communicate with it
    let (blocking_responder, unblocking_closure_receiver) = responder::BlockResponseBodyOnce::new();
    let blocking_responder =
        responder::ForPath::new(format!("/blobs/1/{}", duplicate_blob_merkle), blocking_responder);

    // Construct the repo
    let env = TestEnvBuilder::new().build().await;
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg1)
            .add_package(&pkg2)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = repo.server().response_overrider(blocking_responder).start().unwrap();
    env.register_repo(&served_repository).await;

    // Construct the resolver proxies (clients)
    let resolver_proxy_1 = env.connect_to_resolver();
    let resolver_proxy_2 = env.connect_to_resolver();

    // Create a GET request to the hyper server for the duplicate blob
    let package1_resolution_fut =
        resolve_package(&resolver_proxy_1, &"fuchsia-pkg://test/package1");

    // Wait for GET request to be received by hyper server
    let send_shared_blob_body =
        unblocking_closure_receiver.await.expect("received unblocking future from hyper server");

    // At this point, we are confident that the duplicate blob is truncated. So, if we enqueue
    // another package resolve for a package that contains the duplicate blob the package resolver
    // should block resolving the package on that blob fetch finishing.
    let package2_resolution_fut =
        resolve_package(&resolver_proxy_2, &"fuchsia-pkg://test/package2");

    // Wait for the unique blob to exist in blobfs.
    let blobfs_reader = env.blobfs.blob_reader_proxy().unwrap();
    while blobfs_reader
        .get_vmo(unique_blob_merkle.as_bytes().try_into().unwrap())
        .await
        .expect("Getting vmo")
        .is_err()
    {
        fasync::Timer::new(Duration::from_millis(10)).await;
    }

    // At this point, both package resolves should be blocked on the shared blob download. Unblock
    // the server and verify both packages resolve to valid directories.
    send_shared_blob_body();
    let ((), ()) = futures::join!(
        async move {
            let (package1_dir, _resolved_context1) = package1_resolution_fut.await.unwrap();
            pkg1.verify_contents(&package1_dir).await.unwrap();
        },
        async move {
            let (package2_dir, _resolved_context2) = package2_resolution_fut.await.unwrap();
            pkg2.verify_contents(&package2_dir).await.unwrap();
        },
    );

    env.stop().await;
}

// TODO(b/308158482): re-enable when ring works on riscv64
#[cfg(not(target_arch = "riscv64"))]
async fn test_https_endpoint(pkg_name: &str, bind_addr: impl Into<IpAddr>) {
    let env = TestEnvBuilder::new().build().await;

    let pkg = PackageBuilder::new(pkg_name)
        .add_resource_at(format!("bin/{}", pkg_name), &test_package_bin(pkg_name)[..])
        .add_resource_at(format!("meta/{}.cml", pkg_name), &test_package_cml(pkg_name)[..])
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = repo
        .server()
        .use_https_domain(Domain::TestFuchsiaCom)
        .bind_to_addr(bind_addr)
        .start()
        .unwrap();

    env.register_repo(&served_repository).await;

    let (package, _resolved_context) = env
        .resolve_package(format!("fuchsia-pkg://test/{}", pkg_name).as_str())
        .await
        .expect("package to resolve without error");
    pkg.verify_contents(&package).await.unwrap();

    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
// TODO(b/308158482): re-enable when ring works on riscv64
#[cfg(not(target_arch = "riscv64"))]
#[fuchsia::test]
async fn https_endpoint_ipv6_only() {
    test_https_endpoint("https_endpoint_ipv6", Ipv6Addr::LOCALHOST).await
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
// TODO(b/308158482): re-enable when ring works on riscv64
#[cfg(not(target_arch = "riscv64"))]
#[fuchsia::test]
async fn https_endpoint_ipv4_only() {
    test_https_endpoint("https_endpoint_ipv4", Ipv4Addr::LOCALHOST).await
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn verify_concurrent_resolve() {
    let env = TestEnvBuilder::new().build().await;

    let pkg1 = make_pkg_with_extra_blobs("first_concurrent_resolve_pkg", 1).await;
    let pkg2 = make_pkg_with_extra_blobs("second_concurrent_resolve_pkg", 1).await;

    // Make a repo with both packages.
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg1)
            .add_package(&pkg2)
            .build()
            .await
            .unwrap(),
    );

    let pkg1_url = "fuchsia-pkg://test/first_concurrent_resolve_pkg";
    let pkg2_url = "fuchsia-pkg://test/second_concurrent_resolve_pkg";

    let path = format!("/blobs/1/{}", pkg1.content_blob_files().next().unwrap().merkle);
    let (blocker, mut chan) = responder::BlockResponseHeaders::new();
    let responder = responder::ForPath::new(path, blocker);

    let served_repository = repo.server().response_overrider(responder).start().unwrap();
    env.register_repo(&served_repository).await;

    // First resolve should block per the responder we use.
    let pkg1_resolve_fut = env.resolve_package(&pkg1_url);
    // Get the BlockedResponse to make sure we're blocking the above resolve
    // before we try to resolve the second package.
    let blocked_response = chan.next().await.unwrap();
    // Now await on resolving another package.

    assert_matches!(env.resolve_package(&pkg2_url).await, Ok(_));
    // Finally unblock the first resolve to safe
    blocked_response.unblock();
    assert_matches!(pkg1_resolve_fut.await, Ok(_));

    // Tear down the test environment now so it doesn't live until another
    // test environment is created which could cause an OOM.
    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
// Merkle-pinned resolves verify that there is a package of that name in TUF, but then
// download the meta.far directly from the blob url. This test verifies that the resolver
// does not use the size of the meta.far found in TUF, since the pinned meta.far could
// differ in size.
#[fuchsia::test]
async fn merkle_pinned_meta_far_size_different_than_tuf_metadata() {
    let env = TestEnvBuilder::new().build().await;
    // Content chunks in FARs are 4k aligned, so a meta.far for an empty package will be 12k
    // because of meta/package, meta/fuchsia.abi/abi-revision, and meta/contents is empty..
    let pkg_12k_tuf = PackageBuilder::new("merkle-pin-size").build().await.unwrap();
    assert_eq!(pkg_12k_tuf.meta_far().unwrap().metadata().unwrap().len(), 3 * 4096);
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&pkg_12k_tuf)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();

    // Put the larger, merkle-pinned package in /blobs.
    let pkg_16k_pinned = PackageBuilder::new("merkle-pin-size")
        .add_resource_at("meta/zero", &[0u8][..])
        .build()
        .await
        .unwrap();
    assert_eq!(pkg_16k_pinned.meta_far().unwrap().metadata().unwrap().len(), 4 * 4096);
    let pkg_16k_pinned_data = std::fs::read(pkg_16k_pinned.artifacts().join("meta.far")).unwrap();
    let _delivery_size = repo.write_blob(pkg_16k_pinned.hash(), &pkg_16k_pinned_data).unwrap();

    let repo_url = "fuchsia-pkg://test".parse().unwrap();
    let repo_config = served_repository.make_repo_config(repo_url);
    let () = env.proxies.repo_manager.add(&repo_config.into()).await.unwrap().unwrap();

    let pinned_url = format!("fuchsia-pkg://test/merkle-pin-size?hash={}", pkg_16k_pinned.hash());
    let (resolved_pkg, _resolved_context) =
        env.resolve_package(&pinned_url).await.expect("package to resolve without error");
    pkg_16k_pinned.verify_contents(&resolved_pkg).await.unwrap();

    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn superpackage() {
    let env = TestEnvBuilder::new().build().await;
    let startup_blobs = env.blobfs.list_blobs().unwrap();

    let subpackage = PackageBuilder::new("subpackage")
        .add_resource_at("subpackage-blob", "subpackage-blob-contents".as_bytes())
        .build()
        .await
        .unwrap();
    assert_eq!(startup_blobs.intersection(&subpackage.list_blobs()).count(), 0);
    let superpackage = PackageBuilder::new("superpackage")
        .add_subpackage("my-subpackage", &subpackage)
        .build()
        .await
        .unwrap();
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&superpackage)
            .add_package(&subpackage)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = Arc::clone(&repo).server().start().unwrap();
    let repo_config = served_repository.make_repo_config("fuchsia-pkg://test".parse().unwrap());
    let () = env.proxies.repo_manager.add(&repo_config.into()).await.unwrap().unwrap();

    let (package, _resolved_context) = env
        .resolve_package("fuchsia-pkg://test/superpackage")
        .await
        .expect("package to resolve without error");

    superpackage.verify_contents(&package).await.unwrap();
    assert!(env.blobfs.list_blobs().unwrap().is_superset(&subpackage.list_blobs()));

    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn fxblob() {
    let env = TestEnvBuilder::new().fxblob().build().await;
    let pkg = make_pkg_with_extra_blobs("using-fx-blob", 1).await;
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

    let (resolved_pkg, _resolved_context) =
        env.resolve_package("fuchsia-pkg://test/using-fx-blob").await.unwrap();

    pkg.verify_contents(&resolved_pkg).await.unwrap();

    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn ota_resolver_does_not_protect_blobs_from_gc() {
    let env = TestEnvBuilder::new().build().await;
    let pkg = PackageBuilder::new("unprotected-package")
        .add_resource_at("unprotected-blob", &b"unprotected-blob-contents"[..])
        .build()
        .await
        .unwrap();
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

    let (resolved_pkg, _resolved_context) =
        lib::resolve_package(&env.proxies.resolver_ota, "fuchsia-pkg://test/unprotected-package")
            .await
            .unwrap();
    pkg.verify_contents(&resolved_pkg).await.unwrap();

    let () = env.proxies.space_manager.gc().await.unwrap().unwrap();

    assert_matches!(
        pkg.verify_contents(&resolved_pkg).await,
        Err(fuchsia_pkg_testing::VerificationError::MissingFile{path}) if path == "unprotected-blob"
    );

    env.stop().await;
}

// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn resolve_of_already_cached_package_is_not_blocked_by_in_progress_blob_fetches() {
    let env = TestEnvBuilder::new().blob_download_concurrency_limit(1).build().await;

    let blocking_pkg = PackageBuilder::new("blocking-package").build().await.unwrap();
    let already_cached_subpackage = PackageBuilder::new("already-cached-subpackage")
        .add_resource_at(
            "already-cached-subpackage-blob",
            &b"already-cached-subpackage-blob-content"[..],
        )
        .build()
        .await
        .unwrap();
    let already_cached_superpackage = PackageBuilder::new("already-cached-superpackage")
        .add_subpackage("subpackage", &already_cached_subpackage)
        .add_resource_at(
            "already-cached-superpackage-blob",
            &b"already-cached-superpackage-blob-content"[..],
        )
        .build()
        .await
        .unwrap();
    let () = already_cached_superpackage.write_to_blobfs(&env.blobfs).await;

    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&blocking_pkg)
            .add_package(&already_cached_superpackage)
            .build()
            .await
            .unwrap(),
    );
    let (blocker, mut blocked_fetches) = responder::BlockResponseHeaders::new();
    let responder = responder::ForPathPrefix::new("/blobs/1/", blocker);
    let served_repository = repo.server().response_overrider(responder).start().unwrap();
    env.register_repo(&served_repository).await;

    // Wait for the meta.far to be blocked.
    let mut blocking_pkg_resolve =
        std::pin::pin!(env.resolve_package("fuchsia-pkg://test/blocking-package").fuse());
    let blocked_meta_far = blocked_fetches.next().await.unwrap();

    // Already cached package should resolve even with a full blob fetch queue,
    // and the blocking resolve should not complete.
    futures::select_biased! {
        _  = blocking_pkg_resolve => panic!("blocking resolve should not complete"),
        already_cached_resolve = env
            .resolve_package("fuchsia-pkg://test/already-cached-superpackage").fuse() => {
            let _: (fio::DirectoryProxy, _) = already_cached_resolve.unwrap();
        }
    }

    // Finish the blocked resolve to make sure it completes successfully and didn't get canceled by
    // timeout.
    let () = blocked_meta_far.unblock();
    let _: (fio::DirectoryProxy, _) = blocking_pkg_resolve.await.unwrap();

    env.stop().await;
}

// To limit concurrency and deduplicate work, pkg-resolver has a package resolve queue and a blob
// fetch queue.
// The blob fetch queue also ensures that no particular blob is being written to blobfs more than
// once at a time (to avoid errors, as c++blobfs does not support concurrent creates of the same
// blob).
// During a resolve, pkg-resolver first fetches the meta.far using the queue. If the blob fetch
// queue is already at its concurrency limit, the straight-forward implementation of this would
// result in the resolve of a fully-cached package being blocked until the queue works through its
// backlog of blob fetches so it can process the (no-op) fetch of the package's meta.far.
// To avoid blocking resolves of fully-cached packages, before adding the meta.far to the queue
// pkg-resolver checks via pkg-cache to see if the meta.far is already resident.
// When first implemented, the result of this check (which contains an object that allows writing
// the blob to blobfs) was reused in the queue, which is incorrect, because the queue could already
// contain a request to fetch the blob (e.g. if the package is also a subpackage of another package
// that was being resolved concurrently), and so by the time the queue started processing the fetch
// of the meta.far, the blob would have already been fetched and the result would be out-of-date,
// which would result in a blob fetch error that would fail the resolve.
//
// This test makes sure that resolves succeed even if the meta.far peek optimization is performed
// when the meta.far is not in blobfs but a request to fetch the meta.far is already in the queue.
// TODO(https://fxbug.dev/542690944): Covered by PkgAuthority happy-path.
#[fuchsia::test]
async fn already_cached_package_blob_queue_bypass_with_concurrent_meta_far_write() {
    let sub_pkg = PackageBuilder::new("subpackage").build().await.unwrap();
    let super_pkg = PackageBuilder::new("super-pkg")
        .add_subpackage("my-subpackage", &sub_pkg)
        .build()
        .await
        .unwrap();

    let (blocker, mut chan) = responder::BlockResponseHeaders::new();
    let responder = responder::ForPath::new(format!("/blobs/1/{}", sub_pkg.hash()), blocker);

    let env = TestEnvBuilder::new().fxblob().build().await;
    let repo = Arc::new(
        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH)
            .add_package(&sub_pkg)
            .add_package(&super_pkg)
            .build()
            .await
            .unwrap(),
    );
    let served_repository = repo.server().response_overrider(responder).start().unwrap();
    env.register_repo(&served_repository).await;

    // Separate resolver proxies to guarantee concurrent processing of resolves.
    let resolver_proxy_0 = env.connect_to_resolver();
    let resolver_proxy_1 = env.connect_to_resolver();

    // Resolve the superpackage, which will trigger a fetch of the subpackage meta.far.
    let super_pkg_fut = resolve_package(&resolver_proxy_0, "fuchsia-pkg://test/super-pkg");

    // Wait until the meta.far is requested from the repo to be sure that the fetch is in the queue,
    // and block the response so that the fetch stays in the queue until the subpackage resolve
    // performs the optimization.
    let blocked_response = chan.next().await.unwrap();

    // Start the subpackage resolve and wait until the fetch is added to the queue to be sure that
    // the optimization was performed while the other fetch was still in the queue.
    let sub_pkg_fut = resolve_package(&resolver_proxy_1, "fuchsia-pkg://test/subpackage");
    let () = env
        .wait_for_pkg_resolver_inspect_state(tree_assertion!(
            root: contains {
                blob_fetcher: contains {
                    raw_queue: {
                        sub_pkg.hash().to_string() => {
                            running: 1u64,
                            pending: 1u64,
                        }
                    }
                }
            }
        ))
        .await;

    // Unblock the repo's response for the superpackage's resolve's fetch of the meta.far.
    // There should only be one request because the second fetch should re-check for the meta.far
    // in blobfs.
    let () = blocked_response.unblock();

    let _: (fio::DirectoryProxy, pkg::ResolutionContext) = super_pkg_fut.await.unwrap();
    let _: (fio::DirectoryProxy, pkg::ResolutionContext) = sub_pkg_fut.await.unwrap();

    env.stop().await;
}
