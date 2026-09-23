// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ffi::CStr;
use std::future::Future;
use std::os::raw::c_char;
use std::pin::Pin;

use anyhow::Error;

use fuchsia_async::LocalExecutor;
use installer::{BootloaderType, InstallationPaths};
use recovery_util_block::BlockDevice;
use zx::Status;

// Converts a raw c-string into a Rust String, or None if the c-string was NULL.
//
// Making a String copies the data, but we do our work in a separate thread so we need a copy
// anyway to be able to move the string over.
fn to_string(c_str: *const c_char) -> Option<String> {
    if c_str.is_null() {
        return None;
    }

    // Safety: we've checked that the pointer is non-NULL and the C caller is required to meet the
    // remaining safety requirements given by `install_from_usb()`.
    unsafe { CStr::from_ptr(c_str) }.to_str().map(String::from).ok()
}

// Converts a Rust error to Zircon Status.
// We don't own anyhow::Error or Status so can't directly implement From<> or Into<>.
fn to_status(error: anyhow::Error) -> Status {
    // We can't easily convert an arbitrary string into a meaningful Zircon error code, so
    // we log the string for debugging and just report an internal error.
    log::warn!("{error}");
    Status::INTERNAL
}

// We can't currently auto-generate with `cbindgen` during build, so add lint checks as a reminder
// to re-generate the C bindings if this API changes. See README for details.
// LINT.IfChange
/// Installs images from a source disk to a destination disk.
///
/// This function can auto-detect the install source and destination if there is exactly one viable
/// candidate for each, otherwise they must be supplied by the caller.
///
/// # Arguments
/// `source`: UTF-8 source block device topological path, or NULL to auto-detect a removable disk.
/// `destination`: UTF-8 destination block device topological path, or NULL to auto-detect internal
///                storage.
///
/// # Returns
/// A zx_status code.
///
/// # Safety
/// The string arguments must either be NULL or meet all the conditions given at
/// https://doc.rust-lang.org/std/ffi/struct.CStr.html, primarily:
///   1. The string must be null-terminated
///   2. The contents must not be modified until this function returns
#[unsafe(no_mangle)]
pub extern "C" fn install_from_usb(source: *const c_char, destination: *const c_char) -> i32 {
    // Include the function signature in the lint check, but not implementation, which can change
    // without affecting the C bindings.
    // LINT.ThenChange(../ffi_c/bindings.h)

    // This function just handles C/Rust conversion and async execution so the internals can be pure
    // async Rust.
    log::trace!("Starting install_from_usb()");

    // To handle async, this code spins up a separate thread with a new LocalExecutor. There may be
    // a better way to do this, but these other methods failed:
    //   1. New LocalExecutor on this thread - runtime panic, LocalExecutors are per-thread
    //      singletons and some components (in particular fastboot-tcp) may have already created
    //      one on this thread.
    //   2. futures::executor::block_on() - runtime deadlock, this appears to be able to handle
    //      a single async call but the installer library runs concurrent async via
    //      futures::future::try_join_all() which deadlocks.
    let source = to_string(source);
    let destination = to_string(destination);
    let func = move || {
        LocalExecutor::default().run_singlethreaded(install_from_usb_internal(
            source,
            destination,
            &Dependencies::default(),
        ))
    };
    let thread_result = std::thread::spawn(func).join();
    log::trace!("install_from_usb() result = {thread_result:?}");

    match thread_result {
        Ok(result) => Status::result_into_raw(result),
        Err(thread_panic) => {
            log::error!("install_from_usb thread panic: {thread_panic:?}");
            Status::INTERNAL.into_raw()
        }
    }
}

