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

use fidl::endpoints::{DiscoverableProtocolMarker, RequestStream};
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_merkle::Hash;
use futures::prelude::*;
use std::sync::Arc;
use vfs::ObjectRequest;
use zx::Status;

#[derive(Clone)]
enum WriterFailure {
    OnGetVmo,
    OnBytesReady,
}

#[derive(Clone)]
enum CreatorFailure {
    OnCreate,
    OnNeedsOverwrite,
}

#[derive(Clone)]
enum FailureSource {
    Creator(CreatorFailure),
    Writer(WriterFailure),
}

struct BlobFsWithFileCreateOverride {
    wrapped: blobfs_ramdisk::BlobfsRamdisk,
    target: (Hash, FailureSource),
    system_image: Hash,
}

impl crate::Blobfs for BlobFsWithFileCreateOverride {
    fn root_proxy(&self) -> fio::DirectoryProxy {
        self.wrapped.root_dir_handle().unwrap().into_proxy()
    }

    fn svc_dir(&self) -> fio::DirectoryProxy {
        let inner = self.wrapped.svc_dir().unwrap();
        let (client, server) = fidl::endpoints::create_request_stream::<fio::DirectoryMarker>();
        ServiceDirectoryWithBlobCreateOverride { inner, target: self.target.clone() }.spawn(server);
        client.into_proxy()
    }

    fn blob_creator_proxy(&self) -> Option<ffxfs::BlobCreatorProxy> {
        panic!("not implemented");
    }
    fn blob_reader_proxy(&self) -> ffxfs::BlobReaderProxy {
        panic!("not implemented");
    }
}

#[derive(Clone)]
struct ServiceDirectoryWithBlobCreateOverride {
    inner: fio::DirectoryProxy,
    target: (Hash, FailureSource),
}

impl ServiceDirectoryWithBlobCreateOverride {
    fn spawn(self, stream: fio::DirectoryRequestStream) {
        fasync::Task::spawn(self.serve(stream)).detach();
    }

    async fn serve(self, mut stream: fio::DirectoryRequestStream) {
        while let Some(req) = stream.next().await {
            match req.unwrap() {
                fio::DirectoryRequest::Open { path, flags, options, object, control_handle: _ } => {
                    ObjectRequest::new(flags, &options, object).handle(|request| {
                        if path == "." {
                            let stream = fio::NodeRequestStream::from_channel(
                                fasync::Channel::from_channel(request.take().into_channel()),
                            )
                            .cast_stream();
                            self.clone().spawn(stream);
                        } else if path == ffxfs::BlobReaderMarker::PROTOCOL_NAME {
                            self.inner
                                .open(&path, flags, &options, request.take().into_channel())
                                .unwrap();
                        } else if path == ffxfs::BlobCreatorMarker::PROTOCOL_NAME {
                            let (client, server) =
                                fidl::endpoints::create_proxy::<ffxfs::BlobCreatorMarker>();
                            self.inner.open(&path, flags, &options, server.into_channel()).unwrap();
                            FakeCreator { inner: client, target: self.target.clone() }.spawn(
                                request
                                    .take()
                                    .into_server_end::<ffxfs::BlobCreatorMarker>()
                                    .into_stream(),
                            );
                        } else {
                            // The channel will be dropped and closed if the wire call fails.
                            let _ = self.inner.open(
                                &path,
                                flags,
                                &request.options(),
                                request.take().into_channel(),
                            );
                        }
                        Ok(())
                    });
                }
                request => panic!("Unhandled fuchsia.io/Directory request: {request:?}"),
            }
        }
    }
}

struct FakeCreator {
    inner: ffxfs::BlobCreatorProxy,
    target: (Hash, FailureSource),
}

impl FakeCreator {
    fn spawn(self, stream: ffxfs::BlobCreatorRequestStream) {
        fasync::Task::spawn(self.serve(stream)).detach();
    }

