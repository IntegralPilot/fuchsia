// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_erofs::{ErofsMarker, ErofsServeRequest};
use fidl_fuchsia_io as fio;
use fuchsia_component::client::connect_to_protocol_sync;
use starnix_core::fs::fuchsia::RemoteFs;
use starnix_core::mm::ProtectionFlags;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::{CacheMode, FileSystem, FileSystemHandle, FileSystemOptions};
use starnix_logging::{impossible_error, log_error, log_info};
use starnix_uapi::errors::Errno;
use starnix_uapi::mount_flags::FileSystemFlags;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{errno, from_status_like_fdio};
use std::sync::atomic::Ordering;
use zx::{MonotonicInstant, VmoChildOptions};

pub fn new_fs(
    current_task: &CurrentTask,
    options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    let kernel = current_task.kernel();

    // EROFS is strictly read-only.
    if !options.flags.load(Ordering::Relaxed).contains(FileSystemFlags::RDONLY) {
        return Err(errno!(EROFS));
    }

    let fsoffset = options.params.get_as::<u64>(b"fsoffset")?;

    let source_device = current_task.open_file(options.source.as_ref(), OpenFlags::RDONLY)?;
    let memory = source_device.get_memory(current_task, None, ProtectionFlags::READ)?;
    let backing_vmo = memory
        .as_vmo()
        .ok_or_else(|| errno!(EINVAL))?
        .duplicate_handle(zx::Rights::SAME_RIGHTS)
        .map_err(impossible_error)?;

    let backing_vmo = match fsoffset {
        None => {
            log_info!("mounting erofs image source={:?}", options.source);
            backing_vmo
        }
        Some(fsoffset) if fsoffset > 0 => {
            let page_size = zx::system_get_page_size() as u64;
            if fsoffset % page_size != 0 {
                log_error!("fsoffset {} is not page-aligned (page size {})", fsoffset, page_size);
                return Err(errno!(EINVAL));
            }
            let vmo_size = backing_vmo.get_size().map_err(impossible_error)?;
            let clone_len = vmo_size.checked_sub(fsoffset).ok_or_else(|| {
                log_error!("fsoffset {} exceeds vmo size {}", fsoffset, vmo_size);
                errno!(EINVAL)
            })?;
            log_info!("mounting erofs image with fsoffset={}, len={}", fsoffset, clone_len);
            backing_vmo.create_child(VmoChildOptions::SLICE, fsoffset, clone_len).map_err(|e| {
                log_error!("Failed to create child VMO slice for fsoffset {}: {:?}", fsoffset, e);
                errno!(EINVAL)
            })?
        }
        Some(_) => {
            log_info!("mounting erofs image with fsoffset of zero");
            backing_vmo
        }
    };

    let erofs_provider = connect_to_protocol_sync::<ErofsMarker>().map_err(|_| errno!(ENOENT))?;

    let (root_client, root_server) = fidl::endpoints::create_endpoints();
    erofs_provider
        .serve(
            ErofsServeRequest {
                backing_vmo: Some(backing_vmo),
                root: Some(root_server),
                ..Default::default()
            },
            MonotonicInstant::INFINITE,
        )
        .map_err(|e| {
            log_error!("FIDL error on Erofs.Serve: {e:?}");
            errno!(EIO)
        })?
        .map_err(|e| {
            log_error!("Erofs.Serve failed: {e:?}");
            from_status_like_fdio!(zx::Status::err_from_raw(e))
        })?;

    let (remotefs, root_node, info, node_id) =
        RemoteFs::new(root_client.into_channel(), fio::PERM_READABLE, "erofs")?;

    let fs =
        FileSystem::new(kernel, CacheMode::Cached(kernel.fs_cache_config()), remotefs, options)?;

    fs.create_root_with_info(node_id, root_node, info);

    Ok(fs)
}