// Dependency injection for testing.
//
// Unfortunately there doesn't seem to be a super easy way to do this:
//   * mockall doesn't work well for free functions and has lifetime complications
//   * traits can't define async functions so we can't have a trait wrapper
// So we just pass around a struct of function pointers, which due to `sync` requires some ugly
// pin/box boilerplate, lifetime management, and indirection.
//
// TODO: I think we can greatly simplify this by wrapping each function in a sync -> async
// wrapper individually. Maybe less efficient but we don't need the async functionality here
// and it would allow us to mock out sync functions instead which is far easier.
type BoxedFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

struct Dependencies {
    do_install: Box<dyn Fn(InstallationPaths) -> BoxedFuture<'static, Result<(), Error>>>,
    find_install_source: Box<
        dyn for<'a> Fn(
            &'a Vec<BlockDevice>,
            BootloaderType,
        ) -> BoxedFuture<'a, Result<&'a BlockDevice, Error>>,
    >,
    get_block_devices: Box<dyn Fn() -> BoxedFuture<'static, Result<Vec<BlockDevice>, Error>>>,
}

impl Dependencies {
    // Returns the actual implementations.
    fn default() -> Self {
        Self {
            // How do we pass the callback to `do_install()`? For now we don't need it so just pass
            // a no-op closure, but I can't get the compiler to pass a real closure.
            do_install: Box::new(move |a| Box::pin(installer::do_install(a, &|_| {}))),
            find_install_source: Box::new(move |a, b| {
                Box::pin(installer::find_install_source(a, b))
            }),
            get_block_devices: Box::new(move || Box::pin(recovery_util_block::get_block_devices())),
        }
    }
}

// Internal Rust entry point.
async fn install_from_usb_internal(
    source: Option<String>,
    destination: Option<String>,
    dependencies: &Dependencies,
) -> Result<(), Status> {
    log::trace!(
        "Starting install_from_usb_internal(), source = {source:?}, dest = {destination:?}"
    );

    let installation_paths =
        get_installation_paths(source.as_deref(), destination.as_deref(), dependencies).await?;
    log::trace!("Installation paths: {installation_paths:?}");

    (dependencies.do_install)(installation_paths).await.map_err(to_status)
}

