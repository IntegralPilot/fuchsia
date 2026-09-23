// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use ramdevice_client::{RamdiskClient, RamdiskClientBuilder};
use zx::Vmo;

const RAMDISK_BLOCK_SIZE: u64 = 512;

/// Loads a test resource image into a VMO and bind it as a ramdisk to /dev.
///
/// When the returned RamdiskClient is Dropped it will schedule the ramdisk to unbind. Callers
/// must keep a reference to the RamdiskClient while the ramdisk is needed.
pub async fn mount_image_as_ramdisk(resource_path: &str) -> RamdiskClient {
    let image_buffer = fuchsia_fs::file::read_in_namespace(resource_path).await.unwrap();
    let image_size = image_buffer.len();
    let image_vmo = Vmo::create(image_size.try_into().unwrap()).unwrap();
    image_vmo.write(&image_buffer, 0).unwrap();

    let ramdisk_client = RamdiskClientBuilder::new_with_vmo(image_vmo, Some(RAMDISK_BLOCK_SIZE))
        .publish()
        .build()
        .await
        .unwrap();

    ramdisk_client
}
