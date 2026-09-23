// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::DEFAULT_UPDATE_PACKAGE_URL;
use crate::errors::{self, Error};
use crate::update_manager::TargetChannelUpdater;
use anyhow::{Context as _, anyhow};
use fidl_fuchsia_mem as fmem;
use fidl_fuchsia_paver::{
    self as fpaver, Asset, BootManagerMarker, DataSinkMarker, PaverMarker, PaverProxy,
};
use fidl_fuchsia_pkg::{self as fpkg, PackageResolverMarker, PackageResolverProxyInterface};
use fidl_fuchsia_pkg_garbagecollector as fpkg_gc;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_hash::Hash;
use log::{error, info, warn};
use std::io;

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum SystemUpdateStatus {
    UpToDate { system_image: Hash, update_package: Hash },
    UpdateAvailable { current_system_image: Hash, latest_system_image: Hash },
}

pub async fn check_for_system_update(
    last_known_update_package: Option<&Hash>,
    target_channel_manager: &dyn TargetChannelUpdater,
) -> Result<SystemUpdateStatus, Error> {
    let mut file_system = RealFileSystem;
    let package_resolver =
        connect_to_protocol::<PackageResolverMarker>().map_err(Error::ConnectPackageResolver)?;
    let paver = connect_to_protocol::<PaverMarker>().map_err(Error::ConnectPaver)?;
    let space_manager =
        connect_to_protocol::<fpkg_gc::ManagerMarker>().map_err(Error::ConnectSpaceManager)?;

    check_for_system_update_impl(
        DEFAULT_UPDATE_PACKAGE_URL,
        &mut file_system,
        &package_resolver,
        &paver,
        target_channel_manager,
        last_known_update_package,
        &space_manager,
    )
    .await
}

// For mocking
trait FileSystem {
    fn read_to_string(&self, path: &str) -> io::Result<String>;
}

struct RealFileSystem;

impl FileSystem for RealFileSystem {
    fn read_to_string(&self, path: &str) -> io::Result<String> {
        std::fs::read_to_string(path)
    }
}

async fn check_for_system_update_impl(
    default_update_url: &str,
    file_system: &mut impl FileSystem,
    package_resolver: &impl PackageResolverProxyInterface,
    paver: &PaverProxy,
    target_channel_manager: &dyn TargetChannelUpdater,
    last_known_update_package: Option<&Hash>,
    space_manager: &fpkg_gc::ManagerProxy,
) -> Result<SystemUpdateStatus, Error> {
    let update_pkg = latest_update_package(
        default_update_url,
        package_resolver,
        target_channel_manager,
        space_manager,
    )
    .await?;
    let latest_update_merkle = update_pkg.hash().await.map_err(errors::UpdatePackage::Hash)?;
    let current_system_image = current_system_image_merkle(file_system)?;
    let latest_system_image = latest_system_image_merkle(&update_pkg).await?;

    let up_to_date = Ok(SystemUpdateStatus::UpToDate {
        system_image: current_system_image,
        update_package: latest_update_merkle,
    });

    if last_known_update_package == Some(&latest_update_merkle) {
        info!("Last known update package is latest, system is up to date");
        return up_to_date;
    }

    let update_available =
        Ok(SystemUpdateStatus::UpdateAvailable { current_system_image, latest_system_image });

    if current_system_image != latest_system_image {
        info!("Current system image is not latest, system is not up to date");
        return update_available;
    }

    let images = update_pkg
        .images_metadata()
        .await
        .map_err(errors::UpdatePackage::ExtractImagePackagesManifest)?;
    let images = images.fuchsia().ok_or(errors::UpdatePackage::MissingFuchsiaImages)?;

    let (asset_reader, current_config) =
        get_asset_reader(paver).await.map_err(Error::GetAssetReader)?;
    // When present, checking vbmeta is sufficient, but vbmeta isn't supported on all devices.
    for (asset, metadata) in
        [(Asset::VerifiedBootMetadata, images.vbmeta()), (Asset::Kernel, Some(images.zbi()))]
    {
        let Some(metadata) = metadata else {
            warn!(
                "Update package did not contain metadata for {asset:?}. It may not be supported \
                on this architecture, in which case this is expected."
            );
            continue;
        };
        match is_image_up_to_date(&asset_reader, current_config, asset, metadata).await {
            Ok(true) => {
                info!("Asset {:?} is up to date so system is up to date", asset);
                return up_to_date;
            }
            Ok(false) => {
                info!("Asset {:?} is not latest so system is not up to date", asset);
                return update_available;
            }
            Err(err) => {
                warn!("Failed to check if {:?} is up to date: {:#}", asset, anyhow!(err));
            }
        }
    }

    info!("Could not find any system differences, assuming system is up to date");
    up_to_date
}

fn current_system_image_merkle(file_system: &impl FileSystem) -> Result<Hash, Error> {
    file_system
        .read_to_string("/system/meta")
        .map_err(Error::ReadSystemMeta)?
        .parse::<Hash>()
        .map_err(Error::ParseSystemMeta)
}

async fn gc(space_manager: &fpkg_gc::ManagerProxy) -> Result<(), anyhow::Error> {
    let () = space_manager
        .gc()
        .await
        .context("while performing gc call")?
        .map_err(|e| anyhow!("garbage collection responded with {:?}", e))?;
    Ok(())
}