    async fn serve(self, mut stream: ffxfs::BlobCreatorRequestStream) {
        while let Some(req) = stream.next().await {
            match req.unwrap() {
                ffxfs::BlobCreatorRequest::Create { responder, hash, allow_existing } => {
                    if hash.as_slice() == self.target.0.as_slice() {
                        match &self.target.1 {
                            FailureSource::Creator(CreatorFailure::OnCreate) => {
                                responder
                                    .send_no_shutdown_on_err(Err(ffxfs::CreateBlobError::Internal))
                                    .unwrap();
                            }
                            FailureSource::Creator(CreatorFailure::OnNeedsOverwrite) => {
                                responder
                                    .send_no_shutdown_on_err(
                                        self.inner.create(&hash, allow_existing).await.unwrap(),
                                    )
                                    .unwrap();
                            }
                            FailureSource::Writer(writer_failure) => {
                                let (client, server) = fidl::endpoints::create_request_stream::<
                                    ffxfs::BlobWriterMarker,
                                >();
                                FakeWriter { failure_type: writer_failure.clone() }.spawn(server);
                                responder.send(Ok(client)).unwrap();
                            }
                        }
                    } else {
                        responder
                            .send_no_shutdown_on_err(
                                self.inner.create(&hash, allow_existing).await.unwrap(),
                            )
                            .unwrap();
                    }
                }
                ffxfs::BlobCreatorRequest::NeedsOverwrite { responder, blob_hash } => {
                    match &self.target.1 {
                        FailureSource::Creator(CreatorFailure::OnNeedsOverwrite) => {
                            responder
                                .send_no_shutdown_on_err(Err(Status::BAD_STATE.into_raw()))
                                .unwrap();
                        }
                        _ => {
                            responder
                                .send_no_shutdown_on_err(
                                    self.inner.needs_overwrite(&blob_hash).await.unwrap(),
                                )
                                .unwrap();
                        }
                    }
                }
            }
        }
    }
}

struct FakeWriter {
    failure_type: WriterFailure,
}

impl FakeWriter {
    fn spawn(self, stream: ffxfs::BlobWriterRequestStream) {
        fasync::Task::spawn(self.serve(stream)).detach();
    }

    async fn serve(self, mut stream: ffxfs::BlobWriterRequestStream) {
        while let Some(req) = stream.next().await {
            match (&self.failure_type, req.unwrap()) {
                (
                    &WriterFailure::OnBytesReady,
                    ffxfs::BlobWriterRequest::BytesReady { responder, .. },
                ) => {
                    let _ = responder.send(Err(zx::Status::BAD_STATE.into_raw()));
                }
                (_, ffxfs::BlobWriterRequest::BytesReady { responder, .. }) => {
                    let _ = responder.send(Ok(()));
                }
                (&WriterFailure::OnGetVmo, ffxfs::BlobWriterRequest::GetVmo { responder, .. }) => {
                    let _ = responder.send(Err(zx::Status::NO_MEMORY.into_raw()));
                }
                (_, ffxfs::BlobWriterRequest::GetVmo { responder, .. }) => {
                    let _ = match zx::Vmo::create(8192) {
                        Ok(vmo) => responder.send(Ok(vmo)),
                        Err(status) => responder.send(Err(status.into_raw())),
                    };
                }
            }
        }
    }
}

async fn make_blobfs_with_minimal_system_image() -> (blobfs_ramdisk::BlobfsRamdisk, Hash) {
    let blobfs = blobfs_ramdisk::BlobfsRamdisk::start().await.unwrap();
    let system_image = fuchsia_pkg_testing::SystemImageBuilder::new().build().await;
    system_image.write_to_blobfs(&blobfs).await;
    (blobfs, *system_image.hash())
}

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

async fn make_pkg_for_mock_blobfs_tests(
    package_name: &str,
) -> (fuchsia_pkg_testing::Package, Hash, Hash) {
    let pkg = make_pkg_with_extra_blobs(package_name, 1).await;
    let pkg_merkle = *pkg.hash();
    let blob_merkle = fuchsia_merkle::root_from_slice(extra_blob_contents(package_name, 0));
    (pkg, pkg_merkle, blob_merkle)
}

async fn make_mock_blobfs_with_failing_install_pkg(
    package_name: &str,
    failure_source: FailureSource,
) -> (BlobFsWithFileCreateOverride, fuchsia_pkg_testing::Package) {
    let (blobfs, system_image) = make_blobfs_with_minimal_system_image().await;
    let (pkg, pkg_merkle, _) = make_pkg_for_mock_blobfs_tests(package_name).await;

    (
        BlobFsWithFileCreateOverride {
            wrapped: blobfs,
            target: (pkg_merkle, failure_source),
            system_image,
        },
        pkg,
    )
}

async fn make_mock_blobfs_with_failing_install_blob(
    package_name: &str,
    failure_source: FailureSource,
) -> (BlobFsWithFileCreateOverride, fuchsia_pkg_testing::Package) {
    let (blobfs, system_image) = make_blobfs_with_minimal_system_image().await;
    let (pkg, _pkg_merkle, blob_merkle) = make_pkg_for_mock_blobfs_tests(package_name).await;

    (
        BlobFsWithFileCreateOverride {
            wrapped: blobfs,
            target: (blob_merkle, failure_source),
            system_image,
        },
        pkg,
    )
}

