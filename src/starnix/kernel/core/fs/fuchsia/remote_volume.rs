// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fs::fuchsia::RemoteFs;
use crate::task::CurrentTask;
use crate::task::dynamic_thread_spawner::SpawnRequestBuilder;
use crate::vfs::{
    CacheMode, FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions, FsStr, RenameContext,
};
use fidl::endpoints::{DiscoverableProtocolMarker, SynchronousProxy, create_sync_proxy};
use fidl_fuchsia_fshost::StarnixVolumeProviderMarker;
use fidl_fuchsia_fxfs::{CryptMarker, DebugMarker};
use fidl_fuchsia_hardware_inlineencryption::DeviceMarker as InlineEncryptionDeviceMarker;
use fidl_fuchsia_io as fio;
use starnix_crypt::CryptService;
use starnix_logging::{Level, log, log_error};
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error, from_status_like_fdio, statfs};
use std::sync::Arc;
use thiserror::Error;

const CRYPT_THREAD_ROLE: &str = "fuchsia.starnix.remotevol.crypt";
// `KEY_FILE_PATH` determines where the volume-wide keys for the Starnix volume will live in the
// container's data storage capability.
const KEY_FILE_PATH: &str = "key_file";

pub struct RemoteVolume {
    remotefs: RemoteFs,
    exposed_dir_proxy: fio::DirectorySynchronousProxy,
    crypt_service: Arc<CryptService>,
}

impl RemoteVolume {
    pub fn remotefs(&self) -> &RemoteFs {
        &self.remotefs
    }
}

impl FileSystemOps for RemoteVolume {
    fn statfs(&self, fs: &FileSystem, current_task: &CurrentTask) -> Result<statfs, Errno> {
        self.remotefs.statfs(fs, current_task)
    }

    fn name(&self) -> &'static FsStr {
        "remotevol".into()
    }

    fn uses_external_node_ids(&self) -> bool {
        self.remotefs.uses_external_node_ids()
    }

    fn has_casefold_support(&self) -> bool {
        self.remotefs.has_casefold_support()
    }

    fn rename(
        &self,
        fs: &FileSystem,
        current_task: &CurrentTask,
        context: &mut RenameContext<'_>,
        old_name: &FsStr,
        new_name: &FsStr,
    ) -> Result<(), Errno> {
        self.remotefs.rename(fs, current_task, context, old_name, new_name)
    }

    fn unmount(&self) {
        let (proxy, server_end) = create_sync_proxy::<fidl_fuchsia_fs::AdminMarker>();
        if let Err(e) = fdio::service_connect_at(
            self.exposed_dir_proxy.as_channel(),
            &format!("svc/{}", fidl_fuchsia_fs::AdminMarker::PROTOCOL_NAME),
            server_end.into(),
        ) {
            log_error!(e:%; "StarnixVolumeProvider.Unmount failed to connect to fuchsia.fs.Admin");
            return;
        }

        if let Err(e) = proxy.shutdown(zx::MonotonicInstant::INFINITE) {
            log_error!(e:%; "StarnixVolumeProvider.Unmount failed at FIDL layer");
        }
    }

    fn crypt_service(&self) -> Option<Arc<CryptService>> {
        Some(self.crypt_service.clone())
    }

    fn drop_caches(&self, _fs: &FileSystem) -> Result<(), Errno> {
        let (proxy, server_end) = create_sync_proxy::<DebugMarker>();
        if let Err(e) = fdio::service_connect_at(
            self.exposed_dir_proxy.as_channel(),
            &format!("svc/{}", DebugMarker::PROTOCOL_NAME),
            server_end.into(),
        ) {
            log_error!(e:%; "RemoteVolume.drop_caches failed to connect to fuchsia.fxfs.Debug");
            return error!(EIO);
        }

        match proxy.clear_caches(zx::MonotonicInstant::INFINITE) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(status)) => {
                let err = from_status_like_fdio!(zx::Status::err_from_raw(status));
                log_error!(err:?; "RemoteVolume.drop_caches failed");
                Err(err)
            }
            Err(e) => {
                log_error!(e:%; "RemoteVolume.drop_caches failed at FIDL layer");
                error!(EIO)
            }
        }
    }
}

