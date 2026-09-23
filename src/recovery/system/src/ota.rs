// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::config::{RecoveryUpdateConfig, UpdateType};
use crate::setup::DevhostConfig;
use anyhow::{Context, Error, bail, format_err};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_buildinfo::ProviderMarker as BuildInfoMarker;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_component::client;
use http_body_util::BodyExt;
use hyper::Uri;
use isolated_ota::{OmahaConfig, download_and_apply_update};
use serde_json::{Value, json};
use std::fs::File;
use std::str::FromStr;
use std::sync::Arc;
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::immutable::simple::Simple;

const PATH_TO_CONFIGS_DIR: &'static str = "/config/data/ota-configs";
const SERVE_FLAGS: fio::Flags =
    fio::PERM_READABLE.union(fio::PERM_WRITABLE).union(fio::PERM_EXECUTABLE);

enum OtaType {
    /// Ota from a devhost.
    Devhost { cfg: DevhostConfig },
    /// Ota from a well-known location. TODO(simonshields): implement this.
    WellKnown,
}

enum BoardName {
    /// Use board name from /config/build-info.
    BuildInfo,
    /// Override board name with given value.
    #[allow(dead_code)]
    Override { name: String },
}

/// Helper for constructing OTAs.
pub struct OtaEnvBuilder {
    board_name: BoardName,
    omaha_config: Option<OmahaConfig>,
    ota_type: OtaType,
    ssl_certificates: String,
    outgoing_dir: Arc<Simple>,
    blobfs_proxy: Option<fio::DirectoryProxy>,
}

impl OtaEnvBuilder {
    /// Create a new `OtaEnvBuilder`. Requires an `outgoing_dir` which is served
    /// by an instantiation of a Rust VFS tied to this component's outgoing
    /// directory. This is required in order to prepare the outgoing directory
    /// with capabilities like directories and storage for the `pkg-recovery.cm`
    /// component which will be created as a child.
    pub fn new(outgoing_dir: Arc<Simple>) -> Self {
        OtaEnvBuilder {
            board_name: BoardName::BuildInfo,
            omaha_config: None,
            ota_type: OtaType::WellKnown,
            ssl_certificates: "/config/ssl".to_owned(),
            outgoing_dir,
            blobfs_proxy: None,
        }
    }

    #[cfg(test)]
    /// Override the board name for this OTA.
    pub fn board_name(mut self, name: &str) -> Self {
        self.board_name = BoardName::Override { name: name.to_owned() };
        self
    }

    /// Use the given |DevhostConfig| to run an OTA.
    pub fn devhost(mut self, cfg: DevhostConfig) -> Self {
        self.ota_type = OtaType::Devhost { cfg };
        self
    }

    #[allow(dead_code)]
    /// Use the given |OmahaConfig| to run an OTA.
    pub fn omaha_config(mut self, omaha_config: OmahaConfig) -> Self {
        self.omaha_config = Some(omaha_config);
        self
    }

    /// Use the given StorageType as the storage target.
    pub fn blobfs_proxy(mut self, blobfs_proxy: fio::DirectoryProxy) -> Self {
        self.blobfs_proxy = Some(blobfs_proxy);
        self
    }

    #[cfg(test)]
    /// Use the given path for SSL certificates.
    pub fn ssl_certificates(mut self, path: &str) -> Self {
        self.ssl_certificates = path.to_owned();
        self
    }

    /// Returns the name of the board provided by fidl/fuchsia.buildinfo
    async fn get_board_name(&self) -> Result<String, Error> {
        match &self.board_name {
            BoardName::BuildInfo => {
                let proxy = match client::connect_to_protocol::<BuildInfoMarker>() {
                    Ok(p) => p,
                    Err(err) => {
                        bail!("Failed to connect to fuchsia.buildinfo.Provider proxy: {:?}", err)
                    }
                };
                let build_info =
                    proxy.get_build_info().await.context("Failed to read build info")?;
                build_info.board_config.ok_or_else(|| format_err!("No board name provided"))
            }
            BoardName::Override { name } => Ok(name.to_owned()),
        }
    }

