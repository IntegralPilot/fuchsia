// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::{FromArgs, from_env};
use fidl::endpoints::{ServerEnd, create_endpoints, create_proxy};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_mem::Buffer;
use fidl_fuchsia_pkg::{BlobId, PackageCacheMarker, PackageResolverMarker, PackageUrl};
use fidl_fuchsia_sys2::{StorageAdminMarker, StorageIteratorMarker};
use fidl_fuchsia_update_installer::{
    Initiator, InstallerMarker, MonitorMarker, MonitorRequest, Options, RebootControllerMarker,
    State,
};
use fidl_test_security_pkg::PackageServer_Marker;
use fuchsia_async::Task;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_fs::directory::{open_file, readdir};
use fuchsia_fs::file;
use fuchsia_hash::Hash;
use futures::channel::oneshot::channel;
use futures::{TryStreamExt, join};
use log::info;
use security_pkg_test_util::config::load_config;
use security_pkg_test_util::storage::mount_image_as_ramdisk;
use std::fs::File;
use zx::{Rights, Status};

const DEFAULT_DOMAIN: &str = "fuchsia.com";
const HELLO_WORLD_PACKAGE_NAME: &str = "hello_world";
const HELLO_WORLD_V0_PACKAGED_BINARY_PATH: &str = "bin/hello_world_v0";
const HELLO_WORLD_V1_PACKAGED_BINARY_PATH: &str = "bin/hello_world_v1";

/// Flags for access_ota_blob_as_executable.
#[derive(FromArgs, Debug, PartialEq)]
pub struct Args {
    /// absolute path to hello world v0 meta.far file used for designating its
    /// merkle root hash.
    #[argh(option)]
    hello_world_v0_meta_far_path: String,
    /// absolute path to hello world v1 meta.far file used for designating its
    /// merkle root hash.
    #[argh(option)]
    hello_world_v1_meta_far_path: String,
    /// absolute path to v1 update package (update.far) file used for
    /// designating its merkle root hash.
    #[argh(option)]
    v1_update_far_path: String,
    /// absolute path to shared test configuration file understood by
    /// security_pkg_test_util::load_config().
    #[argh(option)]
    test_config_path: String,

    /// switch used by rust test runner.
    #[argh(switch)]
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    nocapture: bool,
}

struct ReadableExecutableResult {
    /// Status of attempt to open-as-readable and read.
    pub readable: Result<()>,
    /// Status of attempt to open-as-executable and read.
    pub executable: Result<()>,
}

impl ReadableExecutableResult {
    /// Signals whether both readable and executable results are ok.
    pub fn is_readable_executable_ok(&self) -> bool {
        self.readable.is_ok() && self.executable.is_ok()
    }

    /// Signals whether only executable result is error.
    pub fn is_executable_err(&self) -> bool {
        self.readable.is_ok() && self.executable.is_err()
    }
}

// Result of attempting to open executable in package several different ways.
struct AccessCheckResult {
    /// Result of opening via fuchsia.pkg/PackageCache.Get API.
    pub pkg_cache_get: Option<ReadableExecutableResult>,
    /// Result of opening via package resolver API with
    /// fuchsia-pkg://[domain]/[package]/0?hash=[hash].
    pub pkg_resolver_with_hash: Option<ReadableExecutableResult>,
    /// Result of opening via package resolver API with
    /// fuchsia-pkg://[domain]/[package]/0.
    pub pkg_resolver_without_hash: Option<ReadableExecutableResult>,
}

struct AccessCheckSelectors {
    /// Perform access check against pkg-cache's fuchsia.pkg/PackageCache.Get API.
    pub pkg_cache_get: bool,
    /// Perform access check against package resolver API with
    /// fuchsia-pkg://[domain]/[package]/0?hash=[hash].
    pub pkg_resolver_with_hash: bool,
    /// Perform access check against package resolver API with
    /// fuchsia-pkg://[domain]/[package]/0.
    pub pkg_resolver_without_hash: bool,
}

impl AccessCheckSelectors {
    /// Enable all access checks.
    pub fn all() -> Self {
        Self { pkg_cache_get: true, pkg_resolver_with_hash: true, pkg_resolver_without_hash: true }
    }
}