// Key file
// ========
//
// Version 1: No longer supported.
// Version 2:
//
//   +-2-+------- 32 -------+------- 32 -------+
//   | V |   metadata key   |     data key     |
//   +---+------------------+------------------+
//
// Version 2 includes a 16 bit version which indicates the version of the key file.  The key
// identifiers used for version 2 key files will use the lblk32 algorithm.

struct VolumeKeys {
    metadata: [u8; 32],
    data: [u8; 32],
}

#[derive(Error, Debug, Eq, PartialEq)]
enum KeyFileError {
    #[error("key file not found")]
    NotFound,
    #[error("failed to read key file")]
    ReadError(#[from] zx::Status),
    #[error("unsupported key file version")]
    UnsupportedVersion,
    #[error("unexpected content")]
    UnexpectedContent,
}

impl VolumeKeys {
    // `KEYS_SIZE` is the size of the two keys (the metadata key, and the data key) stored in the
    // key file.
    const KEYS_SIZE: usize = 64;

    // Includes 2 bytes for the version.
    const FILE_SIZE: usize = 2 + Self::KEYS_SIZE;

    const LATEST_VERSION: u16 = 2;

    /// Returns (keys, did_create).
    fn get_or_create(
        data: &fio::DirectorySynchronousProxy,
        key_path: &str,
    ) -> Result<(Self, bool), Errno> {
        match Self::get(data, key_path) {
            Ok(keys) => Ok((keys, false)),
            Err(KeyFileError::ReadError(status)) => {
                // If there's a read error, we just return the error rather than try and create a
                // key file.  Chances are that if we are unable to read the file, we'll be unable to
                // create the file too.  A missing key file is handled differently.
                Err(from_status_like_fdio!(status))
            }
            Err(e) => {
                log!(
                    if e == KeyFileError::NotFound { Level::Info } else { Level::Warn },
                    "Creating key file at {key_path} (reason={e:?}) which will \
                     cause existing data to be *wiped* if it exists."
                );
                Ok((Self::create(data, key_path)?, true))
            }
        }
    }

    /// Returns None rather than an error if the key file does not exist or is corrupt,
    /// but returns all other errors (e.g. if the connection to `data` is closed).
    fn get(data: &fio::DirectorySynchronousProxy, key_path: &str) -> Result<Self, KeyFileError> {
        match syncio::directory_read_file(data, key_path, zx::MonotonicInstant::INFINITE) {
            Ok(bytes) => {
                if bytes.len() == Self::FILE_SIZE {
                    if u16::from_le_bytes(bytes[0..2].try_into().unwrap()) != Self::LATEST_VERSION {
                        Err(KeyFileError::UnsupportedVersion)
                    } else {
                        Ok(Self {
                            metadata: bytes[2..34].try_into().unwrap(),
                            data: bytes[34..66].try_into().unwrap(),
                        })
                    }
                } else {
                    Err(KeyFileError::UnexpectedContent)
                }
            }
            Err(zx::Status::NOT_FOUND) => {
                // This is expected after an FDR or clean install.
                Err(KeyFileError::NotFound)
            }
            Err(status) => {
                log_error!(status:?; "Failed to read key file");
                Err(status.into())
            }
        }
    }