async fn latest_update_package(
    default_update_url: &str,
    package_resolver: &impl PackageResolverProxyInterface,
    channel_manager: &dyn TargetChannelUpdater,
    space_manager: &fpkg_gc::ManagerProxy,
) -> Result<update_package::UpdatePackage, errors::UpdatePackage> {
    match latest_update_package_attempt(default_update_url, package_resolver, channel_manager).await
    {
        Ok(update_package) => return Ok(update_package),
        Err(errors::UpdatePackage::Resolve(fidl_fuchsia_pkg_ext::ResolveError::NoSpace)) => {}
        Err(raw) => return Err(raw),
    }

    info!("No space left for update package.  Attempting to clean up older packages.");

    // If the first attempt fails with NoSpace, perform a GC and retry.
    if let Err(e) = gc(space_manager).await {
        error!("unable to gc packages: {:#}", anyhow!(e));
    }

    match latest_update_package_attempt(default_update_url, package_resolver, channel_manager).await
    {
        Ok(update_package) => Ok(update_package),
        Err(raw) => {
            error!("`latest_update_package_attempt` failed twice to fetch the update package.");
            Err(raw)
        }
    }
}

async fn latest_update_package_attempt(
    default_update_url: &str,
    package_resolver: &impl PackageResolverProxyInterface,
    channel_manager: &dyn TargetChannelUpdater,
) -> Result<update_package::UpdatePackage, errors::UpdatePackage> {
    let (dir_proxy, dir_server_end) = fidl::endpoints::create_proxy();
    let update_package = channel_manager
        .get_target_channel_update_url()
        .unwrap_or_else(|| default_update_url.to_owned());
    let fut = package_resolver.resolve(&update_package, dir_server_end);
    let _: fpkg::ResolutionContext = fut
        .await
        .map_err(errors::UpdatePackage::ResolveFidl)?
        .map_err(|raw| errors::UpdatePackage::Resolve(raw.into()))?;
    Ok(update_package::UpdatePackage::new(dir_proxy))
}

async fn latest_system_image_merkle(
    update_package: &update_package::UpdatePackage,
) -> Result<Hash, errors::UpdatePackage> {
    let packages =
        update_package.packages().await.map_err(errors::UpdatePackage::ExtractPackagesManifest)?;
    let system_image = packages
        .into_iter()
        .find(|url| url.path() == "system_image/0")
        .ok_or(errors::UpdatePackage::MissingSystemImage)?;
    Ok(system_image.hash())
}

async fn get_asset_reader(
    paver: &fpaver::PaverProxy,
) -> Result<(fpaver::DataSinkProxy, fpaver::Configuration), anyhow::Error> {
    let (boot_manager, server_end) = fidl::endpoints::create_proxy::<BootManagerMarker>();
    let () = paver.find_boot_manager(server_end).context("connect to fuchsia.paver.BootManager")?;
    let configuration = match boot_manager.query_current_configuration().await {
        Ok(res) => {
            res.map_err(zx::Status::err_from_raw).context("querying current configuration")?
        }
        Err(fidl::Error::ClientChannelClosed { epitaph, .. })
            if epitaph == zx::Status::NOT_SUPPORTED =>
        {
            warn!("device does not support ABR. Checking image in slot A");
            fpaver::Configuration::A
        }
        Err(err) => return Err(err).context("querying current configuration"),
    };

    let (data_sink, server_end) = fidl::endpoints::create_proxy::<DataSinkMarker>();
    let () = paver.find_data_sink(server_end).context("connect to fuchsia.paver.DataSink")?;
    Ok((data_sink, configuration))
}

async fn is_image_up_to_date(
    asset_reader: &fpaver::DataSinkProxy,
    current_config: fpaver::Configuration,
    asset: fpaver::Asset,
    latest: &update_package::ImageMetadata,
) -> Result<bool, anyhow::Error> {
    let mut current_image = asset_reader
        .read_asset(current_config, asset)
        .await
        .context("read_asset fidl error")?
        .map_err(|s| anyhow!("read_asset responded with {}", zx::Status::err_from_raw(s)))?;
    // The size field of the buffer returned by DataSink.ReadAsset will be either the size of the
    // entire partition or just the image itself.
    if current_image.size < latest.size() {
        return Ok(false);
    }
    current_image.size = latest.size();
    let current_hash = sha256_buffer(&current_image).context("hashing current")?;
    Ok(current_hash == latest.sha256())
}

fn sha256_buffer(
    fmem::Buffer { vmo, size }: &fmem::Buffer,
) -> anyhow::Result<fuchsia_hash::Sha256> {
    let mapping =
        mapped_vmo::ImmutableMapping::create_from_vmo(vmo, true).context("mapping the buffer")?;
    let size: usize = (*size).try_into().context("buffer size as usize")?;
    if size > mapping.len() {
        anyhow::bail!("buffer size {size} larger than vmo size {}", mapping.len());
    }
    Ok(From::from(*AsRef::<[u8; 32]>::as_ref(&<sha2::Sha256 as sha2::Digest>::digest(
        &mapping[..size],
    ))))
}