// Request to check access control against an executable in a package.
struct AccessCheckConfig {
    /// The name of the package containing the executable.
    pub package_name: String,
    /// The domain name to use for resolving the package with a hash.
    pub domain_with_hash: String,
    /// The domain name to use for resolving the package without a hash.
    pub domain_without_hash: String,
    /// The test-package-local path for side-loading the package. This mechanism
    /// is used to compute the package hash.
    pub local_package_path: String,
    /// The resolved-package-local path for opening the executable.
    pub packaged_binary_path: String,
}

struct AccessCheckRequest {
    /// Configuration for executing access checks.
    config: AccessCheckConfig,
    /// Selection of specific access checks/APIs to execute.
    selectors: AccessCheckSelectors,
}

impl AccessCheckRequest {
    /// Execute the access checks encoded in this request.
    pub async fn perform_access_check(&self) -> AccessCheckResult {
        // Determine the hash (the merkle root of the package meta.far) of the package.
        let package = File::open(&self.config.local_package_path).unwrap();
        let package_merkle = fuchsia_merkle::root_from_reader(package).unwrap();
        let package_blob_id = BlobId { merkle_root: package_merkle.into() };

        // Open package as executable via pkg-cache's fuchsia.pkg/PackageCache.Get API.
        let pkg_cache_get_rx_result = if self.selectors.pkg_cache_get {
            info!(package_merkle:%; "Opening package via fuchsia.pkg/PackageCache.Get");
            // In all of the uses of this check in these tests, the package's blobs are already
            // present in blobfs.
            // If this stops being the case, get_already_cached will fail (notifying a developer to
            // update this test).
            let pkg_cache_proxy = connect_to_protocol::<PackageCacheMarker>().unwrap();

            let pkg_cache = fidl_fuchsia_pkg_ext::cache::Client::from_proxy(pkg_cache_proxy);
            let package_directory = pkg_cache
                .get_already_cached(package_blob_id.try_into().unwrap())
                .await
                .expect("package should already be cached");
            let package_directory_proxy = package_directory.into_proxy();

            Some((
                self.attempt_readable(&package_directory_proxy).await,
                self.attempt_executable(&package_directory_proxy).await,
            ))
        } else {
            info!(
                package_merkle:%;
                "Skipping open package via fuchsia.pkg/PackageCache.Get"
            );
            None
        };

        // Resolve package as executable via pkg-resolver API.
        let url_with_hash = format!(
            "fuchsia-pkg://{}/{}/0?hash={}",
            &self.config.domain_with_hash, &self.config.package_name, &package_merkle
        );
        let pkg_resolver_with_hash_rx_result = if self.selectors.pkg_resolver_with_hash {
            info!(url_with_hash:%; "Opening package via pkg-resolver");
            let package_directory_proxy = self.resolve_package(&url_with_hash).await;
            Some((
                self.attempt_readable(&package_directory_proxy).await,
                self.attempt_executable(&package_directory_proxy).await,
            ))
        } else {
            info!(url_with_hash:%; "Skipping open package via pkg-resolver");
            None
        };

        let url_without_hash = format!(
            "fuchsia-pkg://{}/{}/0",
            &self.config.domain_without_hash, &self.config.package_name
        );
        let pkg_resolver_without_hash_rx_result = if self.selectors.pkg_resolver_without_hash {
            info!(url_without_hash:%; "Opening package via pkg-resolver");
            let package_directory_proxy = self.resolve_package(&url_without_hash).await;
            Some((
                self.attempt_readable(&package_directory_proxy).await,
                self.attempt_executable(&package_directory_proxy).await,
            ))
        } else {
            info!(url_without_hash:%; "Skipping open package via pkg-resolver");
            None
        };

        // Check that all opened-as-executable buffers contain the same data.
        let buffers = vec![
            // ..._rx_result.1 contains Result<Box<Buffer>>.
            pkg_cache_get_rx_result.as_ref().map(|rx| &rx.1),
            pkg_resolver_with_hash_rx_result.as_ref().map(|rx| &rx.1),
        ]
        .into_iter()
        .filter_map(|opt_rx| opt_rx) // Drop `None` (configured not to run).
        .map(Result::as_ref) // Do not consume values inside results.
        .filter_map(Result::ok) // Extract buffers from results.
        .collect();
        Self::check_buffer_consistency(&buffers);

        AccessCheckResult {
            pkg_cache_get: Self::pair_to_result(pkg_cache_get_rx_result),
            pkg_resolver_with_hash: Self::pair_to_result(pkg_resolver_with_hash_rx_result),
            pkg_resolver_without_hash: Self::pair_to_result(pkg_resolver_without_hash_rx_result),
        }
    }