    /// Takes a devhost config, and converts into a pkg-resolver friendly format.
    /// Returns a |File| representing a directory with the repository
    /// configuration in it.
    async fn get_devhost_config(&self, cfg: &DevhostConfig) -> Result<File, Error> {
        // Get the repository information from the devhost (including keys and repo URL).
        let client = fuchsia_hyper::new_client();
        let response = client
            .get(Uri::from_str(&cfg.url).context("Bad URL")?)
            .await
            .context("Fetching config from devhost")?;
        let body = response.into_body().collect().await.context("into body")?.to_bytes();
        let repo_info: Value = serde_json::from_slice(&body).context("Failed to parse JSON")?;

        // Convert into a pkg-resolver friendly format.
        let config_for_resolver = json!({
            "version": "1",
            "content": [
            {
                "repo_url": "fuchsia-pkg://fuchsia.com",
                "root_version": 1,
                "root_threshold": 1,
                "root_keys": repo_info["root_keys"],
                "mirrors":[{
                    "mirror_url": repo_info["repo_url"],
                    "subscribe": true
                }],
                "update_package_url": null
            }
            ]
        });

        // Set up a repo configuration folder for the resolver, and write out the config.
        let tempdir = tempfile::tempdir().context("tempdir")?;
        let file = tempdir.path().join("devhost.json");
        let tmp_file = File::create(file).context("Creating file")?;
        serde_json::to_writer(tmp_file, &config_for_resolver).context("Writing JSON")?;

        Ok(File::open(tempdir.keep()).context("Opening tmpdir")?)
    }

    async fn get_wellknown_config(&self) -> Result<File, Error> {
        println!("recovery-ota: passing in config from config_data");
        Ok(File::open(PATH_TO_CONFIGS_DIR).context("Opening config data path")?)
    }

    /// Construct an |OtaEnv| from this |OtaEnvBuilder|.
    pub async fn build(self) -> Result<OtaEnv, Error> {
        let repo_dir = match &self.ota_type {
            OtaType::Devhost { cfg } => {
                self.get_devhost_config(cfg).await.context("Getting devhost config")?
            }
            OtaType::WellKnown => {
                self.get_wellknown_config().await.context("Preparing wellknown config")?
            }
        };

        let ssl_certificates =
            File::open(&self.ssl_certificates).context("Opening SSL certificate folder")?;

        let board_name = self.get_board_name().await.context("Could not get board name")?;

        let blobfs_proxy =
            self.blobfs_proxy.ok_or_else(|| format_err!("Blobfs proxy not found"))?;

        Ok(OtaEnv {
            blobfs_proxy,
            board_name,
            omaha_config: self.omaha_config,
            repo_dir,
            ssl_certificates,
            outgoing_dir: self.outgoing_dir,
        })
    }
}

pub struct OtaEnv {
    blobfs_proxy: fio::DirectoryProxy,
    board_name: String,
    omaha_config: Option<OmahaConfig>,
    repo_dir: File,
    ssl_certificates: File,
    outgoing_dir: Arc<Simple>,
}

impl OtaEnv {
    /// Run the OTA, targeting the given channel and reporting the given version
    /// as the current system version.
    pub async fn do_ota(self, channel: &str, version: &str) -> Result<(), Error> {
        fn proxy_from_file(file: File) -> Result<fio::DirectoryProxy, Error> {
            Ok(fio::DirectoryProxy::new(fuchsia_async::Channel::from_channel(
                fdio::transfer_fd(file)?.into(),
            )))
        }

        // Utilize the repository configs and ssl certificates we were provided,
        // by placing them in our outgoing directory.
        self.outgoing_dir.add_entry(
            "config",
            vfs::pseudo_directory! {
                "data" => vfs::pseudo_directory!{
                        "repositories" => vfs::remote::remote_dir(proxy_from_file(self.repo_dir)?)
                },
                "ssl" => vfs::remote::remote_dir(
                    proxy_from_file(self.ssl_certificates)?
                ),
                "build-info" => vfs::pseudo_directory!{
                    "board" => vfs::file::vmo::read_only(self.board_name),
                    "version" => vfs::file::vmo::read_only(String::from(version)),
                }
            },
        )?;

        self.outgoing_dir.add_entry("blob", vfs::remote::remote_dir(self.blobfs_proxy))?;

        download_and_apply_update(channel, version, self.omaha_config)
            .await
            .context("Installing OTA")?;

        Ok(())
    }
}