    /// Creates a new key file at the latest version, with new random metadata and data keys.
    fn create(data: &fio::DirectorySynchronousProxy, key_path: &str) -> Result<Self, Errno> {
        let mut bytes = [0; Self::FILE_SIZE];
        bytes[..2].copy_from_slice(&Self::LATEST_VERSION.to_le_bytes());
        starnix_crypto::cprng_draw(&mut bytes[2..]);
        let tmp_file = syncio::directory_create_tmp_file(
            data,
            fio::PERM_READABLE,
            zx::MonotonicInstant::INFINITE,
        )
        .map_err(|e| {
            let err = from_status_like_fdio!(e);
            log_error!("Failed to create tmp file with error: {:?}", err);
            err
        })?;
        tmp_file
            .write(&bytes, zx::MonotonicInstant::INFINITE)
            .map_err(|e| {
                log_error!("FIDL transport error on File.Write {:?}", e);
                errno!(ENOENT)
            })?
            .map_err(|e| {
                let err = from_status_like_fdio!(zx::Status::err_from_raw(e));
                log_error!("File.Write failed with {:?}", err);
                err
            })?;
        tmp_file
            .sync(zx::MonotonicInstant::INFINITE)
            .map_err(|e| {
                log_error!("FIDL transport error on File.Sync {:?}", e);
                errno!(ENOENT)
            })?
            .map_err(|e| {
                let err = from_status_like_fdio!(zx::Status::err_from_raw(e));
                log_error!("File.Sync failed with {:?}", err);
                err
            })?;
        let (status, token) = data.get_token(zx::MonotonicInstant::INFINITE).map_err(|e| {
            log_error!("transport error on get_token for the data directory, error: {:?}", e);
            errno!(ENOENT)
        })?;
        zx::Status::ok(status).map_err(|e| {
            let err = from_status_like_fdio!(e);
            log_error!("Failed to get_token for the data directory, error: {:?}", err);
            err
        })?;

        tmp_file
            .link_into(
                zx::Event::from(token.ok_or_else(|| errno!(ENOENT))?),
                key_path,
                zx::MonotonicInstant::INFINITE,
            )
            .map_err(|e| {
                log_error!("FIDL transport error on File.LinkInto {:?}", e);
                errno!(EIO)
            })?
            .map_err(|e| {
                let err = from_status_like_fdio!(zx::Status::err_from_raw(e));
                log_error!("File.LinkInto failed with {:?}", err);
                err
            })?;
        Ok(Self {
            metadata: bytes[2..34].try_into().unwrap(),
            data: bytes[34..].try_into().unwrap(),
        })
    }
}

pub fn new_remote_vol(
    current_task: &CurrentTask,
    options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    let kernel = current_task.kernel();
    // TODO(https://fxbug.dev/460156877): Starnix cannot handle multiple volumes.
    let volume_provider = current_task
        .kernel()
        .connect_to_protocol_at_container_svc::<StarnixVolumeProviderMarker>()
        .map_err(|_| errno!(ENOENT))?
        .into_sync_proxy();

    // We need one crypt instance for fsck, the other for mounting.
    let (crypt_client_end, crypt_proxy) = fidl::endpoints::create_endpoints::<CryptMarker>();
    let (crypt2_client_end, crypt2_proxy) = fidl::endpoints::create_endpoints::<CryptMarker>();

    let key_location = match options.params.get(FsStr::new(b"keylocation")) {
        Some(path) => str::from_utf8(path.as_bytes()).map_err(|_| errno!(EINVAL))?,
        None => {
            // TODO(https://fxbug.dev/460156877): Starnix cannot handle unencrypted volumes.
            log_error!(
                "TODO(b/460156877): Starnix is unable to mount remote volumes without encryption. \
                Encrypted volumes should specify a keylocation in the mount flags."
            );
            return Err(errno!(EINVAL));
        }
    };

    let open_flags =
        fio::PERM_READABLE | fio::Flags::FLAG_MAYBE_CREATE | fio::Flags::PROTOCOL_DIRECTORY;
    let (root, subdir) = kernel.open_ns_dir(key_location, open_flags)?;

    let open_rights = fio::PERM_READABLE | fio::PERM_WRITABLE;
    let subdir = if subdir.is_empty() { ".".to_string() } else { subdir };
    let key_location_proxy = syncio::directory_open_directory_async(&root, &subdir, open_rights)
        .map_err(|e| errno!(EIO, format!("Failed to open proxy for keylocation: {e}")))?;

    let (keys, created_key_file) = VolumeKeys::get_or_create(&key_location_proxy, KEY_FILE_PATH)?;

    // Attempt to connect to the inline encryption device if mount options specify inline crypt
    let inline_encryption_provider = if options.params.get(FsStr::new(b"inlinecrypt")).is_some() {
        match current_task
            .kernel()
            .connect_to_protocol_at_container_svc::<InlineEncryptionDeviceMarker>()
        {
            Ok(client_end) => Some(client_end.into_sync_proxy()),
            Err(error) => {
                log_error!(error:?; "Error connecting to inline encryption device");
                return Err(error);
            }
        }
    } else {
        None
    };
    let crypt_service =
        Arc::new(CryptService::new(&keys.metadata, &keys.data, inline_encryption_provider));

    let (exposed_dir_client_end, exposed_dir_server) =
        fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();

    let crypt_service_clone = Arc::clone(&crypt_service);
    let closure = async move |_: &CurrentTask| {
        let (r1, r2) = futures::join!(
            crypt_service_clone.handle_connection(crypt_proxy.into_stream()),
            crypt_service_clone.handle_connection(crypt2_proxy.into_stream())
        );
        match (r1, r2) {
            (Err(e1), Err(e2)) => {
                log_error!("Error while handling Crypt connections: {e1:?}, {e2:?}")
            }
            (Err(e), Ok(())) => log_error!("Error while handling Crypt connection: {e:?}"),
            (Ok(()), Err(e)) => log_error!("Error while handling Crypt2 connection: {e:?}"),
            (Ok(()), Ok(())) => {}
        }
    };
    let req = SpawnRequestBuilder::new()
        .with_debug_name("remote-volume-crypt")
        .with_role(CRYPT_THREAD_ROLE)
        .with_async_closure(closure)
        .build();
    kernel.kthreads.spawner().spawn_from_request(req);

    let mode = if created_key_file {
        fidl_fuchsia_fshost::MountMode::AlwaysCreate
    } else {
        // If fsck fails, return an error.  This might trigger a reboot, which is OK -- the device
        // should eventually land back into recovery mode.
        volume_provider
            .check(crypt2_client_end, zx::MonotonicInstant::INFINITE)
            .map_err(|e| {
                log_error!("FIDL transport error on StarnixVolumeProvider.Check {:?}", e);
                errno!(ENOENT)
            })?
            .map_err(|e| {
                let error = from_status_like_fdio!(zx::Status::err_from_raw(e));
                log_error!(
                    error:?;
                    "Volume check failed. The filesystem might be corrupt!");
                error
            })?;
        fidl_fuchsia_fshost::MountMode::MaybeCreate
    };
    let guid = volume_provider
        .mount(crypt_client_end, mode, exposed_dir_server, zx::MonotonicInstant::INFINITE)
        .map_err(|e| {
            log_error!("FIDL transport error on StarnixVolumeProvider.Mount {:?}", e);
            errno!(ENOENT)
        })?
        .map_err(|e| {
            let error = from_status_like_fdio!(zx::Status::err_from_raw(e));
            log_error!(error:?; "StarnixVolumeProvider.Mount failed");
            error
        })?;

    crypt_service.set_uuid(guid);

    let exposed_dir_proxy = exposed_dir_client_end.into_sync_proxy();

    let root = syncio::directory_open_directory_async(
        &exposed_dir_proxy,
        "root",
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    )
    .map_err(|e| errno!(EIO, format!("Failed to open root: {e}")))?;

    let rights = fio::PERM_READABLE | fio::PERM_WRITABLE;

    let (remotefs, root_node, info, node_id) =
        RemoteFs::new(root.into_channel(), rights, "remotevol")?;

    let use_remote_ids = remotefs.use_remote_ids();
    let remotevol = RemoteVolume { remotefs, exposed_dir_proxy, crypt_service };
    let fs =
        FileSystem::new(kernel, CacheMode::Cached(kernel.fs_cache_config()), remotevol, options)?;

    if use_remote_ids {
        fs.create_root_with_info(node_id, root_node, info);
    } else {
        let root_ino = fs.allocate_ino();
        fs.create_root_with_info(root_ino, root_node, info);
    }

    Ok(fs)
}