async fn assert_resolve_package_with_failing_blobfs(
    blobfs: BlobFsWithFileCreateOverride,
    pkg: fuchsia_pkg_testing::Package,
    expected_res: Result<(), fidl_fuchsia_pkg::ResolveError>,
) {
    let repo = fuchsia_pkg_testing::RepositoryBuilder::from_template_dir(crate::EMPTY_REPO_PATH)
        .add_package(&pkg)
        .build()
        .await
        .unwrap();
    let served_repository = Arc::new(repo).server().start().unwrap();
    let repo_config =
        served_repository.make_repo_config("fuchsia-pkg://example.org".parse().unwrap());
    let system_image = blobfs.system_image;
    let env = crate::TestEnv::builder()
        .blobfs_and_system_image_hash(blobfs, Some(system_image))
        .pkg_authority(crate::MockPkgAuthority::from_repo_config_and_packages(
            &repo_config,
            &[&pkg],
        ))
        .build()
        .await;
    let res = env.resolve_ota(&format!("fuchsia-pkg://example.org/{}", pkg.name())).await;

    assert_eq!(res.map(|_| ()), expected_res);
}

#[fuchsia::test]
async fn fails_on_create_far_in_install_pkg() {
    let (blobfs, pkg) = make_mock_blobfs_with_failing_install_pkg(
        "fails_on_open_far_in_install_pkg",
        FailureSource::Creator(CreatorFailure::OnCreate),
    )
    .await;

    assert_resolve_package_with_failing_blobfs(blobfs, pkg, Err(fidl_fuchsia_pkg::ResolveError::Io))
        .await
}

// pkg-cache tries to overwrite if BlobCreator.NeedsOverwrite fails.
#[fuchsia::test]
async fn succeeds_on_needs_overwrite_far_in_install_pkg() {
    let (blobfs, pkg) = make_mock_blobfs_with_failing_install_pkg(
        "fails_on_needs_overwrite_far_in_install_pkg",
        FailureSource::Creator(CreatorFailure::OnNeedsOverwrite),
    )
    .await;

    assert_resolve_package_with_failing_blobfs(blobfs, pkg, Ok(())).await
}

#[fuchsia::test]
async fn fails_get_vmo_far_in_install_pkg() {
    let (blobfs, pkg) = make_mock_blobfs_with_failing_install_pkg(
        "fails_truncate_far_in_install_pkg",
        FailureSource::Writer(WriterFailure::OnGetVmo),
    )
    .await;

    assert_resolve_package_with_failing_blobfs(blobfs, pkg, Err(fidl_fuchsia_pkg::ResolveError::Io))
        .await
}

#[fuchsia::test]
async fn fails_bytes_ready_far_in_install_pkg() {
    let (blobfs, pkg) = make_mock_blobfs_with_failing_install_pkg(
        "fails_write_far_in_install_pkg",
        FailureSource::Writer(WriterFailure::OnBytesReady),
    )
    .await;

    assert_resolve_package_with_failing_blobfs(blobfs, pkg, Err(fidl_fuchsia_pkg::ResolveError::Io))
        .await
}

#[fuchsia::test]
async fn fails_on_create_blob_in_install_blob() {
    let (blobfs, pkg) = make_mock_blobfs_with_failing_install_blob(
        "fails_on_open_blob_in_install_blob",
        FailureSource::Creator(CreatorFailure::OnCreate),
    )
    .await;

    assert_resolve_package_with_failing_blobfs(blobfs, pkg, Err(fidl_fuchsia_pkg::ResolveError::Io))
        .await
}

// pkg-cache tries to overwrite if BlobCreator.NeedsOverwrite fails.
#[fuchsia::test]
async fn succeeds_on_needs_overwrite_blob_in_install_blob() {
    let (blobfs, pkg) = make_mock_blobfs_with_failing_install_blob(
        "fails_on_needs_overwrite_blob_in_install_blob",
        FailureSource::Creator(CreatorFailure::OnNeedsOverwrite),
    )
    .await;

    assert_resolve_package_with_failing_blobfs(blobfs, pkg, Ok(())).await
}

#[fuchsia::test]
async fn fails_get_vmo_blob_in_install_blob() {
    let (blobfs, pkg) = make_mock_blobfs_with_failing_install_blob(
        "fails_truncate_blob_in_install_blob",
        FailureSource::Writer(WriterFailure::OnGetVmo),
    )
    .await;

    assert_resolve_package_with_failing_blobfs(blobfs, pkg, Err(fidl_fuchsia_pkg::ResolveError::Io))
        .await
}

#[fuchsia::test]
async fn fails_bytes_ready_blob_in_install_blob() {
    let (blobfs, pkg) = make_mock_blobfs_with_failing_install_blob(
        "fails_write_blob_in_install_blob",
        FailureSource::Writer(WriterFailure::OnBytesReady),
    )
    .await;

    assert_resolve_package_with_failing_blobfs(blobfs, pkg, Err(fidl_fuchsia_pkg::ResolveError::Io))
        .await
}