    async fn resolve_package(&self, package_url: &str) -> fio::DirectoryProxy {
        let (package_directory_proxy, package_directory_server_end) =
            create_proxy::<fio::DirectoryMarker>();
        connect_to_protocol::<PackageResolverMarker>()
            .unwrap()
            .resolve(package_url, package_directory_server_end)
            .await
            .unwrap()
            .unwrap();
        package_directory_proxy
    }

    fn check_buffer_consistency(buffers: &Vec<&Box<Buffer>>) {
        if buffers.len() > 1 {
            info!("Checking consistency of {} buffers", buffers.len());
            let buffer1 = buffers[0];
            for i in 1..buffers.len() {
                let buffer2 = buffers[i];
                assert_eq!(buffer1.size, buffer2.size);
                let mut buffer1_vec = vec![0; buffer1.size.try_into().unwrap()];
                let buffer1_data = buffer1_vec.as_mut_slice();
                buffer1.vmo.read(buffer1_data, 0).unwrap();
                let mut buffer2_vec = vec![0; buffer2.size.try_into().unwrap()];
                let buffer2_data = buffer2_vec.as_mut_slice();
                buffer2.vmo.read(buffer2_data, 0).unwrap();
                assert_eq!(buffer1_data, buffer2_data);
            }
        } else {
            info!("Skipping buffer consistency check: No buffers");
        }
    }

    async fn attempt_executable(
        &self,
        package_directory_proxy: &fio::DirectoryProxy,
    ) -> Result<Box<Buffer>> {
        let bin_file = open_file(
            package_directory_proxy,
            &self.config.packaged_binary_path,
            fio::PERM_READABLE | fio::PERM_EXECUTABLE,
        )
        .await?;
        let vmo = bin_file
            .get_backing_memory(fio::VmoFlags::READ | fio::VmoFlags::EXECUTE)
            .await
            .unwrap()
            .map_err(Status::err_from_raw)?;
        let bin_info = vmo.basic_info().unwrap();
        assert_eq!(bin_info.rights & Rights::READ, Rights::READ);
        assert_eq!(bin_info.rights & Rights::EXECUTE, Rights::EXECUTE);
        let size = vmo.get_content_size()?;
        Ok(Box::new(Buffer { vmo, size }))
    }

    async fn attempt_readable(&self, package_directory_proxy: &fio::DirectoryProxy) -> Result<()> {
        let bin_file = open_file(
            package_directory_proxy,
            &self.config.packaged_binary_path,
            fio::PERM_READABLE,
        )
        .await?;
        file::read(&bin_file).await.unwrap();
        Ok(())
    }

    fn pair_to_result(
        pair: Option<(Result<()>, Result<Box<Buffer>>)>,
    ) -> Option<ReadableExecutableResult> {
        pair.map(|rx| ReadableExecutableResult { readable: rx.0, executable: rx.1.map(|_| ()) })
    }
}

