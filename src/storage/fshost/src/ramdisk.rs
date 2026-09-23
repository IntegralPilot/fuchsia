// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Functionality for extracting a ramdisk image from the zircon boot items. This ramdisk contains
//! an fvm with blobfs and data volumes, and is intended to be used in conjunction with the
//! ramdisk_image option, in run modes where we need to operate on the real disk and can't run
//! filesystems off it, such as recovery.

use crate::device::{Device, LocalBlockDevice};
use anyhow::{Context, Error, ensure};
use fuchsia_component::client::connect_to_protocol;
use vmo_backed_block_server::VmoBackedServer;

// Defined in //sdk/lib/zbi-format/include/lib/zbi-format/internal/storage.h.
const ZBI_FLAGS_STORAGE_COMPRESSED: zbi::Flags = zbi::Flags::from_bits_retain(1);

async fn create_ramdisk(zbi_vmo: zx::Vmo) -> Result<Box<dyn Device>, Error> {
    let mut header_buf = [0u8; std::mem::size_of::<zbi::Header>()];
    zbi_vmo.read(&mut header_buf, 0).context("reading zbi item header")?;
    // Expect is fine here - we made the buffer ourselves to the exact size of the header so
    // something is very wrong if we trip this.
    let header: zbi::Header = unsafe { std::mem::transmute(header_buf) };

    ensure!(
        header.flags.contains(zbi::Flags::VERSION),
        "invalid ZBI_TYPE_STORAGE_RAMDISK item header: flags"
    );
    ensure!(header.magic == zbi::ITEM_MAGIC, "invalid ZBI_TYPE_STORAGE_RAMDISK item header: magic");
    ensure!(
        header.r#type == zbi::Type::StorageRamdisk,
        "invalid ZBI_TYPE_STORAGE_RAMDISK item header: type"
    );

    // TODO(https://fxbug.dev/42109921): The old code ignored uncompressed items too, and silently.  Really
    // the protocol should be cleaned up so the VMO arrives without the header in it and then it
    // could just be used here directly if uncompressed (or maybe bootsvc deals with decompression
    // in the first place so the uncompressed VMO is always what we get).
    ensure!(
        header.flags.contains(ZBI_FLAGS_STORAGE_COMPRESSED),
        "ignoring uncompressed RAMDISK item in ZBI"
    );

    let ramdisk_vmo = zx::Vmo::create(header.extra as u64).context("making output vmo")?;
    let mut compressed_buf = vec![0u8; header.length as usize];
    zbi_vmo
        .read(&mut compressed_buf, std::mem::size_of::<zbi::Header>() as u64)
        .context("reading compressed ramdisk")?;
    let decompressed_buf =
        zstd::decode_all(compressed_buf.as_slice()).context("zstd decompression failed")?;
    ramdisk_vmo.write(&decompressed_buf, 0).context("writing decompressed contents to vmo")?;

    let server =
        VmoBackedServer::from_vmo(512, ramdisk_vmo).context("building ramdisk from vmo")?;

    Ok(Box::new(LocalBlockDevice::new(server).await?))
}

/// Set up a ramdisk provided by the boot items service as a vmo. If there is no vmo provided, None
/// is returned. If there is, the ramdisk is decoded and set up, and the topological path is
/// returned.
pub async fn set_up_ramdisk() -> Result<Option<Box<dyn Device>>, Error> {
    let proxy = connect_to_protocol::<fidl_fuchsia_boot::ItemsMarker>()?;
    let (maybe_vmo, _length) = proxy
        .get(zbi::Type::StorageRamdisk.into(), 0)
        .await
        .context("boot items get failed (fidl failure)")?;

    if let Some(vmo) = maybe_vmo { Ok(Some(create_ramdisk(vmo).await?)) } else { Ok(None) }
}