#[cfg(test)]
pub mod test_check_for_system_update_impl {
    use super::*;
    use crate::update_manager::tests::FakeTargetChannelUpdater;
    use assert_matches::assert_matches;
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_paver::Configuration;
    use fidl_fuchsia_pkg::{
        PackageResolverGetHashResult, PackageResolverResolveResult,
        PackageResolverResolveWithContextResult, PackageUrl,
    };
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use futures::{TryFutureExt, TryStreamExt, future};
    use maplit::hashmap;
    use mock_paver::MockPaverServiceBuilder;
    use std::collections::hash_map::HashMap;
    use std::fs;
    use std::sync::{Arc, LazyLock};

    const ACTIVE_SYSTEM_IMAGE_MERKLE: &str =
        "0000000000000000000000000000000000000000000000000000000000000000";
    const NEW_SYSTEM_IMAGE_MERKLE: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";
    const TEST_UPDATE_PACKAGE_URL: &str = "fuchsia-pkg://fuchsia.test/update";
    const ACTIVE_VBMETA_CONTENTS: [u8; 1] = [1];
    const ACTIVE_VBMETA_HASH: fuchsia_hash::Sha256 = fuchsia_hash::Sha256::from_array([
        0x4b, 0xf5, 0x12, 0x2f, 0x34, 0x45, 0x54, 0xc5, 0x3b, 0xde, 0x2e, 0xbb, 0x8c, 0xd2, 0xb7,
        0xe3, 0xd1, 0x60, 0x0a, 0xd6, 0x31, 0xc3, 0x85, 0xa5, 0xd7, 0xcc, 0xe2, 0x3c, 0x77, 0x85,
        0x45, 0x9a,
    ]);
    const NEW_VBMETA_HASH: fuchsia_hash::Sha256 = fuchsia_hash::Sha256::from_array([4u8; 32]);
    const ACTIVE_ZBI_CONTENTS: [u8; 1] = [0];
    const ACTIVE_ZBI_HASH: fuchsia_hash::Sha256 = fuchsia_hash::Sha256::from_array([
        0x6e, 0x34, 0x0b, 0x9c, 0xff, 0xb3, 0x7a, 0x98, 0x9c, 0xa5, 0x44, 0xe6, 0xbb, 0x78, 0x0a,
        0x2c, 0x78, 0x90, 0x1d, 0x3f, 0xb3, 0x37, 0x38, 0x76, 0x85, 0x11, 0xa3, 0x06, 0x17, 0xaf,
        0xa0, 0x1d,
    ]);
    const NEW_ZBI_HASH: fuchsia_hash::Sha256 = fuchsia_hash::Sha256::from_array([6u8; 32]);

    static UPDATE_PACKAGE_MERKLE: LazyLock<Hash> = LazyLock::new(|| [0x22; 32].into());