async fn get_storage_for_component_instance(moniker_prefix: &str) -> fio::DirectoryProxy {
    let storage_admin = connect_to_protocol::<StorageAdminMarker>().unwrap();
    let (storage_user_iterator, storage_user_iterator_server_end) =
        create_proxy::<StorageIteratorMarker>();
    storage_admin
        .list_storage_in_realm(".", storage_user_iterator_server_end)
        .await
        .unwrap()
        .unwrap();
    let mut matching_storage_users = vec![];
    loop {
        let chunk = storage_user_iterator.next().await.unwrap();
        if chunk.is_empty() {
            break;
        }
        let mut matches: Vec<String> =
            chunk.into_iter().filter(|moniker| moniker.starts_with(moniker_prefix)).collect();
        matching_storage_users.append(&mut matches);
    }
    assert_eq!(1, matching_storage_users.len());
    let (proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
    storage_admin
        .open_storage(
            matching_storage_users.first().unwrap(),
            ServerEnd::new(server_end.into_channel()),
        )
        .await
        .unwrap()
        .unwrap();
    proxy
}

async fn get_local_package_server_url() -> String {
    connect_to_protocol::<PackageServer_Marker>().unwrap().get_url().await.unwrap()
}

async fn get_hello_world_v1_update_merkle(v1_update_far_path: String) -> Hash {
    let (sender, receiver) = channel::<Hash>();
    Task::local(async move {
        let hello_world_v1_update = File::open(&v1_update_far_path).unwrap();
        let hello_world_v1_update_merkle =
            fuchsia_merkle::root_from_reader(hello_world_v1_update).unwrap();
        sender.send(hello_world_v1_update_merkle).unwrap();
    })
    .detach();
    receiver.await.unwrap()
}

async fn perform_update(update_url: &str) {
    let installer_proxy = connect_to_protocol::<InstallerMarker>().unwrap();
    let (monitor_client_end, monitor_server_end) = create_endpoints::<MonitorMarker>();

    // Prevent reboot attempt by signalling that the client (this code) will
    // manage reboot via the provided RebootController.
    let (_reboot_controller_proxy, reboot_controller_server_end) =
        create_proxy::<RebootControllerMarker>();
    installer_proxy
        .start_update(
            &PackageUrl { url: update_url.to_string() },
            &Options {
                initiator: Some(Initiator::Service),
                allow_attach_to_existing_attempt: Some(false),
                should_write_recovery: Some(false),
                ..Default::default()
            },
            monitor_client_end,
            Some(reboot_controller_server_end),
        )
        .await
        .unwrap()
        .unwrap();

    let mut monitor_stream = monitor_server_end.into_stream();
    while let Some(request) = monitor_stream.try_next().await.unwrap() {
        match request {
            MonitorRequest::OnState { state, responder } => {
                info!("Update state change: {:#?}", state);
                responder.send().unwrap();
                match state {
                    State::WaitToReboot(_) => {
                        break;
                    }
                    State::Reboot(_)
                    | State::DeferReboot(_)
                    | State::Complete(_)
                    | State::FailPrepare(_)
                    | State::FailFetch(_) => {
                        panic!("Update entered unexpected terminal state: {:#?}", state);
                    }
                    _ => {}
                }
            }
        }
    }
}

#[fuchsia::test]
async fn access_ota_blob_as_executable() {
    info!("Starting access_ota_blob_as_executable test");
    let args @ Args {
        hello_world_v0_meta_far_path,
        hello_world_v1_meta_far_path,
        v1_update_far_path,
        test_config_path,
        ..
    } = &from_env();
    info!(args:?; "Initalizing access_ota_blob_as_executable");

    // Load test environment configuration.
    let config = load_config(test_config_path);

    // Setup storage capabilities.
    let _ramdisk_client =
        mount_image_as_ramdisk("/pkg/data/assemblies/hello_world_v0/fs.blk").await;
    let pkg_resolver_storage_proxy = get_storage_for_component_instance("pkg-resolver").await;
    // TODO(https://fxbug.dev/42169686): Need a test that confirms assumption: Production
    // configuration is an empty mutable storage directory.
    assert!(readdir(&pkg_resolver_storage_proxy).await.unwrap().is_empty());

    info!("Gathering data and connecting to package server");

    let (
        hello_world_v0_access_check,
        hello_world_v1_access_check,
        different_package_name_access_check,
    ) = (
        // Access check against base version of package: Should succeed before
        // and after update.
        AccessCheckRequest {
            config: AccessCheckConfig {
                package_name: HELLO_WORLD_PACKAGE_NAME.to_string(),
                domain_with_hash: config.update_domain.clone(),
                domain_without_hash: DEFAULT_DOMAIN.to_string(),
                local_package_path: hello_world_v0_meta_far_path.to_string(),
                packaged_binary_path: HELLO_WORLD_V0_PACKAGED_BINARY_PATH.to_string(),
            },
            selectors: AccessCheckSelectors::all(),
        },
        // Access check against updated version of package: All hash-qualified
        // checks should fail.
        AccessCheckRequest {
            config: AccessCheckConfig {
                package_name: HELLO_WORLD_PACKAGE_NAME.to_string(),
                domain_with_hash: config.update_domain.clone(),
                domain_without_hash: DEFAULT_DOMAIN.to_string(),
                local_package_path: hello_world_v1_meta_far_path.to_string(),
                packaged_binary_path: HELLO_WORLD_V1_PACKAGED_BINARY_PATH.to_string(),
            },
            selectors: AccessCheckSelectors {
                pkg_cache_get: true,
                pkg_resolver_with_hash: true,

                // Disable non-hash-qualified checks.
                pkg_resolver_without_hash: false,
            },
        },
        // Access check for referring to package by a different name (but using
        // a hash to load the correct executable).
        AccessCheckRequest {
            config: AccessCheckConfig {
                package_name: "system_image".to_string(),
                domain_with_hash: config.update_domain.clone(),
                domain_without_hash: DEFAULT_DOMAIN.to_string(),
                local_package_path: hello_world_v1_meta_far_path.to_string(),
                packaged_binary_path: HELLO_WORLD_V1_PACKAGED_BINARY_PATH.to_string(),
            },
            selectors: AccessCheckSelectors {
                pkg_resolver_with_hash: true,

                // Disable most checks; only interested in package resolution.
                pkg_cache_get: false,
                pkg_resolver_without_hash: false,
            },
        },
    );

    // Setup package server, which must be ready before access checks are performed.
    let package_server_url = get_local_package_server_url().await;

    // Get v1 update merkle and perform pre-update access check.
    let (hello_world_v0_access_check_result, update_merkle) = join!(
        hello_world_v0_access_check.perform_access_check(),
        get_hello_world_v1_update_merkle(v1_update_far_path.to_string())
    );

    // Pre-update base version access check: Access should always succeed.
    assert!(hello_world_v0_access_check_result.pkg_cache_get.unwrap().is_readable_executable_ok());
    assert!(
        hello_world_v0_access_check_result
            .pkg_resolver_with_hash
            .unwrap()
            .is_readable_executable_ok()
    );
    assert!(
        hello_world_v0_access_check_result
            .pkg_resolver_without_hash
            .unwrap()
            .is_readable_executable_ok()
    );

    info!("Package server running on {}", package_server_url);

    // Placeholder assertion for well-formed local URL. Test will eventually use
    // URL to configure network connection for `pkg-resolver`.
    assert!(package_server_url.starts_with("https://localhost"));

    let update_url =
        format!("fuchsia-pkg://{}/update/0?hash={}", config.update_domain, update_merkle);

    info!(update_url:%; "Initiating update");

    perform_update(&update_url).await;

    let hello_world_v1_access_check_result =
        hello_world_v1_access_check.perform_access_check().await;

    // Post-update new version access check: Access should fail on all
    // hash-qualified attempts to open as executable.
    assert!(hello_world_v1_access_check_result.pkg_cache_get.unwrap().is_executable_err());
    assert!(hello_world_v1_access_check_result.pkg_resolver_with_hash.unwrap().is_executable_err());

    let hello_world_v0_access_check_result =
        hello_world_v0_access_check.perform_access_check().await;

    // Post-update base version access check: Access should always succeed.
    assert!(hello_world_v0_access_check_result.pkg_cache_get.unwrap().is_readable_executable_ok());
    assert!(
        hello_world_v0_access_check_result
            .pkg_resolver_with_hash
            .unwrap()
            .is_readable_executable_ok()
    );
    assert!(
        hello_world_v0_access_check_result
            .pkg_resolver_without_hash
            .unwrap()
            .is_readable_executable_ok()
    );

    let different_package_name_access_check_result =
        different_package_name_access_check.perform_access_check().await;

    // Accessing via different package name: Should fail.
    assert!(
        different_package_name_access_check_result
            .pkg_resolver_with_hash
            .unwrap()
            .is_executable_err()
    );
}
