// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::TestEnv;
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_io as fio;
use futures::TryFutureExt;
use zx::Status;

struct BrokenBlobfs;

impl crate::Blobfs for BrokenBlobfs {
    fn root_proxy(&self) -> fio::DirectoryProxy {
        fidl::endpoints::create_proxy::<fio::DirectoryMarker>().0
    }
    fn svc_dir(&self) -> fio::DirectoryProxy {
        fidl::endpoints::create_proxy::<fio::DirectoryMarker>().0
    }
    fn blob_creator_proxy(&self) -> Option<ffxfs::BlobCreatorProxy> {
        None
    }
    fn blob_reader_proxy(&self) -> fidl_fuchsia_fxfs::BlobReaderProxy {
        fidl::endpoints::create_proxy::<ffxfs::BlobReaderMarker>().0
    }
}

#[fuchsia::test]
async fn sync_success() {
    let env = TestEnv::builder().build().await;

    let res = env.proxies.package_cache.sync().await;

    assert_eq!(res.unwrap(), Ok(()));
}

#[fuchsia::test]
async fn sync_returns_errs() {
    let env = TestEnv::builder()
        .blobfs_and_system_image_hash(BrokenBlobfs, None)
        .require_system_image(false)
        .build()
        .await;

    assert_eq!(
        env.proxies
            .package_cache
            .sync()
            .map_ok(|res| res.map_err(Status::err_from_raw))
            .await
            .unwrap(),
        Err(Status::INTERNAL)
    );
}