// Finds the source and target to install.
async fn get_installation_paths(
    requested_source: Option<&str>,
    requested_destination: Option<&str>,
    dependencies: &Dependencies,
) -> Result<InstallationPaths, Status> {
    // The installer library hardcodes some rules about what partitions are expected on a disk
    // depending on the bootloader (coreboot vs EFI). This is a bit brittle, we may want to look
    // into removing this dependency in the future.
    // For now we don't care about coreboot, just hardcode EFI.
    let bootloader_type = BootloaderType::Efi;

    log::trace!("Looking for block devices");
    let block_devices = (dependencies.get_block_devices)().await.map_err(to_status)?;
    log::trace!("Got block devices {block_devices:?}");

    let install_source = match requested_source {
        // If a particular block device was requested, use it (or error out if not found).
        Some(device_path) => block_devices
            .iter()
            .find(|d| d.is_disk() && d.topo_path == device_path)
            .ok_or(Status::NOT_FOUND)?,
        // Otherwise, try to auto-detect the removable disk (e.g. USB).
        None => (dependencies.find_install_source)(&block_devices, bootloader_type)
            .await
            .map_err(to_status)?,
    };

    let install_filter = |d: &&BlockDevice| {
        d.is_disk()
            && match requested_destination {
                // If a particular block device was requested, use it.
                Some(device_path) => d.topo_path == device_path,
                // Otherwise use the disk that isn't our source.
                None => *d != install_source,
            }
    };
    let mut install_iter = block_devices.iter().filter(install_filter);
    let install_target = install_iter.next().ok_or(Status::NOT_FOUND)?;
    // Don't install if there could have been multiple targets, since it's ambiguous which one
    // the caller wants. They must provide a `requested_destination` in this case.
    if install_iter.next().is_some() {
        return Err(Status::INVALID_ARGS);
    }

    let paths = InstallationPaths {
        install_source: Some(install_source.clone()),
        install_target: Some(install_target.clone()),
        bootloader_type: Some(bootloader_type),
        // I don't think this is currently used - see if we can delete it.
        install_destinations: Vec::new(),
        available_disks: block_devices,
    };
    log::trace!("Found installation paths: {paths:?}");

    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    use anyhow::anyhow;
    use std::ffi::CString;
    use std::ptr::null;

    impl Dependencies {
        // Returns the dependency test implementations.
        fn test() -> Self {
            Self {
                do_install: Box::new(move |_| Box::pin(async { Ok(()) })),
                find_install_source: Box::new(move |a, b| Box::pin(fake_find_install_source(a, b))),
                get_block_devices: Box::new(move || Box::pin(fake_get_block_devices())),
            }
        }

        // Returns the dependency test implementations that shows 3 disks.
        fn test_3_disks() -> Self {
            let mut deps = Self::test();
            deps.get_block_devices = Box::new(move || Box::pin(fake_get_block_devices_3_disks()));
            deps
        }
    }

    // A fake set of block devices to test against.
    async fn fake_get_block_devices() -> Result<Vec<BlockDevice>, Error> {
        Ok(vec![
            // 2 disks (disks do not contain "/block/part-" in topo_path).
            BlockDevice {
                topo_path: String::from("/dev/sys/platform/foo/block"),
                class_path: String::from(""),
                size: 0,
            },
            BlockDevice {
                topo_path: String::from("/dev/sys/platform/bar/block"),
                class_path: String::from(""),
                size: 0,
            },
            // A handful of partitions on the disks.
            BlockDevice {
                topo_path: String::from("/dev/sys/platform/foo/block/part-000"),
                class_path: String::from(""),
                size: 0,
            },
            BlockDevice {
                topo_path: String::from("/dev/sys/platform/foo/block/part-001"),
                class_path: String::from(""),
                size: 0,
            },
            BlockDevice {
                topo_path: String::from("/dev/sys/platform/bar/block/part-000"),
                class_path: String::from(""),
                size: 0,
            },
            BlockDevice {
                topo_path: String::from("/dev/sys/platform/bar/block/part-001"),
                class_path: String::from(""),
                size: 0,
            },
        ])
    }

    // A fake set of block devices that contains more than 3 so we can't auto-detect the install
    // target.
    async fn fake_get_block_devices_3_disks() -> Result<Vec<BlockDevice>, Error> {
        let mut devices = fake_get_block_devices().await.unwrap();
        devices.append(&mut vec![
            BlockDevice {
                topo_path: String::from("/dev/sys/platform/baz/block"),
                class_path: String::from(""),
                size: 0,
            },
            BlockDevice {
                topo_path: String::from("/dev/sys/platform/baz/block/part-000"),
                class_path: String::from(""),
                size: 0,
            },
            BlockDevice {
                topo_path: String::from("/dev/sys/platform/baz/block/part-001"),
                class_path: String::from(""),
                size: 0,
            },
        ]);
        Ok(devices)
    }

    // Returns the first found block device. The real implementation checks to see if the disk has
    // partitions with the expected installer GUID, but there's no point replicating that here.
    async fn fake_find_install_source(
        block_devices: &Vec<BlockDevice>,
        _: BootloaderType,
    ) -> Result<&BlockDevice, Error> {
        Ok(&block_devices[0])
    }

    #[fuchsia::test]
    fn test_to_string() {
        let c_string = CString::new("test string").unwrap();
        assert_eq!(to_string(c_string.as_ptr()).unwrap(), "test string");
    }

    #[fuchsia::test]
    fn test_to_string_null() {
        assert!(to_string(null()).is_none());
    }

    #[fuchsia::test]
    fn test_to_status() {
        assert_eq!(to_status(anyhow!("expected test error")), Status::INTERNAL);
    }

    #[fuchsia::test]
    async fn test_get_installation_paths() {
        let deps = Dependencies::test();
        let fake_devices = (deps.get_block_devices)().await.unwrap();

        let paths = get_installation_paths(None, None, &deps).await.unwrap();
        assert_eq!(
            paths,
            InstallationPaths {
                install_source: Some(fake_devices[0].clone()),
                // Target should be the non-source disk.
                install_target: Some(fake_devices[1].clone()),
                bootloader_type: Some(BootloaderType::Efi),
                install_destinations: Vec::new(),
                available_disks: fake_devices,
            }
        );
    }

    #[fuchsia::test]
    async fn test_get_installation_paths_request_source() {
        let deps = Dependencies::test();
        let fake_devices = (deps.get_block_devices)().await.unwrap();

        // Request the 2nd disk as the install source.
        let paths =
            get_installation_paths(Some(&fake_devices[1].topo_path), None, &deps).await.unwrap();
        assert_eq!(
            paths,
            InstallationPaths {
                install_source: Some(fake_devices[1].clone()),
                install_target: Some(fake_devices[0].clone()),
                bootloader_type: Some(BootloaderType::Efi),
                install_destinations: Vec::new(),
                available_disks: fake_devices,
            }
        );
    }

    #[fuchsia::test]
    async fn test_get_installation_paths_request_target() {
        let deps = Dependencies::test();
        let fake_devices = (deps.get_block_devices)().await.unwrap();

        // Request the 2nd disk as the install target.
        let paths =
            get_installation_paths(None, Some(&fake_devices[1].topo_path), &deps).await.unwrap();
        assert_eq!(
            paths,
            InstallationPaths {
                install_source: Some(fake_devices[0].clone()),
                install_target: Some(fake_devices[1].clone()),
                bootloader_type: Some(BootloaderType::Efi),
                install_destinations: Vec::new(),
                available_disks: fake_devices,
            }
        );
    }

    #[fuchsia::test]
    async fn test_get_installation_paths_request_both() {
        let deps = Dependencies::test_3_disks();
        let fake_devices = (deps.get_block_devices)().await.unwrap();

        let paths = get_installation_paths(
            Some(&fake_devices[1].topo_path),
            // device[6] is the 3rd disk "baz" in our fake disks list.
            Some(&fake_devices[6].topo_path),
            &deps,
        )
        .await
        .unwrap();

        assert_eq!(
            paths,
            InstallationPaths {
                install_source: Some(fake_devices[1].clone()),
                install_target: Some(fake_devices[6].clone()),
                bootloader_type: Some(BootloaderType::Efi),
                install_destinations: Vec::new(),
                available_disks: fake_devices,
            }
        );
    }

    #[fuchsia::test]
    async fn test_get_installation_paths_ambiguous_target() {
        let deps = Dependencies::test_3_disks();
        let fake_devices = (deps.get_block_devices)().await.unwrap();

        // With 3 disks we should error out because we can't determine which target to use.
        assert_eq!(
            get_installation_paths(Some(&fake_devices[0].topo_path), None, &deps,).await,
            Err(Status::INVALID_ARGS)
        );
    }

    #[fuchsia::test]
    async fn test_install_from_usb() {
        let deps = Dependencies::test();

        assert!(install_from_usb_internal(None, None, &deps).await.is_ok());
    }

    #[fuchsia::test]
    fn test_install_from_usb_fail_sync() {
        // Test the top-level API in a sync context.
        // We expect it to fail, this is primarily to ensure our async calls work.
        let source = CString::new("foo").unwrap();
        let dest = CString::new("bar").unwrap();
        assert!(Status::ok(install_from_usb(source.as_ptr(), dest.as_ptr())).is_err());
    }

    #[fuchsia::test]
    async fn test_install_from_usb_fail_async() {
        // Test the top-level API in an async context.
        // We expect it to fail, this is primarily to ensure our async calls work.
        let source = CString::new("foo").unwrap();
        let dest = CString::new("bar").unwrap();
        assert!(Status::ok(install_from_usb(source.as_ptr(), dest.as_ptr())).is_err());
    }
}