    struct FakeFileSystem {
        contents: HashMap<String, String>,
    }
    impl FakeFileSystem {
        fn new_with_valid_system_meta() -> FakeFileSystem {
            FakeFileSystem {
                contents: hashmap![
                    "/system/meta".to_string() => ACTIVE_SYSTEM_IMAGE_MERKLE.to_string()
                ],
            }
        }
    }
    impl FileSystem for FakeFileSystem {
        fn read_to_string(&self, path: &str) -> io::Result<String> {
            self.contents
                .get(path)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("not present in fake file system: {path}"),
                    )
                })
                .map(|s| s.to_string())
        }
    }

    pub struct PackageResolverProxyTempDirBuilder {
        temp_dir: tempfile::TempDir,
        expected_package_url: String,
        write_packages_json: bool,
        packages: Vec<String>,
        zbi: Option<fuchsia_hash::Sha256>,
        vbmeta: Option<fuchsia_hash::Sha256>,
    }

    impl PackageResolverProxyTempDirBuilder {
        fn new() -> PackageResolverProxyTempDirBuilder {
            Self {
                temp_dir: tempfile::tempdir().expect("create temp dir"),
                expected_package_url: TEST_UPDATE_PACKAGE_URL.to_owned(),
                write_packages_json: false,
                packages: Vec::new(),
                zbi: None,
                vbmeta: None,
            }
        }

        fn with_packages_json(mut self) -> Self {
            self.write_packages_json = true;
            self
        }

        fn with_system_image_merkle(mut self, merkle: &str) -> Self {
            assert!(self.write_packages_json);
            self.packages.push(format!("fuchsia-pkg://fuchsia.com/system_image/0?hash={merkle}"));
            self
        }

        fn with_zbi(mut self, zbi: fuchsia_hash::Sha256) -> Self {
            self.zbi = Some(zbi);
            self
        }

        fn with_vbmeta(mut self, vbmeta: fuchsia_hash::Sha256) -> Self {
            self.vbmeta = Some(vbmeta);
            self
        }

        fn with_update_url(mut self, url: &str) -> Self {
            self.expected_package_url = url.to_owned();
            self
        }

        fn build(self) -> PackageResolverProxyTempDir {
            fs::write(self.temp_dir.path().join("meta"), UPDATE_PACKAGE_MERKLE.to_string())
                .expect("write meta");
            if self.write_packages_json {
                let packages_json = serde_json::json!({
                    "version": "1",
                    "content": self.packages
                })
                .to_string();
                fs::write(self.temp_dir.path().join("packages.json"), packages_json)
                    .expect("write packages.json");
            }

            // is_image_up_to_date only reads the first latest_image.len() bytes from the
            // active_image VMO.
            // The URLs are unused.
            let mut images = update_package::images::ImagePackagesManifest::builder();
            images.fuchsia_package(
                update_package::images::ImageMetadata::new(
                    ACTIVE_ZBI_CONTENTS.len().try_into().unwrap(),
                    self.zbi.unwrap_or(ACTIVE_ZBI_HASH),
                    "fuchsia-pkg://fuchsia.test/images?hash=0000000000000000000000000000000000000000000000000000000000000000#zbi".parse().unwrap(),
                ),
                self.vbmeta.map(|h| {
                    update_package::images::ImageMetadata::new(
                        ACTIVE_VBMETA_CONTENTS.len().try_into().unwrap(),
                        h,
                        "fuchsia-pkg://fuchsia.test/images?hash=0000000000000000000000000000000000000000000000000000000000000000#vbmeta".parse().unwrap(),
                    )
                }),
            );
            let images = images.build();

            let () = fs::write(
                self.temp_dir.path().join("images.json"),
                serde_json::to_vec(&images).unwrap(),
            )
            .expect("writing images.json");

            PackageResolverProxyTempDir {
                temp_dir: self.temp_dir,
                expected_package_url: self.expected_package_url,
            }
        }
    }

    pub struct PackageResolverProxyTempDir {
        temp_dir: tempfile::TempDir,
        expected_package_url: String,
    }
    impl PackageResolverProxyTempDir {
        fn new_with_default_meta() -> PackageResolverProxyTempDir {
            PackageResolverProxyTempDirBuilder::new().build()
        }

        fn new_with_empty_packages_json() -> PackageResolverProxyTempDir {
            PackageResolverProxyTempDirBuilder::new().with_packages_json().build()
        }

        fn new_with_latest_system_image(merkle: &str) -> PackageResolverProxyTempDir {
            PackageResolverProxyTempDirBuilder::new()
                .with_packages_json()
                .with_system_image_merkle(merkle)
                .build()
        }

        fn new_with_vbmeta(vbmeta: fuchsia_hash::Sha256) -> PackageResolverProxyTempDir {
            PackageResolverProxyTempDirBuilder::new()
                .with_packages_json()
                .with_system_image_merkle(ACTIVE_SYSTEM_IMAGE_MERKLE)
                .with_vbmeta(vbmeta)
                .build()
        }

        fn new_with_zbi(zbi: fuchsia_hash::Sha256) -> PackageResolverProxyTempDir {
            PackageResolverProxyTempDirBuilder::new()
                .with_packages_json()
                .with_system_image_merkle(ACTIVE_SYSTEM_IMAGE_MERKLE)
                .with_zbi(zbi)
                .build()
        }
    }
    impl PackageResolverProxyInterface for PackageResolverProxyTempDir {
        type ResolveResponseFut = future::Ready<Result<PackageResolverResolveResult, fidl::Error>>;
        fn resolve(
            &self,
            package_url: &str,
            dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
        ) -> Self::ResolveResponseFut {
            assert_eq!(package_url, self.expected_package_url);
            fdio::open(
                self.temp_dir.path().to_str().expect("path is utf8"),
                fio::PERM_READABLE,
                dir.into_channel(),
            )
            .unwrap();
            future::ok(Ok(fpkg::ResolutionContext { bytes: vec![] }))
        }

        type ResolveWithContextResponseFut =
            future::Ready<Result<PackageResolverResolveWithContextResult, fidl::Error>>;
        fn resolve_with_context(
            &self,
            _package_url: &str,
            _context: &fpkg::ResolutionContext,
            _dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
        ) -> Self::ResolveWithContextResponseFut {
            panic!("resolve_with_context not implemented");
        }

        type GetHashResponseFut = future::Ready<Result<PackageResolverGetHashResult, fidl::Error>>;
        fn get_hash(&self, _package_url: &PackageUrl) -> Self::GetHashResponseFut {
            panic!("get_hash not implemented");
        }
    }

    struct MockSpaceManagerService {
        call_count: Mutex<u32>,
    }

    impl MockSpaceManagerService {
        fn new() -> Self {
            Self { call_count: Mutex::new(0) }
        }

        /// Spawns a new task to serve the space manager protocol.
        pub fn spawn_gc_service(self: &Arc<Self>) -> fpkg_gc::ManagerProxy {
            let (proxy, server_end) =
                fidl::endpoints::create_proxy_and_stream::<fpkg_gc::ManagerMarker>();

            fasync::Task::spawn(Arc::clone(self).run_gc_service(server_end).unwrap_or_else(|e| {
                panic!("error running space manager service: {:#}", anyhow!(e))
            }))
            .detach();

            proxy
        }

        async fn run_gc_service(
            self: Arc<Self>,
            mut stream: fpkg_gc::ManagerRequestStream,
        ) -> Result<(), anyhow::Error> {
            while let Some(req) = stream.try_next().await? {
                match req {
                    fpkg_gc::ManagerRequest::Gc { responder } => {
                        *self.call_count.lock() += 1;
                        responder.send(Ok(())).unwrap()
                    }
                    fpkg_gc::ManagerRequest::_UnknownMethod { .. } => {
                        return Err(anyhow!("Unknown method called on garbage collector."));
                    }
                }
            }
            Ok(())
        }
    }

    #[fuchsia::test]
    async fn test_missing_system_meta_file() {
        let mut file_system = FakeFileSystem { contents: hashmap![] };
        let package_resolver = PackageResolverProxyTempDir::new_with_default_meta();
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();

        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(result, Err(Error::ReadSystemMeta(_)));
    }

    #[fuchsia::test]
    async fn test_malformatted_system_meta_file() {
        let mut file_system = FakeFileSystem {
            contents: hashmap![
                "/system/meta".to_string() => "not-a-merkle".to_string()
            ],
        };
        let package_resolver = PackageResolverProxyTempDir::new_with_default_meta();
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();

        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(result, Err(Error::ParseSystemMeta(_)));
    }

    #[fuchsia::test]
    async fn test_resolve_update_package_fidl_error() {
        struct PackageResolverProxyFidlError;
        impl PackageResolverProxyInterface for PackageResolverProxyFidlError {
            type ResolveResponseFut =
                future::Ready<Result<PackageResolverResolveResult, fidl::Error>>;
            fn resolve(
                &self,
                _package_url: &str,
                _dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
            ) -> Self::ResolveResponseFut {
                future::err(fidl::Error::Invalid)
            }

            type ResolveWithContextResponseFut =
                future::Ready<Result<PackageResolverResolveWithContextResult, fidl::Error>>;
            fn resolve_with_context(
                &self,
                _package_url: &str,
                _context: &fpkg::ResolutionContext,
                _dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
            ) -> Self::ResolveWithContextResponseFut {
                future::err(fidl::Error::Invalid)
            }

            type GetHashResponseFut =
                future::Ready<Result<PackageResolverGetHashResult, fidl::Error>>;
            fn get_hash(&self, _package_url: &PackageUrl) -> Self::GetHashResponseFut {
                panic!("get_hash not implemented");
            }
        }

        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyFidlError;
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(result, Err(Error::UpdatePackage(errors::UpdatePackage::ResolveFidl(_))));
    }

    #[fuchsia::test]
    async fn test_update_package_garbage_collection_called_no_space() {
        struct PackageResolverProxyNoSpaceOnce {
            temp_dir: tempfile::TempDir,
            call_count: Mutex<u32>,
        }

        impl PackageResolverProxyNoSpaceOnce {
            fn new() -> PackageResolverProxyNoSpaceOnce {
                let package_resolver = PackageResolverProxyTempDirBuilder::new()
                    .with_packages_json()
                    .with_system_image_merkle(ACTIVE_SYSTEM_IMAGE_MERKLE)
                    .build();
                PackageResolverProxyNoSpaceOnce {
                    temp_dir: package_resolver.temp_dir,
                    call_count: Mutex::new(0),
                }
            }
        }

        impl PackageResolverProxyInterface for PackageResolverProxyNoSpaceOnce {
            type ResolveResponseFut =
                future::Ready<Result<PackageResolverResolveResult, fidl::Error>>;
            fn resolve(
                &self,
                package_url: &str,
                dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
            ) -> Self::ResolveResponseFut {
                *self.call_count.lock() += 1;

                if *self.call_count.lock() == 1 {
                    return future::ok(Err(fidl_fuchsia_pkg::ResolveError::NoSpace));
                }

                assert_eq!(package_url, TEST_UPDATE_PACKAGE_URL);
                fdio::open(
                    self.temp_dir.path().to_str().expect("path is utf8"),
                    fio::PERM_READABLE,
                    dir.into_channel(),
                )
                .unwrap();
                future::ok(Ok(fpkg::ResolutionContext { bytes: vec![] }))
            }

            type ResolveWithContextResponseFut =
                future::Ready<Result<PackageResolverResolveWithContextResult, fidl::Error>>;
            fn resolve_with_context(
                &self,
                _package_url: &str,
                _context: &fpkg::ResolutionContext,
                _dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
            ) -> Self::ResolveWithContextResponseFut {
                panic!("resolve_with_context not implemented");
            }

            type GetHashResponseFut =
                future::Ready<Result<PackageResolverGetHashResult, fidl::Error>>;
            fn get_hash(&self, _package_url: &PackageUrl) -> Self::GetHashResponseFut {
                panic!("get_hash not implemented");
            }
        }

        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyNoSpaceOnce::new();
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            Some(&UPDATE_PACKAGE_MERKLE),
            &space_manager,
        )
        .await;

        assert_matches!(result, Ok(SystemUpdateStatus::UpToDate { system_image, update_package: _ })
        if system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
        .parse()
        .expect("active system image string literal"));

        assert_matches!(*mock_space_manager.call_count.lock(), 1);
        assert_matches!(*package_resolver.call_count.lock(), 2);
    }

    #[fuchsia::test]
    async fn test_update_package_garbage_collection_succeeds() {
        struct PackageResolverProxyNoSpaceError;
        impl PackageResolverProxyInterface for PackageResolverProxyNoSpaceError {
            type ResolveResponseFut =
                future::Ready<Result<PackageResolverResolveResult, fidl::Error>>;
            fn resolve(
                &self,
                _package_url: &str,
                _dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
            ) -> Self::ResolveResponseFut {
                future::ok(Err(fidl_fuchsia_pkg::ResolveError::NoSpace))
            }

            type ResolveWithContextResponseFut =
                future::Ready<Result<PackageResolverResolveWithContextResult, fidl::Error>>;
            fn resolve_with_context(
                &self,
                _package_url: &str,
                _context: &fpkg::ResolutionContext,
                _dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
            ) -> Self::ResolveWithContextResponseFut {
                panic!("resolve_with_context not implemented");
            }

            type GetHashResponseFut =
                future::Ready<Result<PackageResolverGetHashResult, fidl::Error>>;
            fn get_hash(&self, _package_url: &PackageUrl) -> Self::GetHashResponseFut {
                panic!("get_hash not implemented");
            }
        }

        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyNoSpaceError;
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Err(Error::UpdatePackage(errors::UpdatePackage::Resolve(
                fidl_fuchsia_pkg_ext::ResolveError::NoSpace
            )))
        );
        assert_matches!(*mock_space_manager.call_count.lock(), 1);
    }

    #[fuchsia::test]
    async fn test_resolve_update_package_zx_error() {
        struct PackageResolverProxyZxError;
        impl PackageResolverProxyInterface for PackageResolverProxyZxError {
            type ResolveResponseFut =
                future::Ready<Result<PackageResolverResolveResult, fidl::Error>>;
            fn resolve(
                &self,
                _package_url: &str,
                _dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
            ) -> Self::ResolveResponseFut {
                future::ok(Err(fidl_fuchsia_pkg::ResolveError::Internal))
            }

            type ResolveWithContextResponseFut =
                future::Ready<Result<PackageResolverResolveWithContextResult, fidl::Error>>;
            fn resolve_with_context(
                &self,
                _package_url: &str,
                _context: &fpkg::ResolutionContext,
                _dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
            ) -> Self::ResolveWithContextResponseFut {
                panic!("resolve_with_context not implemented");
            }

            type GetHashResponseFut =
                future::Ready<Result<PackageResolverGetHashResult, fidl::Error>>;
            fn get_hash(&self, _package_url: &PackageUrl) -> Self::GetHashResponseFut {
                panic!("get_hash not implemented");
            }
        }

        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyZxError;
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(result, Err(Error::UpdatePackage(errors::UpdatePackage::Resolve(_))));
    }

    #[fuchsia::test]
    async fn test_resolve_update_package_directory_closed() {
        struct PackageResolverProxyDirectoryCloser;
        impl PackageResolverProxyInterface for PackageResolverProxyDirectoryCloser {
            type ResolveResponseFut =
                future::Ready<Result<PackageResolverResolveResult, fidl::Error>>;
            fn resolve(
                &self,
                _package_url: &str,
                _dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
            ) -> Self::ResolveResponseFut {
                future::ok(Ok(fpkg::ResolutionContext { bytes: vec![] }))
            }

            type ResolveWithContextResponseFut =
                future::Ready<Result<PackageResolverResolveWithContextResult, fidl::Error>>;
            fn resolve_with_context(
                &self,
                _package_url: &str,
                _context: &fpkg::ResolutionContext,
                _dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
            ) -> Self::ResolveWithContextResponseFut {
                panic!("resolve_with_context not implemented");
            }

            type GetHashResponseFut =
                future::Ready<Result<PackageResolverGetHashResult, fidl::Error>>;
            fn get_hash(&self, _package_url: &PackageUrl) -> Self::GetHashResponseFut {
                panic!("get_hash not implemented");
            }
        }

        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyDirectoryCloser;
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(result, Err(Error::UpdatePackage(errors::UpdatePackage::Hash(_))));
    }

    #[fuchsia::test]
    async fn test_update_package_missing_packages_json() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyTempDir::new_with_default_meta();
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Err(Error::UpdatePackage(errors::UpdatePackage::ExtractPackagesManifest(_)))
        );
    }

    #[fuchsia::test]
    async fn test_update_package_empty_packages_json() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyTempDir::new_with_empty_packages_json();
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Err(Error::UpdatePackage(errors::UpdatePackage::MissingSystemImage))
        );
    }

    #[fuchsia::test]
    async fn test_update_package_bad_system_image() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver =
            PackageResolverProxyTempDir::new_with_latest_system_image("bad-merkle");
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;
        assert_matches!(
            result,
            Err(Error::UpdatePackage(errors::UpdatePackage::ExtractPackagesManifest(_)))
        );
    }

    #[fuchsia::test]
    async fn test_same_update_package_up_to_date() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyTempDir::new_with_vbmeta(ACTIVE_VBMETA_HASH);
        let mock_paver = Arc::new(
            MockPaverServiceBuilder::new()
                .active_config(Configuration::A)
                .insert_hook(mock_paver::hooks::read_asset(|configuration, asset| {
                    assert_eq!(configuration, Configuration::A);
                    match asset {
                        Asset::Kernel => panic!("shouldn't read kernel if vbmeta is the same!"),
                        Asset::VerifiedBootMetadata => Ok(ACTIVE_VBMETA_CONTENTS.into()),
                    }
                }))
                .build(),
        );
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            Some(&UPDATE_PACKAGE_MERKLE),
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Ok(SystemUpdateStatus::UpToDate { system_image, update_package: _ })
                if system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                .parse()
                .expect("active system image string literal")
        );
    }

    #[fuchsia::test]
    async fn test_up_to_date_with_missing_vbmeta() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyTempDir::new_with_vbmeta(ACTIVE_VBMETA_HASH);
        let mock_paver = Arc::new(
            MockPaverServiceBuilder::new()
                .active_config(Configuration::A)
                .insert_hook(mock_paver::hooks::read_asset(|configuration, asset| {
                    assert_eq!(configuration, Configuration::A);
                    match asset {
                        Asset::Kernel => Err(zx::Status::NOT_FOUND),
                        Asset::VerifiedBootMetadata => Err(zx::Status::NOT_FOUND),
                    }
                }))
                .build(),
        );
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            Some(&UPDATE_PACKAGE_MERKLE),
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Ok(SystemUpdateStatus::UpToDate { system_image, update_package: _ })
                if system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                .parse()
                .expect("active system image string literal")
        );
    }

    #[fuchsia::test]
    async fn test_update_available() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver =
            PackageResolverProxyTempDir::new_with_latest_system_image(NEW_SYSTEM_IMAGE_MERKLE);
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Ok(SystemUpdateStatus::UpdateAvailable { current_system_image, latest_system_image })
            if
                current_system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                    .parse()
                    .expect("active system image string literal") &&
                latest_system_image == NEW_SYSTEM_IMAGE_MERKLE
                    .parse()
                    .expect("new system image string literal")
        );
        assert_matches!(*mock_space_manager.call_count.lock(), 0);
    }

    #[fuchsia::test]
    async fn test_update_available_changing_target_channel() {
        let update_url: &str = "fuchsia-pkg://target.channel.test/my-update-package";
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyTempDirBuilder::new()
            .with_packages_json()
            .with_system_image_merkle(NEW_SYSTEM_IMAGE_MERKLE)
            .with_update_url(update_url)
            .build();
        let mock_paver = Arc::new(MockPaverServiceBuilder::new().build());
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new_with_update_url(update_url),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Ok(SystemUpdateStatus::UpdateAvailable { current_system_image, latest_system_image })
            if
                current_system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                    .parse()
                    .expect("active system image string literal") &&
                latest_system_image == NEW_SYSTEM_IMAGE_MERKLE
                    .parse()
                    .expect("new system image string literal")
        );
    }

    #[fuchsia::test]
    async fn test_different_update_package_hash_does_not_trigger_update() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver =
            PackageResolverProxyTempDir::new_with_latest_system_image(ACTIVE_SYSTEM_IMAGE_MERKLE);
        let mock_paver = Arc::new(
            MockPaverServiceBuilder::new()
                .active_config(Configuration::A)
                .insert_hook(mock_paver::hooks::read_asset(|configuration, asset| {
                    assert_eq!(configuration, Configuration::A);
                    match asset {
                        Asset::Kernel => Ok(ACTIVE_ZBI_CONTENTS.into()),
                        Asset::VerifiedBootMetadata => panic!("no vbmeta available"),
                    }
                }))
                .build(),
        );
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let previous_update_package = Hash::from([0x44; 32]);
        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            Some(&previous_update_package),
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Ok(SystemUpdateStatus::UpToDate { system_image, update_package})
            if system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                .parse()
                .expect("active system image string literal") &&
            update_package == *UPDATE_PACKAGE_MERKLE
        );
    }

    #[fuchsia::test]
    async fn test_vbmeta_only_update_available() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyTempDir::new_with_vbmeta(NEW_VBMETA_HASH);
        let mock_paver = Arc::new(
            MockPaverServiceBuilder::new()
                .active_config(Configuration::A)
                .insert_hook(mock_paver::hooks::read_asset(|configuration, asset| {
                    assert_eq!(configuration, Configuration::A);
                    match asset {
                        Asset::Kernel => panic!("not expecting to read kernel"),
                        Asset::VerifiedBootMetadata => Ok(ACTIVE_VBMETA_CONTENTS.into()),
                    }
                }))
                .build(),
        );
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Ok(SystemUpdateStatus::UpdateAvailable { current_system_image, latest_system_image })
            if
                current_system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                    .parse()
                    .expect("active system image string literal") &&
                latest_system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                    .parse()
                    .expect("new system image string literal")
        );
    }

    #[fuchsia::test]
    async fn test_zbi_only_update_available() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyTempDir::new_with_zbi(NEW_ZBI_HASH);
        let mock_paver = Arc::new(
            MockPaverServiceBuilder::new()
                .active_config(Configuration::A)
                .insert_hook(mock_paver::hooks::read_asset(|configuration, asset| {
                    assert_eq!(configuration, Configuration::A);
                    match asset {
                        Asset::Kernel => Ok(ACTIVE_ZBI_CONTENTS.into()),
                        Asset::VerifiedBootMetadata => panic!("no vbmeta available"),
                    }
                }))
                .build(),
        );
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Ok(SystemUpdateStatus::UpdateAvailable { current_system_image, latest_system_image })
            if
                current_system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                    .parse()
                    .expect("active system image string literal") &&
                latest_system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                    .parse()
                    .expect("new system image string literal")
        );
    }

    #[fuchsia::test]
    async fn test_vbmeta_did_not_require_update() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyTempDir::new_with_vbmeta(ACTIVE_VBMETA_HASH);
        let mock_paver = Arc::new(
            MockPaverServiceBuilder::new()
                .active_config(Configuration::A)
                .insert_hook(mock_paver::hooks::read_asset(|configuration, asset| {
                    assert_eq!(configuration, Configuration::A);
                    match asset {
                        Asset::Kernel => panic!("not expecting to read kernel"),
                        Asset::VerifiedBootMetadata => Ok(ACTIVE_VBMETA_CONTENTS.into()),
                    }
                }))
                .build(),
        );
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Ok(SystemUpdateStatus::UpToDate { system_image, update_package: _ })
                if system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                .parse()
                .expect("active system image string literal")
        );
    }

    #[fuchsia::test]
    async fn test_zbi_did_not_require_update() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyTempDir::new_with_zbi(ACTIVE_ZBI_HASH);
        let mock_paver = Arc::new(
            MockPaverServiceBuilder::new()
                .active_config(Configuration::A)
                .insert_hook(mock_paver::hooks::read_asset(|configuration, asset| {
                    assert_eq!(configuration, Configuration::A);
                    match asset {
                        Asset::Kernel => Ok(ACTIVE_ZBI_CONTENTS.into()),
                        Asset::VerifiedBootMetadata => panic!("no vbmeta available"),
                    }
                }))
                .build(),
        );
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Ok(SystemUpdateStatus::UpToDate { system_image, update_package: _ })
                if system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                .parse()
                .expect("active system image string literal")
        );
    }

    #[fuchsia::test]
    async fn test_zbi_only_no_abr_update_available() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyTempDir::new_with_zbi(NEW_ZBI_HASH);
        let mock_paver = Arc::new(
            MockPaverServiceBuilder::new()
                .boot_manager_close_with_epitaph(zx::Status::NOT_SUPPORTED)
                .insert_hook(mock_paver::hooks::read_asset(|configuration, asset| {
                    assert_eq!(configuration, Configuration::A);
                    match asset {
                        Asset::Kernel => Ok(ACTIVE_ZBI_CONTENTS.into()),
                        Asset::VerifiedBootMetadata => panic!("no vbmeta available"),
                    }
                }))
                .build(),
        );
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Ok(SystemUpdateStatus::UpdateAvailable { current_system_image, latest_system_image })
            if
                current_system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                    .parse()
                    .expect("active system image string literal") &&
                latest_system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                    .parse()
                    .expect("new system image string literal")
        );
    }

    #[fuchsia::test]
    async fn test_zbi_no_abr_did_not_require_update() {
        let mut file_system = FakeFileSystem::new_with_valid_system_meta();
        let package_resolver = PackageResolverProxyTempDir::new_with_zbi(ACTIVE_ZBI_HASH);
        let mock_paver = Arc::new(
            MockPaverServiceBuilder::new()
                .boot_manager_close_with_epitaph(zx::Status::NOT_SUPPORTED)
                .insert_hook(mock_paver::hooks::read_asset(|configuration, asset| {
                    assert_eq!(configuration, Configuration::A);
                    match asset {
                        Asset::Kernel => Ok(ACTIVE_ZBI_CONTENTS.into()),
                        Asset::VerifiedBootMetadata => panic!("no vbmeta available"),
                    }
                }))
                .build(),
        );
        let paver = mock_paver.spawn_paver_service();
        let mock_space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager = mock_space_manager.spawn_gc_service();

        let result = check_for_system_update_impl(
            TEST_UPDATE_PACKAGE_URL,
            &mut file_system,
            &package_resolver,
            &paver,
            &FakeTargetChannelUpdater::new(),
            None,
            &space_manager,
        )
        .await;

        assert_matches!(
            result,
            Ok(SystemUpdateStatus::UpToDate { system_image, update_package: _ })
                if system_image == ACTIVE_SYSTEM_IMAGE_MERKLE
                .parse()
                .expect("active system image string literal")
        );
    }
}

#[cfg(test)]
mod test_real_file_system {
    use super::*;
    use assert_matches::assert_matches;
    use proptest::prelude::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn test_read_to_string_errors_on_missing_file() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let read_res = RealFileSystem.read_to_string(
            dir.path().join("this-file-does-not-exist").to_str().expect("paths are utf8"),
        );
        assert_matches!(read_res.map_err(|e| e.kind()), Err(io::ErrorKind::NotFound));
    }

    proptest! {
        #![proptest_config(ProptestConfig{
            failure_persistence: None,
            ..Default::default()
        })]

        #[test]
        fn test_read_to_string_preserves_contents(
            contents in ".{0, 65}",
            file_name in "[^\\.\0/]{1,10}",
        ) {
            let dir = tempfile::tempdir().expect("create temp dir");
            let file_path = dir.path().join(file_name);
            let mut file = fs::File::create(&file_path).expect("create file");
            file.write_all(contents.as_bytes()).expect("write the contents");

            let read_contents = RealFileSystem
                .read_to_string(file_path.to_str().expect("paths are utf8"))
                .expect("read the file");

            prop_assert_eq!(read_contents, contents);
        }
    }
}