/// Run an OTA from a development host. Returns when the system and SSH keys have been installed.
pub async fn run_devhost_ota(
    cfg: DevhostConfig,
    out_dir: ServerEnd<fio::DirectoryMarker>,
) -> Result<(), Error> {
    // TODO(https://fxbug.dev/42064284, b/255340851): deduplicate this spinup code with the code in
    // ota_main.rs. To do that, we'll need to remove the run_devhost_ota call
    // from //src/recovery/system/src/main.rs and make run_*_ota public to only ota_main.rs.
    // Also, remove out_dir - ota_main.rs should provide an outgoing directory already spun up.
    let outgoing_dir_vfs = vfs::pseudo_directory! {};

    let scope = vfs::execution_scope::ExecutionScope::new();
    vfs::directory::serve_on(outgoing_dir_vfs.clone(), SERVE_FLAGS, scope.clone(), out_dir);
    fasync::Task::local(async move { scope.wait().await }).detach();

    let ota_env = OtaEnvBuilder::new(outgoing_dir_vfs)
        .devhost(cfg)
        .build()
        .await
        .context("Failed to create devhost OTA env")?;
    ota_env.do_ota("devhost", "20200101.1.1").await
}

/// Run an OTA against a TUF or Omaha server. Returns Ok after the system has successfully been installed.
pub async fn run_wellknown_ota(
    blobfs_proxy: fio::DirectoryProxy,
    outgoing_dir: Arc<Simple>,
) -> Result<(), Error> {
    let config =
        RecoveryUpdateConfig::resolve_update_config().await.context("Couldn't get config")?;
    let channel = config.channel;
    let version = config.version;

    match config.update_type {
        UpdateType::Tuf => {
            println!("recovery-ota: Creating TUF OTA environment");
            let ota_env = OtaEnvBuilder::new(outgoing_dir)
                .blobfs_proxy(blobfs_proxy)
                .build()
                .await
                .context("Failed to create OTA env")?;
            println!(
                "recovery-ota: Starting TUF OTA on channel '{}' against version '{}'",
                channel, version
            );
            ota_env.do_ota(&channel, &version).await
        }
        UpdateType::Omaha { app_id, service_url } => {
            println!("recovery-ota: Creating Omaha OTA environment");
            // Check for testing override
            println!(
                "recovery-ota: trying Omaha OTA on channel '{}' against version '{}', with service URL '{}' and app id '{}'",
                channel, version, service_url, app_id
            );

            let ota_env = OtaEnvBuilder::new(outgoing_dir)
                .omaha_config(OmahaConfig { app_id: app_id, server_url: service_url })
                .blobfs_proxy(blobfs_proxy)
                .build()
                .await
                .context("Failed to create OTA env");

            match ota_env {
                Ok(ref _ota_env) => {
                    println!("got no error while creating OTA env...")
                }
                Err(ref e) => {
                    eprintln!("got error while creating OTA env: {:?}", e)
                }
            }

            println!(
                "recovery-ota: Starting Omaha OTA on channel '{}' against version '{}'",
                channel, version
            );
            let res = ota_env?.do_ota(&channel, &version).await;
            println!("recovery-ota: OTA result: {:?}", res);
            res
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blobfs_ramdisk::BlobfsRamdisk;
    use fidl_fuchsia_pkg_ext::RepositoryKey;
    use fuchsia_async as fasync;
    use fuchsia_pkg_testing::serve::HttpResponder;
    use fuchsia_pkg_testing::{Package, PackageBuilder, RepositoryBuilder, make_epoch_json};
    use fuchsia_repo::body::Body;
    use fuchsia_runtime::{HandleType, take_startup_handle};
    use fuchsia_sync::Mutex;
    use futures::FutureExt as _;
    use futures::future::{BoxFuture, ready};
    use hyper::{Request, Response, StatusCode, header};
    use std::collections::{BTreeSet, HashMap};
    use url::Url;

    /// Wrapper around a ramdisk blobfs.
    struct FakeStorage {
        blobfs: BlobfsRamdisk,
    }

    impl FakeStorage {
        pub async fn new() -> Result<Self, Error> {
            let blobfs = BlobfsRamdisk::start().await.context("launching blobfs")?;
            Ok(FakeStorage { blobfs })
        }

        /// Get all the blobs inside the blobfs.
        pub fn list_blobs(&self) -> Result<BTreeSet<fuchsia_merkle::Hash>, Error> {
            self.blobfs.list_blobs()
        }

        /// Get the blobfs proxy.
        pub fn blobfs_root(&self) -> Result<fio::DirectoryProxy, Error> {
            self.blobfs.root_dir_proxy()
        }
    }

    /// This wraps a |FakeConfigHandler| in an |Arc|
    /// so that we can implement UriPathHandler for it.
    struct FakeConfigArc {
        pub arc: Arc<FakeConfigHandler>,
    }

    /// This class is used to provide the '/config.json' endpoint
    /// which the OTA process uses to discover information about the devhost repo.
    struct FakeConfigHandler {
        repo_keys: BTreeSet<RepositoryKey>,
        address: Mutex<String>,
    }

    impl FakeConfigHandler {
        pub fn new(repo_keys: BTreeSet<RepositoryKey>) -> Arc<Self> {
            Arc::new(FakeConfigHandler { repo_keys, address: Mutex::new("unknown".to_owned()) })
        }

        pub fn set_repo_address(self: Arc<Self>, addr: String) {
            let mut val = self.address.lock();
            *val = addr;
        }
    }

    impl HttpResponder for FakeConfigArc {
        fn respond(
            &self,
            request: &Request<Body>,
            response: Response<Body>,
        ) -> BoxFuture<'_, Response<Body>> {
            if request.uri().path() != "/config.json" {
                return ready(response).boxed();
            }

            // We don't expect any contention on this lock: we only need it
            // because the test doesn't know the address of the server until it's running.
            let val = self.arc.address.lock();
            if *val == "unknown" {
                panic!("Expected address to be set!");
            }
            let repo_url = match Url::parse(&*val) {
                Ok(u) => match u.host_str() {
                    Some(host) => format!("fuchsia-pkg://{}", host),
                    _ => "default".into(),
                },
                _ => panic!("Invalid address provided: {}", &*val),
            };

            // This emulates the format returned by `ffx repository serve` running on a devhost.
            let config = json!({
                "repo_url": repo_url,
                "root_version": "1",
                "root_threshold": "1",
                "root_keys": self.arc.repo_keys,
                "mirrors":[{
                    "mirror_url": &*val,
                    "subscribe": true
                }],
                "use_local_mirror": false,
                "repo_storage_type": "ephemeral",
            });

            let json_str = serde_json::to_string(&config).context("Serializing JSON").unwrap();
            let response = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, json_str.len())
                .body(Body::from(json_str))
                .unwrap();

            ready(response).boxed()
        }
    }

    const EMPTY_REPO_PATH: &str = "/pkg/empty-repo";
    const TEST_SSL_CERTS: &str = "/pkg/data/ssl";

    /// Represents an OTA that is yet to be run.
    struct TestOtaEnv {
        images: HashMap<String, Vec<u8>>,
        packages: Vec<Package>,
        storage: FakeStorage,
    }

    impl TestOtaEnv {
        pub async fn new() -> Result<Self, Error> {
            Ok(TestOtaEnv {
                images: HashMap::new(),
                packages: vec![],
                storage: FakeStorage::new().await.context("Starting fake storage")?,
            })
        }

        /// Add a package to be installed by this OTA.
        pub fn add_package(mut self, p: Package) -> Self {
            self.packages.push(p);
            self
        }

        /// Add an image to include in the update package for this OTA.
        pub fn add_image(mut self, name: &str, data: &str) -> Self {
            self.images.insert(name.to_owned(), data.to_owned().into_bytes());
            self
        }

        /// Generates the packages.json file for the update package.
        fn generate_packages_list(&self) -> String {
            let package_urls: Vec<String> = self
                .packages
                .iter()
                .map(|p| format!("fuchsia-pkg://fuchsia.com/{}/0?hash={}", p.name(), p.hash()))
                .collect();
            let packages = json!({
                "version": 1,
                "content": package_urls,
            });
            serde_json::to_string(&packages).unwrap()
        }

        /// Build an update package from the list of packages and images included
        /// in this update.
        async fn make_update_package(&self) -> Result<Package, Error> {
            let mut update = PackageBuilder::new("update")
                .add_resource_at("packages.json", self.generate_packages_list().as_bytes());

            for (name, data) in self.images.iter() {
                update = update.add_resource_at(name, data.as_slice());
            }

            update.build().await.context("Building update package")
        }

        /// Run the OTA.
        pub async fn run_ota(&mut self) -> Result<(), Error> {
            let update = self.make_update_package().await?;
            // Create the repo.
            let repo = Arc::new(
                self.packages
                    .iter()
                    .fold(
                        RepositoryBuilder::from_template_dir(EMPTY_REPO_PATH).add_package(&update),
                        |repo, package| repo.add_package(package),
                    )
                    .build()
                    .await
                    .context("Building repo")?,
            );
            // We expect the update package to be in blobfs, so add it to the list of packages.
            self.packages.push(update);

            // Add a hook to handle the config.json file, which is exposed by
            // `pm serve` to enable autoconfiguration of repositories.
            let request_handler = FakeConfigHandler::new(repo.root_keys());
            let served_repo = Arc::clone(&repo)
                .server()
                .response_overrider(FakeConfigArc { arc: Arc::clone(&request_handler) })
                .start()
                .context("Starting repository")?;

            // Configure the address of the repository for config.json
            let url = served_repo.local_url();
            let config_url = format!("{}/config.json", url);
            request_handler.set_repo_address(url);

            let cfg = DevhostConfig { url: config_url };

            let directory_handle = take_startup_handle(HandleType::DirectoryRequest.into())
                .expect("cannot take startup handle");
            let outgoing_dir = zx::Channel::from(directory_handle);
            let outgoing_dir_vfs = vfs::pseudo_directory! {};

            let scope = vfs::execution_scope::ExecutionScope::new();
            vfs::directory::serve_on(
                outgoing_dir_vfs.clone(),
                SERVE_FLAGS,
                scope.clone(),
                outgoing_dir.into(),
            );
            fasync::Task::local(async move { scope.wait().await }).detach();

            let blobfs_proxy = self.storage.blobfs_root()?;

            // Build the environment, and do the OTA.
            let ota_env = OtaEnvBuilder::new(outgoing_dir_vfs)
                .board_name("x64")
                .blobfs_proxy(blobfs_proxy)
                .ssl_certificates(TEST_SSL_CERTS)
                .devhost(cfg)
                .build()
                .await
                .context("Building environment")?;

            ota_env.do_ota("devhost", "20240101.1.1").await.context("Running OTA")?;
            Ok(())
        }

        /// Check that the blobfs contains exactly the blobs we expect it to contain.
        pub async fn check_blobs(&self) {
            let written_blobs = self.storage.list_blobs().expect("Listing blobfs blobs");
            let mut all_package_blobs = BTreeSet::new();
            for package in self.packages.iter() {
                all_package_blobs.append(&mut package.list_blobs());
            }

            assert_eq!(written_blobs, all_package_blobs);
        }
    }

    #[ignore] //TODO(https://fxbug.dev/42053153) Move to integration test
    #[fuchsia::test]
    async fn test_run_devhost_ota() -> Result<(), Error> {
        let package = PackageBuilder::new("test-package")
            .add_resource_at("data/file1", "Hello, world!".as_bytes())
            .build()
            .await
            .unwrap();
        let mut env = TestOtaEnv::new()
            .await?
            .add_package(package)
            .add_image("zbi.signed", "zbi image")
            .add_image("fuchsia.vbmeta", "fuchsia vbmeta")
            .add_image("epoch.json", &make_epoch_json(1));

        env.run_ota().await?;
        env.check_blobs().await;
        Ok(())
    }
}
