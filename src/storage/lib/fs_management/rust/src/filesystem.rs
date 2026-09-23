// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Contains the asynchronous version of [`Filesystem`][`crate::Filesystem`].

use crate::error::{QueryError, ShutdownError};
use crate::{ComponentType, FSConfig, Options};
use anyhow::{Context, Error, anyhow, bail, ensure};
use fidl::endpoints::{ClientEnd, ServerEnd, create_endpoints, create_proxy};
use fidl_fuchsia_component::{self as fcomponent, RealmMarker};
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_fs::AdminMarker;
use fidl_fuchsia_fs_startup::{
    CheckOptions, CreateOptions, MountOptions, StartupMarker, VolumesMarker,
};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_storage_block::{self as fblock, BlockMarker};
use fuchsia_component_client::{
    connect_to_named_protocol_at_dir_root, connect_to_protocol, connect_to_protocol_at_dir_root,
    connect_to_protocol_at_dir_svc, open_childs_exposed_directory,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use zx::Status;

/// Creates new connections to an instance of fuchsia.hardware.block.Block and similar protocols
/// (Volume, Partition).
///
/// NOTE: It is important to understand the difference between `BlockConnector` and the actual
/// protocols (e.g. a `ClientEnd<BlockMarker>` or `BlockProxy`): `BlockConnector` is used to *create
/// new connections* to a Block.
///
/// It is not possible to directly convert a `ClientEnd<BlockMarker>` (or `BlockProxy`) into a
/// `BlockConnector`, because Block is not cloneable.  To implement `BlockConnector`, you will need
/// a way to generate new connections to a Block instance.  A few common implementations are
/// provided below.
pub trait BlockConnector: Send + Sync {
    fn connect_channel_to_block(&self, server_end: ServerEnd<BlockMarker>) -> Result<(), Error>;
    fn connect_block(&self) -> Result<ClientEnd<BlockMarker>, Error> {
        let (client, server) = fidl::endpoints::create_endpoints();
        self.connect_channel_to_block(server)?;
        Ok(client)
    }
}

/// Implements `BlockConnector` via a service dir.  Wraps `connect_to_named_protocol_at_dir_root`.
#[derive(Clone, Debug)]
pub struct DirBasedBlockConnector(fio::DirectoryProxy, String);

impl DirBasedBlockConnector {
    /// Creates a new [`DirBasedBlockConnector`].  It is expected that `path` within `dir` hosts the
    /// Volume protocol.
    pub fn new(dir: fio::DirectoryProxy, path: String) -> Self {
        Self(dir, path)
    }

    pub fn dir(&self) -> &fio::DirectoryProxy {
        &self.0
    }

    /// Returns the path relative to the directory which hosts the volume protocol.
    pub fn path(&self) -> &str {
        &self.1
    }
}

impl BlockConnector for DirBasedBlockConnector {
    fn connect_channel_to_block(&self, server_end: ServerEnd<BlockMarker>) -> Result<(), Error> {
        self.0.open(
            self.path(),
            fio::Flags::PROTOCOL_SERVICE,
            &fio::Options::default(),
            server_end.into_channel(),
        )?;
        Ok(())
    }
}

impl BlockConnector for fidl_fuchsia_device::ControllerProxy {
    fn connect_channel_to_block(&self, server_end: ServerEnd<BlockMarker>) -> Result<(), Error> {
        let () = self.connect_to_device_fidl(server_end.into_channel())?;
        Ok(())
    }
}

impl BlockConnector for fidl_fuchsia_storage_partitions::PartitionServiceProxy {
    fn connect_channel_to_block(&self, server_end: ServerEnd<BlockMarker>) -> Result<(), Error> {
        self.connect_channel_to_volume(server_end)?;
        Ok(())
    }
}

// NB: We have to be specific here; we cannot do a blanket impl for AsRef<T: BlockConnector> because
// that would conflict with a downstream crate that implements AsRef for a concrete BlockConnector
// defined here already.
impl<T: BlockConnector> BlockConnector for Arc<T> {
    fn connect_channel_to_block(&self, server_end: ServerEnd<BlockMarker>) -> Result<(), Error> {
        self.as_ref().connect_channel_to_block(server_end)
    }
}

impl<F> BlockConnector for F
where
    F: Fn(ServerEnd<BlockMarker>) -> Result<(), Error> + Send + Sync,
{
    fn connect_channel_to_block(&self, server_end: ServerEnd<BlockMarker>) -> Result<(), Error> {
        self(server_end)
    }
}

/// Asynchronously manages a block device for filesystem operations.
pub struct Filesystem {
    /// The filesystem struct keeps the FSConfig in a Box<dyn> instead of holding it directly for
    /// code size reasons. Using a type parameter instead would make monomorphized versions of the
    /// Filesystem impl block for each filesystem type, which duplicates several multi-kilobyte
    /// functions (get_component_exposed_dir and serve in particular) that are otherwise quite
    /// generic over config. Clients that want to be generic over filesystem type also pay the
    /// monomorphization cost, with some, like fshost, paying a lot.
    config: Box<dyn FSConfig>,
    block_connector: Box<dyn BlockConnector>,
    component: Option<Arc<DynamicComponentInstance>>,
}

// Used to disambiguate children in our component collection.
static COLLECTION_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Filesystem {
    pub fn config(&self) -> &dyn FSConfig {
        self.config.as_ref()
    }

    pub fn into_config(self) -> Box<dyn FSConfig> {
        self.config
    }

    /// Creates a new `Filesystem`.
    pub fn new<B: BlockConnector + 'static, FSC: FSConfig>(
        block_connector: B,
        config: FSC,
    ) -> Self {
        Self::from_boxed_config(Box::new(block_connector), Box::new(config))
    }

    /// Creates a new `Filesystem`.
    pub fn from_boxed_config(
        block_connector: Box<dyn BlockConnector>,
        config: Box<dyn FSConfig>,
    ) -> Self {
        Self { config, block_connector, component: None }
    }

    /// Returns the (relative) moniker of the filesystem component. This will start the component
    /// instance if it is not running.
    pub async fn get_component_moniker(&mut self) -> Result<String, Error> {
        let _ = self.get_component_exposed_dir().await?;
        Ok(match self.config.options().component_type {
            ComponentType::StaticChild => self.config.options().component_name.to_string(),
            ComponentType::DynamicChild { .. } => {
                let component = self.component.as_ref().unwrap();
                format!("{}:{}", component.collection, component.name)
            }
        })
    }

    async fn get_component_exposed_dir(&mut self) -> Result<fio::DirectoryProxy, Error> {
        let options = self.config.options();
        let component_name = options.component_name;
        match options.component_type {
            ComponentType::StaticChild => open_childs_exposed_directory(component_name, None).await,
            ComponentType::DynamicChild { collection_name } => {
                if let Some(component) = &self.component {
                    return open_childs_exposed_directory(
                        component.name.clone(),
                        Some(component.collection.clone()),
                    )
                    .await;
                }

                // We need a unique name, so we pull in the process Koid here since it's possible
                // for the same binary in a component to be launched multiple times and we don't
                // want to collide with children created by other processes.
                let name = format!(
                    "{}-{}-{}",
                    component_name,
                    fuchsia_runtime::process_self().koid().unwrap().raw_koid(),
                    COLLECTION_COUNTER.fetch_add(1, Ordering::Relaxed)
                );

                let collection_ref = fdecl::CollectionRef { name: collection_name };
                let child_decls = vec![
                    fdecl::Child {
                        name: Some(format!("{}-relative", name)),
                        url: Some(format!("#meta/{}.cm", component_name)),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                    fdecl::Child {
                        name: Some(name),
                        url: Some(format!(
                            "fuchsia-boot:///{}#meta/{}.cm",
                            component_name, component_name
                        )),
                        startup: Some(fdecl::StartupMode::Lazy),
                        ..Default::default()
                    },
                ];
                let realm_proxy = connect_to_protocol::<RealmMarker>()?;
                for child_decl in child_decls {
                    // Launch a new component in our collection.
                    realm_proxy
                        .create_child(
                            &collection_ref,
                            &child_decl,
                            fcomponent::CreateChildArgs::default(),
                        )
                        .await?
                        .map_err(|e| anyhow!("create_child failed: {:?}", e))?;

                    let component = Arc::new(DynamicComponentInstance {
                        name: child_decl.name.unwrap(),
                        collection: collection_ref.name.clone(),
                        should_not_drop: AtomicBool::new(false),
                    });

                    if let Ok(proxy) = open_childs_exposed_directory(
                        component.name.clone(),
                        Some(component.collection.clone()),
                    )
                    .await
                    {
                        self.component = Some(component);
                        return Ok(proxy);
                    }
                }
                Err(anyhow!("Failed to open exposed directory"))
            }
        }
    }

    /// Calls fuchsia.fs.startup/Startup.Format on the configured filesystem component.
    ///
    /// Which component is used and the options passed to it are controlled by the config this
    /// `Filesystem` was created with.
    ///
    /// See [`FSConfig`].
    ///
    /// # Errors
    ///
    /// Returns any errors from the Format method. Also returns an error if the startup protocol is
    /// not found, if it couldn't launch or find the filesystem component, or if it couldn't get
    /// the block device channel.
    pub async fn format(&mut self) -> Result<(), Error> {
        let channel = self.block_connector.connect_block()?;

        let exposed_dir = self.get_component_exposed_dir().await?;
        let proxy = connect_to_protocol_at_dir_root::<StartupMarker>(&exposed_dir)?;
        proxy
            .format(channel, &self.config().options().format_options)
            .await?
            .map_err(Status::err_from_raw)?;

        Ok(())
    }

    /// Calls fuchsia.fs.startup/Startup.Check on the configured filesystem component.
    ///
    /// Which component is used and the options passed to it are controlled by the config this
    /// `Filesystem` was created with.
    ///
    /// See [`FSConfig`].
    ///
    /// # Errors
    ///
    /// Returns any errors from the Check method. Also returns an error if the startup protocol is
    /// not found, if it couldn't launch or find the filesystem component, or if it couldn't get
    /// the block device channel.
    pub async fn fsck(&mut self) -> Result<(), Error> {
        let channel = self.block_connector.connect_block()?;
        let exposed_dir = self.get_component_exposed_dir().await?;
        let proxy = connect_to_protocol_at_dir_root::<StartupMarker>(&exposed_dir)?;
        proxy.check(channel, CheckOptions::default()).await?.map_err(Status::err_from_raw)?;
        Ok(())
    }

    /// Serves the filesystem on the block device and returns a [`ServingSingleVolumeFilesystem`]
    /// representing the running filesystem component.
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if serving the filesystem failed.
    pub async fn serve(mut self) -> Result<ServingSingleVolumeFilesystem, Error> {
        if self.config.is_multi_volume() {
            bail!("Can't serve a multivolume filesystem; use serve_multi_volume");
        }
        let Options { start_options, reuse_component_after_serving, .. } = self.config.options();

        let exposed_dir = self.get_component_exposed_dir().await?;
        let proxy = connect_to_protocol_at_dir_root::<StartupMarker>(&exposed_dir)?;
        proxy
            .start(self.block_connector.connect_block()?, &start_options)
            .await?
            .map_err(Status::err_from_raw)?;

        let (root_dir, server_end) = create_endpoints::<fio::NodeMarker>();
        exposed_dir.open(
            "root",
            fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE | fio::Flags::PERM_INHERIT_EXECUTE,
            &Default::default(),
            server_end.into_channel(),
        )?;
        let component = self.component.clone();
        if !reuse_component_after_serving {
            self.component = None;
        }
        Ok(ServingSingleVolumeFilesystem {
            component,
            exposed_dir: Some(exposed_dir),
            root_dir: ClientEnd::<fio::DirectoryMarker>::new(root_dir.into_channel()).into_proxy(),
            binding: None,
        })
    }

    /// Serves the filesystem on the block device and returns a [`ServingMultiVolumeFilesystem`]
    /// representing the running filesystem component.  No volumes are opened; clients have to do
    /// that explicitly.
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if serving the filesystem failed.
    pub async fn serve_multi_volume(mut self) -> Result<ServingMultiVolumeFilesystem, Error> {
        if !self.config.is_multi_volume() {
            bail!("Can't serve_multi_volume a single-volume filesystem; use serve");
        }

        let exposed_dir = self.get_component_exposed_dir().await?;
        let proxy = connect_to_protocol_at_dir_root::<StartupMarker>(&exposed_dir)?;
        proxy
            .start(self.block_connector.connect_block()?, &self.config.options().start_options)
            .await?
            .map_err(Status::err_from_raw)?;

        Ok(ServingMultiVolumeFilesystem {
            component: self.component,
            exposed_dir: Some(exposed_dir),
        })
    }
}

// Destroys the child when dropped.
struct DynamicComponentInstance {
    name: String,
    collection: String,
    should_not_drop: AtomicBool,
}

impl DynamicComponentInstance {
    fn forget(&self) {
        self.should_not_drop.store(true, Ordering::Relaxed);
    }
}

impl Drop for DynamicComponentInstance {
    fn drop(&mut self) {
        if self.should_not_drop.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(realm_proxy) = connect_to_protocol::<RealmMarker>() {
            let _ = realm_proxy.destroy_child(&fdecl::ChildRef {
                name: self.name.clone(),
                collection: Some(self.collection.clone()),
            });
        }
    }
}

/// Manages the binding of a `fuchsia_io::DirectoryProxy` into the local namespace.  When the object
/// is dropped, the binding is removed.
#[derive(Default)]
pub struct NamespaceBinding(String);

impl NamespaceBinding {
    pub fn create(root_dir: &fio::DirectoryProxy, path: String) -> Result<NamespaceBinding, Error> {
        let (client_end, server_end) = create_endpoints();
        root_dir.clone(ServerEnd::new(server_end.into_channel()))?;
        let namespace = fdio::Namespace::installed()?;
        namespace.bind(&path, client_end)?;
        Ok(Self(path))
    }
}

impl std::ops::Deref for NamespaceBinding {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for NamespaceBinding {
    fn drop(&mut self) {
        if let Ok(namespace) = fdio::Namespace::installed() {
            let _ = namespace.unbind(&self.0);
        }
    }
}

// TODO(https://fxbug.dev/42174810): Soft migration; remove this after completion
pub type ServingFilesystem = ServingSingleVolumeFilesystem;

/// Asynchronously manages a serving filesystem. Created from [`Filesystem::serve()`].
pub struct ServingSingleVolumeFilesystem {
    component: Option<Arc<DynamicComponentInstance>>,
    // exposed_dir will always be Some, except when the filesystem is shutting down.
    exposed_dir: Option<fio::DirectoryProxy>,
    root_dir: fio::DirectoryProxy,

    // The path in the local namespace that this filesystem is bound to (optional).
    binding: Option<NamespaceBinding>,
}

impl ServingSingleVolumeFilesystem {
    /// Returns a proxy to the exposed directory of the serving filesystem.
    pub fn exposed_dir(&self) -> &fio::DirectoryProxy {
        self.exposed_dir.as_ref().unwrap()
    }

    /// Returns a proxy to the root directory of the serving filesystem.
    pub fn root(&self) -> &fio::DirectoryProxy {
        &self.root_dir
    }

    /// Binds the root directory being served by this filesystem to a path in the local namespace.
    /// The path must be absolute, containing no "." nor ".." entries.  The binding will be dropped
    /// when self is dropped.  Only one binding is supported.
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if binding failed.
    pub fn bind_to_path(&mut self, path: &str) -> Result<(), Error> {
        ensure!(self.binding.is_none(), "Already bound");
        self.binding = Some(NamespaceBinding::create(&self.root_dir, path.to_string())?);
        Ok(())
    }

    pub fn bound_path(&self) -> Option<&str> {
        self.binding.as_deref()
    }

    /// Returns a [`FilesystemInfo`] object containing information about the serving filesystem.
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if querying the filesystem failed.
    pub async fn query(&self) -> Result<Box<fio::FilesystemInfo>, QueryError> {
        let (status, info) = self.root_dir.query_filesystem().await?;
        Status::ok(status).map_err(QueryError::DirectoryQuery)?;
        info.ok_or(QueryError::DirectoryEmptyResult)
    }

    /// Take the exposed dir from this filesystem instance, dropping the management struct without
    /// shutting the filesystem down. This leaves the caller with the responsibility of shutting
    /// down the filesystem, and the filesystem component if necessary.
    pub fn take_exposed_dir(mut self) -> fio::DirectoryProxy {
        self.component.take().expect("BUG: component missing").forget();
        self.exposed_dir.take().expect("BUG: exposed dir missing")
    }

    /// Attempts to shutdown the filesystem using the
    /// [`fidl_fuchsia_fs::AdminProxy::shutdown()`] FIDL method and waiting for the filesystem
    /// process to terminate.
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if the shutdown failed or the filesystem process did not terminate.
    pub async fn shutdown(mut self) -> Result<(), ShutdownError> {
        connect_to_protocol_at_dir_root::<fidl_fuchsia_fs::AdminMarker>(
            &self.exposed_dir.take().expect("BUG: exposed dir missing"),
        )?
        .shutdown()
        .await?;
        Ok(())
    }

    /// Attempts to kill the filesystem process and waits for the process to terminate.
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if the filesystem process could not be terminated. There is no way to
    /// recover the [`Filesystem`] from this error.
    pub async fn kill(self) -> Result<(), Error> {
        // For components, just shut down the filesystem.
        // TODO(https://fxbug.dev/293949323): Figure out a way to make this more abrupt - the use-cases are
        // either testing or when the filesystem isn't responding.
        self.shutdown().await?;
        Ok(())
    }
}

impl Drop for ServingSingleVolumeFilesystem {
    fn drop(&mut self) {
        // Make a best effort attempt to shut down to the filesystem, if we need to.
        if let Some(exposed_dir) = self.exposed_dir.take() {
            if let Ok(proxy) =
                connect_to_protocol_at_dir_root::<fidl_fuchsia_fs::AdminMarker>(&exposed_dir)
            {
                let _ = proxy.shutdown();
            }
        }
    }
}

/// Asynchronously manages a serving multivolume filesystem. Created from
/// [`Filesystem::serve_multi_volume()`].
pub struct ServingMultiVolumeFilesystem {
    component: Option<Arc<DynamicComponentInstance>>,
    // exposed_dir will always be Some, except in Self::shutdown.
    exposed_dir: Option<fio::DirectoryProxy>,
}

/// Represents an opened volume in a [`ServingMultiVolumeFilesystem'] instance.
pub struct ServingVolume {
    root_dir: fio::DirectoryProxy,
    binding: Option<NamespaceBinding>,
    exposed_dir: fio::DirectoryProxy,
}

impl ServingVolume {
    fn new(exposed_dir: fio::DirectoryProxy) -> Result<Self, Error> {
        let (root_dir, server_end) = create_endpoints::<fio::NodeMarker>();
        exposed_dir.open(
            "root",
            fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE | fio::Flags::PERM_INHERIT_EXECUTE,
            &Default::default(),
            server_end.into_channel(),
        )?;
        Ok(ServingVolume {
            root_dir: ClientEnd::<fio::DirectoryMarker>::new(root_dir.into_channel()).into_proxy(),
            binding: None,
            exposed_dir,
        })
    }

    /// Returns a proxy to the root directory of the serving volume.
    pub fn root(&self) -> &fio::DirectoryProxy {
        &self.root_dir
    }

    /// Returns a proxy to the exposed directory of the serving volume.
    pub fn exposed_dir(&self) -> &fio::DirectoryProxy {
        &self.exposed_dir
    }

    /// Binds the root directory being served by this filesystem to a path in the local namespace.
    /// The path must be absolute, containing no "." nor ".." entries.  The binding will be dropped
    /// when self is dropped, or when unbind_path is called.  Only one binding is supported.
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if binding failed, or if a binding already exists.
    pub fn bind_to_path(&mut self, path: &str) -> Result<(), Error> {
        ensure!(self.binding.is_none(), "Already bound");
        self.binding = Some(NamespaceBinding::create(&self.root_dir, path.to_string())?);
        Ok(())
    }

    /// Remove the namespace binding to the root directory being served by this volume, if there is
    /// one. If there is no binding, this function does nothing. After this, it is safe to call
    /// bind_to_path again.
    pub fn unbind_path(&mut self) {
        let _ = self.binding.take();
    }

    pub fn bound_path(&self) -> Option<&str> {
        self.binding.as_deref()
    }

    /// Returns a [`FilesystemInfo`] object containing information about the serving volume.
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if querying the filesystem failed.
    pub async fn query(&self) -> Result<Box<fio::FilesystemInfo>, QueryError> {
        let (status, info) = self.root_dir.query_filesystem().await?;
        Status::ok(status).map_err(QueryError::DirectoryQuery)?;
        info.ok_or(QueryError::DirectoryEmptyResult)
    }

    /// Attempts to shutdown the filesystem using the [`fidl_fuchsia_fs::AdminProxy::shutdown()`]
    /// FIDL method. Fails if the volume is not already open.
    pub async fn shutdown(self) -> Result<(), Error> {
        let admin_proxy = connect_to_protocol_at_dir_svc::<AdminMarker>(self.exposed_dir())?;
        admin_proxy.shutdown().await.context("failed to shutdown volume")?;
        Ok(())
    }
}

impl ServingMultiVolumeFilesystem {
    /// Returns whether the given volume exists.
    pub async fn has_volume(&self, volume: &str) -> Result<bool, Error> {
        let path = format!("volumes/{}", volume);
        fuchsia_fs::directory::open_node(self.exposed_dir(), &path, fio::Flags::PROTOCOL_NODE)
            .await
            .map(|_| true)
            .or_else(|e| {
                if let fuchsia_fs::node::OpenError::OpenError(status) = &e {
                    if *status == zx::Status::NOT_FOUND {
                        return Ok(false);
                    }
                }
                Err(e.into())
            })
    }

    /// Creates and mounts the volume.  Fails if the volume already exists.
    /// If `options.crypt` is set, the volume will be encrypted using the provided Crypt instance.
    /// If `options.as_blob` is set, creates a blob volume that is mounted as a blob filesystem.
    pub async fn create_volume(
        &self,
        volume: &str,
        create_options: CreateOptions,
        options: MountOptions,
    ) -> Result<ServingVolume, Error> {
        let (exposed_dir, server) = create_proxy::<fio::DirectoryMarker>();
        connect_to_protocol_at_dir_root::<VolumesMarker>(self.exposed_dir())?
            .create(volume, server, create_options, options)
            .await?
            .map_err(|e| anyhow!(zx::Status::err_from_raw(e)))?;
        ServingVolume::new(exposed_dir)
    }

    /// Deletes the volume. Fails if the volume is already mounted.
    pub async fn remove_volume(&self, volume: &str) -> Result<(), Error> {
        connect_to_protocol_at_dir_root::<VolumesMarker>(self.exposed_dir())?
            .remove(volume)
            .await?
            .map_err(|e| anyhow!(zx::Status::err_from_raw(e)))
    }

    /// Mounts an existing volume.  Fails if the volume is already mounted or doesn't exist.
    /// If `crypt` is set, the volume will be decrypted using the provided Crypt instance.
    pub async fn open_volume(
        &self,
        volume: &str,
        options: MountOptions,
    ) -> Result<ServingVolume, Error> {
        let (exposed_dir, server) = create_proxy::<fio::DirectoryMarker>();
        let path = format!("volumes/{}", volume);
        connect_to_named_protocol_at_dir_root::<fidl_fuchsia_fs_startup::VolumeMarker>(
            self.exposed_dir(),
            &path,
        )?
        .mount(server, options)
        .await?
        .map_err(|e| anyhow!(zx::Status::err_from_raw(e)))?;

        ServingVolume::new(exposed_dir)
    }

    /// Returns volume info for `volume`.
    pub async fn get_volume_info(
        &self,
        volume: &str,
    ) -> Result<fidl_fuchsia_fs_startup::VolumeInfo, Error> {
        let path = format!("volumes/{}", volume);
        connect_to_named_protocol_at_dir_root::<fidl_fuchsia_fs_startup::VolumeMarker>(
            self.exposed_dir(),
            &path,
        )?
        .get_info()
        .await?
        .map_err(|e| anyhow!(zx::Status::err_from_raw(e)))
    }

    /// Sets the max byte limit for a volume. Fails if the volume is not mounted.
    pub async fn set_byte_limit(&self, volume: &str, byte_limit: u64) -> Result<(), Error> {
        if byte_limit == 0 {
            return Ok(());
        }
        let path = format!("volumes/{}", volume);
        connect_to_named_protocol_at_dir_root::<fidl_fuchsia_fs_startup::VolumeMarker>(
            self.exposed_dir(),
            &path,
        )?
        .set_limit(byte_limit)
        .await?
        .map_err(|e| anyhow!(zx::Status::err_from_raw(e)))
    }

    pub async fn check_volume(&self, volume: &str, options: CheckOptions) -> Result<(), Error> {
        let path = format!("volumes/{}", volume);
        connect_to_named_protocol_at_dir_root::<fidl_fuchsia_fs_startup::VolumeMarker>(
            self.exposed_dir(),
            &path,
        )?
        .check(options)
        .await?
        .map_err(|e| anyhow!(zx::Status::err_from_raw(e)))?;
        Ok(())
    }

    /// Provides access to the internal |exposed_dir| for use in testing
    /// callsites which need directory access.
    pub fn exposed_dir(&self) -> &fio::DirectoryProxy {
        self.exposed_dir.as_ref().expect("BUG: exposed dir missing")
    }

    /// Attempts to shutdown the filesystem using the [`fidl_fuchsia_fs::AdminProxy::shutdown()`]
    /// FIDL method.
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if the shutdown failed.
    pub async fn shutdown(mut self) -> Result<(), ShutdownError> {
        connect_to_protocol_at_dir_root::<fidl_fuchsia_fs::AdminMarker>(
            // Take exposed_dir so we don't attempt to shut down again in Drop.
            &self.exposed_dir.take().expect("BUG: exposed dir missing"),
        )?
        .shutdown()
        .await?;
        Ok(())
    }

    /// Take the exposed dir from this filesystem instance, dropping the management struct without
    /// shutting the filesystem down. This leaves the caller with the responsibility of shutting
    /// down the filesystem, and the filesystem component if necessary.
    pub fn take_exposed_dir(mut self) -> fio::DirectoryProxy {
        self.component.take().expect("BUG: missing component").forget();
        self.exposed_dir.take().expect("BUG: exposed dir missing")
    }

    /// Returns a list of volumes found in the filesystem.
    pub async fn list_volumes(&self) -> Result<Vec<String>, Error> {
        let volumes_dir = fuchsia_fs::directory::open_async::<fio::DirectoryMarker>(
            self.exposed_dir(),
            "volumes",
            fio::PERM_READABLE,
        )
        .unwrap();
        fuchsia_fs::directory::readdir(&volumes_dir)
            .await
            .map(|entries| entries.into_iter().map(|e| e.name).collect())
            .map_err(|e| anyhow!("failed to read volumes dir: {}", e))
    }

    /// Returns the volume manager information.
    pub async fn get_info(&self) -> Result<fblock::VolumeManagerInfo, Error> {
        Ok(*connect_to_protocol_at_dir_root::<VolumesMarker>(self.exposed_dir())?
            .get_info()
            .await?
            .map_err(|e| anyhow!(zx::Status::err_from_raw(e)))?
            .ok_or_else(|| anyhow!("Missing info"))?)
    }
}

impl Drop for ServingMultiVolumeFilesystem {
    fn drop(&mut self) {
        if let Some(exposed_dir) = self.exposed_dir.take() {
            // Make a best effort attempt to shut down to the filesystem.
            if let Ok(proxy) =
                connect_to_protocol_at_dir_root::<fidl_fuchsia_fs::AdminMarker>(&exposed_dir)
            {
                let _ = proxy.shutdown();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Blobfs, F2fs, Fxfs, Minfs};
    use delivery_blob::{CompressionMode, Type1Blob};
    use fidl::endpoints::DiscoverableProtocolMarker;
    use fidl_fuchsia_fxfs::{BlobCreatorMarker, BlobReaderMarker};
    use ramdevice_client::RamdiskClient;
    use std::io::{Read as _, Write as _};

    async fn ramdisk(block_size: u64) -> RamdiskClient {
        RamdiskClient::create(block_size, 1 << 16).await.unwrap()
    }

    async fn new_fs<FSC: FSConfig>(ramdisk: &RamdiskClient, config: FSC) -> Filesystem {
        let block_dir = fuchsia_fs::directory::clone(ramdisk.outgoing()).unwrap();
        let block_connector =
            DirBasedBlockConnector::new(block_dir, format!("svc/{}", BlockMarker::PROTOCOL_NAME));
        Filesystem::new(block_connector, config)
    }

    #[fuchsia::test]
    async fn blobfs_custom_config() {
        let block_size = 512;
        let ramdisk = ramdisk(block_size).await;
        let config = Blobfs { verbose: true, readonly: true, ..Default::default() };
        let mut blobfs = new_fs(&ramdisk, config).await;

        blobfs.format().await.expect("failed to format blobfs");
        blobfs.fsck().await.expect("failed to fsck blobfs");
        let serving = blobfs.serve().await.expect("failed to serve blobfs");
        serving.shutdown().await.expect("failed to shutdown blobfs");
    }

    #[fuchsia::test]
    async fn blobfs_format_fsck_success() {
        let block_size = 512;
        let ramdisk = ramdisk(block_size).await;
        let mut blobfs = new_fs(&ramdisk, Blobfs::default()).await;

        blobfs.format().await.expect("failed to format blobfs");
        blobfs.fsck().await.expect("failed to fsck blobfs");
    }

    #[fuchsia::test]
    async fn blobfs_format_serve_write_query_restart_read_shutdown() {
        let block_size = 512;
        let ramdisk = ramdisk(block_size).await;
        let mut blobfs = new_fs(&ramdisk, Blobfs::default()).await;

        blobfs.format().await.expect("failed to format blobfs");

        let serving = blobfs.serve().await.expect("failed to serve blobfs the first time");

        // snapshot of FilesystemInfo
        let fs_info1 =
            serving.query().await.expect("failed to query filesystem info after first serving");

        // pre-generated merkle test fixture data
        let content = b"test content";
        let merkle = fuchsia_merkle::root_from_slice(content);
        let delivery_blob = Type1Blob::generate(content, CompressionMode::Never);

        {
            let creator = fuchsia_component_client::connect_to_protocol_at_dir_root::<
                BlobCreatorMarker,
            >(serving.exposed_dir())
            .unwrap();
            let writer = creator.create(&merkle.into(), false).await.unwrap().unwrap();
            let mut writer =
                blob_writer::BlobWriter::create(writer.into_proxy(), delivery_blob.len() as u64)
                    .await
                    .unwrap();
            writer.write(&delivery_blob).await.unwrap();
        }

        // check against the snapshot FilesystemInfo
        let fs_info2 = serving.query().await.expect("failed to query filesystem info after write");
        assert_eq!(
            fs_info2.used_bytes - fs_info1.used_bytes,
            fs_info2.block_size as u64 // assuming content < 8K
        );

        serving.shutdown().await.expect("failed to shutdown blobfs the first time");
        let blobfs = new_fs(&ramdisk, Blobfs::default()).await;
        let serving = blobfs.serve().await.expect("failed to serve blobfs the second time");
        {
            let reader = fuchsia_component_client::connect_to_protocol_at_dir_root::<
                BlobReaderMarker,
            >(serving.exposed_dir())
            .unwrap();
            let vmo = reader.get_vmo(&merkle.into()).await.unwrap().unwrap();
            let read_content = vmo.read_to_vec::<u8>(0, content.len() as u64).unwrap();
            assert_eq!(read_content, content);
        }

        // once more check against the snapshot FilesystemInfo
        let fs_info3 = serving.query().await.expect("failed to query filesystem info after read");
        assert_eq!(
            fs_info3.used_bytes - fs_info1.used_bytes,
            fs_info3.block_size as u64 // assuming content < 8K
        );

        serving.shutdown().await.expect("failed to shutdown blobfs the second time");
    }

    #[fuchsia::test]
    async fn blobfs_bind_to_path() {
        let block_size = 512;
        let test_content = b"test content";
        let merkle = fuchsia_merkle::root_from_slice(test_content);
        let delivery_blob = Type1Blob::generate(test_content, CompressionMode::Never);
        let ramdisk = ramdisk(block_size).await;
        let mut blobfs = new_fs(&ramdisk, Blobfs::default()).await;

        blobfs.format().await.expect("failed to format blobfs");
        let mut serving = blobfs.serve().await.expect("failed to serve blobfs");
        serving.bind_to_path("/test-blobfs-path").expect("bind_to_path failed");

        {
            let creator = fuchsia_component_client::connect_to_protocol_at_dir_root::<
                BlobCreatorMarker,
            >(serving.exposed_dir())
            .unwrap();
            let writer = creator.create(&merkle.into(), false).await.unwrap().unwrap();
            let mut writer =
                blob_writer::BlobWriter::create(writer.into_proxy(), delivery_blob.len() as u64)
                    .await
                    .unwrap();
            writer.write(&delivery_blob).await.unwrap();
        }

        let entries = std::fs::read_dir("/test-blobfs-path")
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(entries, &[merkle.to_string()]);

        serving.shutdown().await.expect("failed to shutdown blobfs");
    }

    #[fuchsia::test]
    async fn minfs_custom_config() {
        let block_size = 512;
        let ramdisk = ramdisk(block_size).await;
        let config = Minfs {
            verbose: true,
            readonly: true,
            fsck_after_every_transaction: true,
            ..Default::default()
        };
        let mut minfs = new_fs(&ramdisk, config).await;

        minfs.format().await.expect("failed to format minfs");
        minfs.fsck().await.expect("failed to fsck minfs");
        let serving = minfs.serve().await.expect("failed to serve minfs");
        serving.shutdown().await.expect("failed to shutdown minfs");
    }

    #[fuchsia::test]
    async fn minfs_format_fsck_success() {
        let block_size = 8192;
        let ramdisk = ramdisk(block_size).await;
        let mut minfs = new_fs(&ramdisk, Minfs::default()).await;

        minfs.format().await.expect("failed to format minfs");
        minfs.fsck().await.expect("failed to fsck minfs");
    }

    #[fuchsia::test]
    async fn minfs_format_serve_write_query_restart_read_shutdown() {
        let block_size = 8192;
        let ramdisk = ramdisk(block_size).await;
        let mut minfs = new_fs(&ramdisk, Minfs::default()).await;

        minfs.format().await.expect("failed to format minfs");
        let serving = minfs.serve().await.expect("failed to serve minfs the first time");

        // snapshot of FilesystemInfo
        let fs_info1 =
            serving.query().await.expect("failed to query filesystem info after first serving");

        let filename = "test_file";
        let content = String::from("test content").into_bytes();

        {
            let test_file = fuchsia_fs::directory::open_file(
                serving.root(),
                filename,
                fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE,
            )
            .await
            .expect("failed to create test file");
            let _: u64 = test_file
                .write(&content)
                .await
                .expect("failed to write to test file")
                .map_err(Status::err_from_raw)
                .expect("write error");
        }

        // check against the snapshot FilesystemInfo
        let fs_info2 = serving.query().await.expect("failed to query filesystem info after write");
        assert_eq!(
            fs_info2.used_bytes - fs_info1.used_bytes,
            fs_info2.block_size as u64 // assuming content < 8K
        );

        serving.shutdown().await.expect("failed to shutdown minfs the first time");
        let minfs = new_fs(&ramdisk, Minfs::default()).await;
        let serving = minfs.serve().await.expect("failed to serve minfs the second time");

        {
            let test_file =
                fuchsia_fs::directory::open_file(serving.root(), filename, fio::PERM_READABLE)
                    .await
                    .expect("failed to open test file");
            let read_content =
                fuchsia_fs::file::read(&test_file).await.expect("failed to read from test file");
            assert_eq!(content, read_content);
        }

        // once more check against the snapshot FilesystemInfo
        let fs_info3 = serving.query().await.expect("failed to query filesystem info after read");
        assert_eq!(
            fs_info3.used_bytes - fs_info1.used_bytes,
            fs_info3.block_size as u64 // assuming content < 8K
        );

        let _ = serving.shutdown().await.expect("failed to shutdown minfs the second time");
    }

    #[fuchsia::test]
    async fn minfs_bind_to_path() {
        let block_size = 8192;
        let test_content = b"test content";
        let ramdisk = ramdisk(block_size).await;
        let mut minfs = new_fs(&ramdisk, Minfs::default()).await;

        minfs.format().await.expect("failed to format minfs");
        let mut serving = minfs.serve().await.expect("failed to serve minfs");
        serving.bind_to_path("/test-minfs-path").expect("bind_to_path failed");
        let test_path = "/test-minfs-path/test_file";

        {
            let mut file = std::fs::File::create(test_path).expect("failed to create test file");
            file.write_all(test_content).expect("write bytes");
        }

        {
            let mut file = std::fs::File::open(test_path).expect("failed to open test file");
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).expect("failed to read test file");
            assert_eq!(buf, test_content);
        }

        serving.shutdown().await.expect("failed to shutdown minfs");

        std::fs::File::open(test_path).expect_err("test file was not unbound");
    }

    #[fuchsia::test]
    async fn minfs_take_exposed_dir_does_not_drop() {
        let block_size = 512;
        let test_content = b"test content";
        let test_file_name = "test-file";
        let ramdisk = ramdisk(block_size).await;
        let mut minfs = new_fs(&ramdisk, Minfs::default()).await;

        minfs.format().await.expect("failed to format fxfs");

        let fs = minfs.serve().await.expect("failed to serve fxfs");
        let file = {
            let file = fuchsia_fs::directory::open_file(
                fs.root(),
                test_file_name,
                fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
            )
            .await
            .unwrap();
            fuchsia_fs::file::write(&file, test_content).await.unwrap();
            file.close().await.expect("close fidl error").expect("close error");
            fuchsia_fs::directory::open_file(fs.root(), test_file_name, fio::PERM_READABLE)
                .await
                .unwrap()
        };

        let exposed_dir = fs.take_exposed_dir();

        assert_eq!(fuchsia_fs::file::read(&file).await.unwrap(), test_content);

        connect_to_protocol_at_dir_root::<fidl_fuchsia_fs::AdminMarker>(&exposed_dir)
            .expect("connecting to admin marker")
            .shutdown()
            .await
            .expect("shutdown failed");
    }

    #[fuchsia::test]
    async fn f2fs_format_fsck_success() {
        let block_size = 4096;
        let ramdisk = ramdisk(block_size).await;
        let mut f2fs = new_fs(&ramdisk, F2fs::default()).await;

        f2fs.format().await.expect("failed to format f2fs");
        f2fs.fsck().await.expect("failed to fsck f2fs");
    }

    #[fuchsia::test]
    async fn f2fs_format_serve_write_query_restart_read_shutdown() {
        let block_size = 4096;
        let ramdisk = ramdisk(block_size).await;
        let mut f2fs = new_fs(&ramdisk, F2fs::default()).await;

        f2fs.format().await.expect("failed to format f2fs");
        let serving = f2fs.serve().await.expect("failed to serve f2fs the first time");

        // snapshot of FilesystemInfo
        let fs_info1 =
            serving.query().await.expect("failed to query filesystem info after first serving");

        let filename = "test_file";
        let content = String::from("test content").into_bytes();

        {
            let test_file = fuchsia_fs::directory::open_file(
                serving.root(),
                filename,
                fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_WRITABLE,
            )
            .await
            .expect("failed to create test file");
            let _: u64 = test_file
                .write(&content)
                .await
                .expect("failed to write to test file")
                .map_err(Status::err_from_raw)
                .expect("write error");
        }

        // check against the snapshot FilesystemInfo
        let fs_info2 = serving.query().await.expect("failed to query filesystem info after write");
        // With zx::stream, f2fs doesn't support the inline data feature allowing file
        // inode blocks to include small data. This way requires keeping two copies of VMOs
        // for the same inline data
        // assuming content < 4K and its inode block.
        let expected_size2 = fs_info2.block_size * 2;
        assert_eq!(fs_info2.used_bytes - fs_info1.used_bytes, expected_size2 as u64);

        serving.shutdown().await.expect("failed to shutdown f2fs the first time");
        let f2fs = new_fs(&ramdisk, F2fs::default()).await;
        let serving = f2fs.serve().await.expect("failed to serve f2fs the second time");

        {
            let test_file =
                fuchsia_fs::directory::open_file(serving.root(), filename, fio::PERM_READABLE)
                    .await
                    .expect("failed to open test file");
            let read_content =
                fuchsia_fs::file::read(&test_file).await.expect("failed to read from test file");
            assert_eq!(content, read_content);
        }

        // once more check against the snapshot FilesystemInfo
        let fs_info3 = serving.query().await.expect("failed to query filesystem info after read");
        // assuming content < 4K and its inode block.
        let expected_size3 = fs_info3.block_size * 2;
        assert_eq!(fs_info3.used_bytes - fs_info1.used_bytes, expected_size3 as u64);

        serving.shutdown().await.expect("failed to shutdown f2fs the second time");
        let mut f2fs = new_fs(&ramdisk, F2fs::default()).await;
        f2fs.fsck().await.expect("failed to fsck f2fs after shutting down the second time");
    }

    #[fuchsia::test]
    async fn f2fs_bind_to_path() {
        let block_size = 4096;
        let test_content = b"test content";
        let ramdisk = ramdisk(block_size).await;
        let mut f2fs = new_fs(&ramdisk, F2fs::default()).await;

        f2fs.format().await.expect("failed to format f2fs");
        let mut serving = f2fs.serve().await.expect("failed to serve f2fs");
        serving.bind_to_path("/test-f2fs-path").expect("bind_to_path failed");
        let test_path = "/test-f2fs-path/test_file";

        {
            let mut file = std::fs::File::create(test_path).expect("failed to create test file");
            file.write_all(test_content).expect("write bytes");
        }

        {
            let mut file = std::fs::File::open(test_path).expect("failed to open test file");
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).expect("failed to read test file");
            assert_eq!(buf, test_content);
        }

        serving.shutdown().await.expect("failed to shutdown f2fs");

        std::fs::File::open(test_path).expect_err("test file was not unbound");
    }

    #[fuchsia::test]
    async fn fxfs_open_volume() {
        let block_size = 512;
        let ramdisk = ramdisk(block_size).await;
        let mut fxfs = new_fs(&ramdisk, Fxfs::default()).await;

        fxfs.format().await.expect("failed to format fxfs");

        let fs = fxfs.serve_multi_volume().await.expect("failed to serve fxfs");

        assert_eq!(fs.has_volume("foo").await.expect("has_volume"), false);
        assert!(
            fs.open_volume("foo", MountOptions::default()).await.is_err(),
            "Opening nonexistent volume should fail"
        );

        let vol = fs
            .create_volume("foo", CreateOptions::default(), MountOptions::default())
            .await
            .expect("Create volume failed");
        vol.query().await.expect("Query volume failed");
        // TODO(https://fxbug.dev/42057878) Closing the volume is not synchronous. Immediately reopening the
        // volume will race with the asynchronous close and sometimes fail because the volume is
        // still mounted.
        // fs.open_volume("foo", MountOptions{crypt: None, as_blob: false}).await
        //    .expect("Open volume failed");
        assert_eq!(fs.has_volume("foo").await.expect("has_volume"), true);
    }

    #[fuchsia::test]
    async fn fxfs_take_exposed_dir_does_not_drop() {
        let block_size = 512;
        let test_content = b"test content";
        let test_file_name = "test-file";
        let ramdisk = ramdisk(block_size).await;
        let mut fxfs = new_fs(&ramdisk, Fxfs::default()).await;

        fxfs.format().await.expect("failed to format fxfs");

        let fs = fxfs.serve_multi_volume().await.expect("failed to serve fxfs");
        let file = {
            let vol = fs
                .create_volume("foo", CreateOptions::default(), MountOptions::default())
                .await
                .expect("Create volume failed");
            let file = fuchsia_fs::directory::open_file(
                vol.root(),
                test_file_name,
                fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
            )
            .await
            .unwrap();
            fuchsia_fs::file::write(&file, test_content).await.unwrap();
            file.close().await.expect("close fidl error").expect("close error");
            fuchsia_fs::directory::open_file(vol.root(), test_file_name, fio::PERM_READABLE)
                .await
                .unwrap()
        };

        let exposed_dir = fs.take_exposed_dir();

        assert_eq!(fuchsia_fs::file::read(&file).await.unwrap(), test_content);

        connect_to_protocol_at_dir_root::<fidl_fuchsia_fs::AdminMarker>(&exposed_dir)
            .expect("connecting to admin marker")
            .shutdown()
            .await
            .expect("shutdown failed");
    }
}
