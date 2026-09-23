// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::resolver::Resolver;
use anyhow::{Context as _, Error, anyhow};
use async_trait::async_trait;
use directed_graph::DirectedGraph;
use fidl::endpoints::{ClientEnd, Proxy};
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_resolution as fresolution;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_pkg as fpkg;
use fuchsia_url::boot::{AbsoluteComponentUrl, AbsolutePackageUrl, ComponentUrl, PackageUrl};
use futures::TryStreamExt;
use futures::future::FutureExt;
use routing::resolving::{self, ComponentAddress, ResolvedComponent, ResolverError};
use std::path::Path;
use std::sync::Arc;
use system_image::{Bootfs, PathHashMapping};
use version_history::AbiRevision;

pub const SCHEME: &str = "fuchsia-boot";

/// The path for the bootfs package index relative to root of
/// the /boot directory.
const BOOT_PACKAGE_INDEX: &str = "data/bootfs_packages";

/// The subdirectory of /boot that holds all merkle-root named
/// blobs used by package resolution.
static BOOTFS_BLOB_DIR: &str = "blob";

/// The flags used to open the bootfs blobs dir.
const BLOB_DIR_FLAGS: fio::Flags = fio::PERM_READABLE.union(fio::PERM_EXECUTABLE);

/// Resolves component URLs with the "fuchsia-boot" scheme, which supports loading components from
/// the /boot directory in component_manager's namespace.
///
/// On a typical system, this /boot directory is the bootfs served from the contents of the
/// 'ZBI_TYPE_STORAGE_BOOTFS' ZBI item by bootsvc, the process which starts component_manager.
///
/// For unit and integration tests, the /pkg directory in component_manager's namespace may be used
/// to load components.
///
/// URL syntax:
/// - fuchsia-boot:///path/within/bootfs#meta/component.cm
#[derive(Clone, Debug)]
pub struct FuchsiaBootResolver {
    boot_package_resolver: Option<Arc<FuchsiaBootPackageResolver>>,
}

impl FuchsiaBootResolver {
    /// Create a new FuchsiaBootResolver and its associated FuchsiaBootPackageResolver.
    ///
    /// This first checks whether the path passed in is present in the namespace, and returns
    /// Ok(None) if not present. For unit and integration tests, this path may point to /pkg.
    pub async fn new(
        path: &'static str,
    ) -> Result<Option<(Self, Option<Arc<FuchsiaBootPackageResolver>>)>, Error> {
        let bootfs_dir = Path::new(path);

        // TODO(97517): Remove this check if there is never a case for starting component manager
        // without a /boot dir in namespace.
        if !bootfs_dir.exists() {
            return Ok(None);
        }

        let boot_proxy =
            fuchsia_fs::directory::open_in_namespace(bootfs_dir.to_str().unwrap(), BLOB_DIR_FLAGS)?;

        let component_resolver = Self::new_from_directory(boot_proxy).await?;
        let package_resolver = component_resolver.boot_package_resolver.clone();
        Ok(Some((component_resolver, package_resolver)))
    }

    /// Create a new FuchsiaBootResolver that resolves URLs within the given directory. Used for
    /// injection in unit tests.
    async fn new_from_directory(proxy: fio::DirectoryProxy) -> Result<Self, Error> {
        let boot_package_resolver = FuchsiaBootPackageResolver::try_instantiate(&proxy).await?;

        Ok(Self { boot_package_resolver })
    }

    async fn resolve_packaged_absolute_component(
        &self,
        url: &AbsoluteComponentUrl,
    ) -> Result<fresolution::Component, fresolution::ResolverError> {
        match &self.boot_package_resolver {
            Some(boot_package_resolver) => {
                let (proxy, server) = fidl::endpoints::create_proxy();
                let context = boot_package_resolver
                    .resolve_absolute_url(&url.to_package_url(), server)
                    .await
                    .map_err(package_to_component_error)?;

                self.construct_component(
                    proxy,
                    &ComponentUrl::Absolute(url.clone()),
                    Some(fresolution::Context { bytes: context.bytes }),
                )
                .await
            }
            None => {
                log::warn!(
                    "Cannot resolve packaged bootfs components without a package index: {url:?}",
                );
                return Err(fresolution::ResolverError::PackageNotFound);
            }
        }
    }

    async fn resolve_packaged_relative_component(
        &self,
        url: &fuchsia_url::RelativeComponentUrl,
        context: fresolution::Context,
    ) -> Result<fresolution::Component, fresolution::ResolverError> {
        match &self.boot_package_resolver {
            Some(boot_package_resolver) => {
                let (proxy, server) = fidl::endpoints::create_proxy();
                let context = boot_package_resolver
                    .resolve_relative_url(
                        url.package_url(),
                        fpkg::ResolutionContext { bytes: context.bytes },
                        server,
                    )
                    .await
                    .map_err(package_to_component_error)?;

                self.construct_component(
                    proxy,
                    &ComponentUrl::Relative(url.clone()),
                    Some(fresolution::Context { bytes: context.bytes }),
                )
                .await
            }
            None => {
                log::warn!(
                    "Cannot resolve packaged bootfs components without a package index: {url:?}",
                );
                return Err(fresolution::ResolverError::PackageNotFound);
            }
        }
    }

    async fn construct_component(
        &self,
        proxy: fio::DirectoryProxy,
        url: &ComponentUrl,
        resolution_context: Option<fresolution::Context>,
    ) -> Result<fresolution::Component, fresolution::ResolverError> {
        let manifest = url.resource();

        let abi_revision =
            fidl_fuchsia_component_abi_ext::read_abi_revision_optional(&proxy, AbiRevision::PATH)
                .await?;

        // Read the component manifest (.cm file) from the package-root.
        let data = mem_util::open_file_data(&proxy, &manifest)
            .await
            .map_err(|_| fresolution::ResolverError::ManifestNotFound)?;

        let decl_bytes =
            mem_util::bytes_from_data(&data).map_err(|_| fresolution::ResolverError::Io)?;

        let decl: fdecl::Component = fidl::unpersist(&decl_bytes[..])
            .map_err(|_| fresolution::ResolverError::InvalidManifest)?;

        let config_values = if let Some(config_decl) = decl.config.as_ref() {
            let strategy = config_decl
                .value_source
                .as_ref()
                .ok_or(fresolution::ResolverError::InvalidManifest)?;
            match strategy {
                // If we have to read the source from a package, do so.
                fdecl::ConfigValueSource::PackagePath(path) => Some(
                    mem_util::open_file_data(&proxy, path)
                        .await
                        .map_err(|_| fresolution::ResolverError::ConfigValuesNotFound)?,
                ),
                // We don't have to do anything for capability routing.
                fdecl::ConfigValueSource::Capabilities(_) => None,
                fdecl::ConfigValueSourceUnknown!() => {
                    return Err(fresolution::ResolverError::InvalidManifest);
                }
            }
        } else {
            None
        };
        Ok(fresolution::Component {
            url: Some(url.to_string()),
            resolution_context,
            decl: Some(data),
            package: Some(fresolution::Package {
                url: Some(url.to_package_url().to_string()),
                directory: Some(ClientEnd::new(proxy.into_channel().unwrap().into_zx_channel())),
                ..Default::default()
            }),
            config_values,
            abi_revision: abi_revision.map(Into::into),
            ..Default::default()
        })
    }

    async fn resolve_unparsed_absolute_url(
        &self,
        url: &str,
    ) -> Result<fresolution::Component, fresolution::ResolverError> {
        let url = AbsoluteComponentUrl::parse(url).map_err(|e| {
            log::warn!("invalid component url {url}: {:#}", anyhow!(e));
            fresolution::ResolverError::InvalidArgs
        })?;
        self.resolve_parsed_absolute_url(url).await
    }

    async fn resolve_parsed_absolute_url(
        &self,
        url: AbsoluteComponentUrl,
    ) -> Result<fresolution::Component, fresolution::ResolverError> {
        self.resolve_packaged_absolute_component(&url).await
    }

    async fn resolve_url(
        &self,
        url: &str,
        context: fresolution::Context,
    ) -> Result<fresolution::Component, fresolution::ResolverError> {
        let url = ComponentUrl::parse(url).map_err(|e| {
            log::warn!(url:?; "invalid boot url: {:#}", anyhow!(e));
            fresolution::ResolverError::InvalidArgs
        })?;
        match url {
            ComponentUrl::Absolute(absolute) => {
                if !context.bytes.is_empty() {
                    log::warn!(
                        "ResolveWithContext context must be empty if url is absolute {} {:?}",
                        absolute,
                        context,
                    );
                    return Err(fresolution::ResolverError::InvalidArgs);
                }
                self.resolve_parsed_absolute_url(absolute).await
            }
            ComponentUrl::Relative(relative) => {
                self.resolve_packaged_relative_component(&relative, context).await
            }
        }
    }

    pub async fn serve(self, mut stream: fresolution::ResolverRequestStream) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                fresolution::ResolverRequest::Resolve { component_url, responder } => {
                    responder.send(self.resolve_unparsed_absolute_url(&component_url).await)?;
                }
                fresolution::ResolverRequest::ResolveWithContext {
                    component_url,
                    context,
                    responder,
                } => {
                    responder.send(self.resolve_url(&component_url, context).await)?;
                }
                fresolution::ResolverRequest::_UnknownMethod { ordinal, .. } => {
                    log::warn!(ordinal:%; "Unknown Resolver request");
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct FuchsiaBootPackageResolver {
    // Cache of open package directories.
    root_dir_cache: package_directory::RootDirCache<fio::DirectoryProxy>,
    // PathHashMapping encoding the index for boot package resolution.
    boot_package_index: PathHashMapping<Bootfs>,
    context_authenticator: context_authenticator::ContextAuthenticator,
}

impl FuchsiaBootPackageResolver {
    // Attempts to instantiate a FuchsiaBootPackageResolver.
    //
    // - The absence of a /boot/blob dir implies that there are no packages in the BootFS,
    // and boot resolver setup should still succeed.
    //
    // - The presence of a /boot/blob dir, but absence of a package index implies incorrect
    //   bootfs assembly, and produces a FuchsiaBootResolver instantiation error.
    async fn try_instantiate(proxy: &fio::DirectoryProxy) -> Result<Option<Arc<Self>>, Error> {
        // Check for the existence of a /boot/blob directory. Until we've started our migration,
        // it's a valid state for no packages to exist in the bootfs, in which case no blobs will
        // exist.
        if !fuchsia_fs::directory::dir_contains(proxy, BOOTFS_BLOB_DIR).await? {
            return Ok(None);
        }

        let boot_blob_storage = fuchsia_fs::directory::open_directory_async(
            &proxy,
            BOOTFS_BLOB_DIR,
            BLOB_DIR_FLAGS
        )
        .map_err(|err| anyhow!("Bootfs blob directory existed, but converting it into a blob client for package resolution failed: {:?}", err))?;

        let root_dir_cache = package_directory::RootDirCache::new(boot_blob_storage);

        let mut boot_package_index = Self::extract_bootfs_index(&proxy)
            .await
            .context("Failed to extract a package index from a bootfs that contains packages")?;

        // Minimize the size of the boot_package_index since this is kept in memory.
        boot_package_index.shrink_to_fit();

        let context_authenticator = Default::default();

        Ok(Some(Arc::new(Self { root_dir_cache, boot_package_index, context_authenticator })))
    }

    /// Load `data/bootfs_packages` from /boot, if present.
    async fn extract_bootfs_index(
        boot_proxy: &fio::DirectoryProxy,
    ) -> Result<PathHashMapping<Bootfs>, Error> {
        let bootfs_package_index = fuchsia_fs::directory::open_file_async(
            &boot_proxy,
            BOOT_PACKAGE_INDEX,
            fio::PERM_READABLE,
        )?;

        let bootfs_package_contents = fuchsia_fs::file::read(&bootfs_package_index).await?;
        PathHashMapping::<Bootfs>::deserialize(&(*bootfs_package_contents))
            .context("Parsing bootfs index failed")
    }

    async fn resolve_unparsed_absolute_url(
        &self,
        url: &str,
        dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    ) -> Result<fpkg::ResolutionContext, fpkg::ResolveError> {
        let url = AbsolutePackageUrl::parse(url).map_err(|e| {
            log::warn!(url:?; "invalid boot url: {:#}", anyhow!(e));
            fpkg::ResolveError::InvalidUrl
        })?;
        self.resolve_absolute_url(&url, dir).await
    }

    async fn resolve_absolute_url(
        &self,
        url: &AbsolutePackageUrl,
        dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    ) -> Result<fpkg::ResolutionContext, fpkg::ResolveError> {
        let path = url.path().as_ref().ok_or_else(|| {
            log::warn!(url:?; "packaged url missing a path");
            fpkg::ResolveError::InvalidUrl
        })?;
        let package_path = fuchsia_pkg::PackagePath::from_name_and_variant(
            fuchsia_url::PackageName::try_from(path).map_err(|e| {
                log::warn!(url:?; "packaged url path should just be a name: {:#}", anyhow!(e));
                fpkg::ResolveError::InvalidUrl
            })?,
            fuchsia_url::PackageVariant::zero(),
        );

        let meta_hash =
            self.boot_package_index.hash_for_package(&package_path).ok_or_else(|| {
                static INDEX_LOGGED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if INDEX_LOGGED.fetch_or(true, std::sync::atomic::Ordering::Relaxed) {
                    log::warn!(url:?; "'{package_path}' not in bootfs index");
                } else {
                    log::warn!(
                     url:?; "'{package_path}' not in bootfs index. Options are: {}",
                     itertools::join(self.boot_package_index.contents().keys(), ", ")
                    );
                }
                fpkg::ResolveError::PackageNotFound
            })?;

        let root_dir = self.root_dir_cache.get_or_insert(meta_hash, None).await.map_err(|e| {
            log::warn!(url:?; "creating RootDir for {meta_hash} {:#}", anyhow!(e));
            fpkg::ResolveError::Internal
        })?;

        let () = vfs::directory::serve_on(
            root_dir,
            BLOB_DIR_FLAGS,
            package_directory::ExecutionScope::new(),
            dir,
        );

        Ok(self.context_authenticator.clone().create(&meta_hash))
    }

    async fn resolve_url(
        &self,
        url: &str,
        context: fpkg::ResolutionContext,
        dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    ) -> Result<fpkg::ResolutionContext, fpkg::ResolveError> {
        let url = PackageUrl::parse(url).map_err(|e| {
            log::warn!(url:?; "invalid boot url: {:#}", anyhow!(e));
            fpkg::ResolveError::InvalidUrl
        })?;
        match url {
            PackageUrl::Absolute(absolute) => {
                if !context.bytes.is_empty() {
                    log::warn!(
                        "ResolveWithContext context must be empty if url is absolute {} {:?}",
                        absolute,
                        context,
                    );
                    return Err(fpkg::ResolveError::InvalidContext);
                }
                self.resolve_absolute_url(&absolute, dir).await
            }
            PackageUrl::Relative(relative) => {
                self.resolve_relative_url(&relative, context, dir).await
            }
        }
    }

    async fn resolve_relative_url(
        &self,
        url: &fuchsia_url::RelativePackageUrl,
        context: fpkg::ResolutionContext,
        dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    ) -> Result<fpkg::ResolutionContext, fpkg::ResolveError> {
        let superpackage_hash =
            self.context_authenticator.clone().authenticate(context).map_err(|e| {
                log::warn!(url:%; "invalid context: {:#}", anyhow!(e));
                fpkg::ResolveError::InvalidContext
            })?;

        let superpackage =
            self.root_dir_cache.get_or_insert(superpackage_hash, None).await.map_err(|e| {
                log::warn!(
                    url:?; "creating RootDir for superpackage {superpackage_hash} {:#}", anyhow!(e)
                );
                fpkg::ResolveError::Internal
            })?;

        let subpackage_hash = match superpackage
            .subpackages()
            .await
            .map_err(|e| {
                log::warn!(
                    "reading subpackages of {} for {}: {:#}",
                    superpackage_hash,
                    url,
                    anyhow!(e)
                );
                fpkg::ResolveError::Internal
            })?
            .subpackages()
            .get(url)
        {
            Some(subpackage) => *subpackage,
            None => {
                let path = superpackage.path().await.ok();
                log::warn!("'{url}' is not a subpackage of {path:?} {superpackage_hash}");
                return Err(fpkg::ResolveError::PackageNotFound);
            }
        };
        let subpackage =
            self.root_dir_cache.get_or_insert(subpackage_hash, None).await.map_err(|e| {
                log::warn!(
                  url:?; "creating RootDir for subpackage {subpackage_hash} {:#}", anyhow!(e)
                );
                fpkg::ResolveError::Internal
            })?;

        let () = vfs::directory::serve_on(
            subpackage,
            BLOB_DIR_FLAGS,
            package_directory::ExecutionScope::new(),
            dir,
        );

        Ok(self.context_authenticator.clone().create(&subpackage_hash))
    }

    pub async fn serve(
        &self,
        mut stream: fpkg::PackageResolverRequestStream,
    ) -> Result<(), anyhow::Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                fpkg::PackageResolverRequest::Resolve { package_url, dir, responder } => {
                    let () = responder.send(
                        self.resolve_unparsed_absolute_url(&package_url, dir)
                            .await
                            .as_ref()
                            .map_err(|e| *e),
                    )?;
                }
                fpkg::PackageResolverRequest::ResolveWithContext {
                    package_url,
                    context,
                    dir,
                    responder,
                } => {
                    let () = responder.send(
                        self.resolve_url(&package_url, context, dir).await.as_ref().map_err(|e| *e),
                    )?;
                }
                // GetHash was added to support a CLI tool for investigating the state of ephemeral
                // packages and should otherwise not be used.
                fpkg::PackageResolverRequest::GetHash { package_url, responder } => {
                    log::error!(
                        "unsupported fuchsia.pkg/PackageResolver.GetHash called with {:?}",
                        package_url
                    );
                    let () = responder
                        .send(Err(zx::Status::NOT_SUPPORTED.into_raw()))
                        .context("sending fuchsia.pkg/PackageResolver.GetHash response")?;
                }
            }
        }
        Ok(())
    }

    /// Returns a callback to be given to `fuchsia_inspect::Node::record_lazy_child`.
    pub fn record_lazy_inspect(
        self: &Arc<Self>,
    ) -> impl Fn() -> futures::future::BoxFuture<
        'static,
        Result<fuchsia_inspect::Inspector, anyhow::Error>,
    > + Send
    + Sync
    + 'static {
        let this = Arc::downgrade(self);
        move || {
            let this = this.clone();
            async move {
                let inspector = fuchsia_inspect::Inspector::default();
                if let Some(this) = this.upgrade() {
                    let root = inspector.root();
                    root.record_lazy_child(
                        "open-packages",
                        this.root_dir_cache.record_lazy_inspect(),
                    );

                    root.record_child("index", |n| {
                        for (path, hash) in this.boot_package_index.contents() {
                            n.record_string(path.to_string(), hash.to_string())
                        }
                    });
                }
                Ok(inspector)
            }
            .boxed()
        }
    }
}

#[async_trait]
impl Resolver for FuchsiaBootResolver {
    async fn resolve(
        &self,
        component_address: &ComponentAddress,
    ) -> Result<ResolvedComponent, ResolverError> {
        let (url, context) = match component_address {
            ComponentAddress::Absolute { url } => {
                (url.as_str(), fresolution::Context { bytes: vec![] })
            }
            url @ ComponentAddress::RelativePath { scheme, url: _, context } => {
                if scheme != SCHEME {
                    return Err(ResolverError::MalformedUrl(
                        anyhow!("expected scheme {SCHEME} but was {scheme}").into(),
                    ));
                }
                (url.url(), context.into())
            }
        };

        let fresolution::Component {
            decl,
            package,
            config_values,
            abi_revision,
            resolution_context,
            ..
        } = self.resolve_url(url, context).await?;
        let decl = decl.ok_or_else(|| {
            ResolverError::ManifestInvalid(
                anyhow!("missing manifest from resolved component").into(),
            )
        })?;
        let mut dependencies = DirectedGraph::new();
        let decl = resolving::read_and_validate_manifest(&decl, &mut dependencies)?;
        let config_values = if let Some(cv) = config_values {
            Some(resolving::read_and_validate_config_values(&cv)?)
        } else {
            None
        };
        Ok(ResolvedComponent {
            context_to_resolve_children: resolution_context.map(Into::into),
            decl,
            package: package.map(|p| p.try_into()).transpose()?,
            config_values,
            abi_revision: abi_revision.map(Into::into),
            dependencies,
        })
    }
}

fn package_to_component_error(e: fpkg::ResolveError) -> fresolution::ResolverError {
    match e {
        fpkg::ResolveError::Internal => fresolution::ResolverError::Internal,
        fpkg::ResolveError::AccessDenied => fresolution::ResolverError::Internal,
        fpkg::ResolveError::Io => fresolution::ResolverError::Io,
        fpkg::ResolveError::BlobNotFound => fresolution::ResolverError::Internal,
        fpkg::ResolveError::PackageNotFound => fresolution::ResolverError::PackageNotFound,
        fpkg::ResolveError::RepoNotFound => fresolution::ResolverError::Internal,
        fpkg::ResolveError::NoSpace => fresolution::ResolverError::NoSpace,
        fpkg::ResolveError::UnavailableBlob => fresolution::ResolverError::Internal,
        fpkg::ResolveError::UnavailableRepoMetadata => fresolution::ResolverError::Internal,
        fpkg::ResolveError::InvalidUrl => fresolution::ResolverError::InvalidArgs,
        fpkg::ResolveError::InvalidContext => fresolution::ResolverError::InvalidArgs,
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::component::ComponentInstance;
    use crate::model::context::ModelContext;
    use ::routing::resolving::ResolvedPackage;
    use assert_matches::assert_matches;
    use cm_rust::{FidlIntoNative, NativeIntoFidl};
    use fidl::endpoints::create_proxy;
    use fidl::persist;
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_data as fdata;
    use fuchsia_async::Task;
    use routing::bedrock::structured_dict::ComponentInput;
    use std::io::Read as _;
    use std::sync::Weak;
    use vfs::ToObjectRequest;
    use vfs::directory::entry_container::Directory;
    use vfs::directory::helper::DirectlyMutable;
    use vfs::directory::simple::Simple as PseudoDir;
    use vfs::execution_scope::ExecutionScope;
    use vfs::file::vmo::read_only;
    use vfs::path::Path as VfsPath;

    fn serve_vfs_dir(root: Arc<impl Directory>) -> (Task<()>, fio::DirectoryProxy) {
        let fs_scope = ExecutionScope::new();
        let (client, server) = create_proxy::<fio::DirectoryMarker>();
        BLOB_DIR_FLAGS
            .to_object_request(server.into_channel())
            .handle(|request| root.open(fs_scope.clone(), VfsPath::dot(), BLOB_DIR_FLAGS, request));
        let vfs_task = Task::spawn(async move { fs_scope.wait().await });
        (vfs_task, client)
    }

    #[fuchsia::test]
    async fn hello_world_test() -> Result<(), Error> {
        let manifest_bytes = std::fs::read("/pkg/meta/hello-world-rust.cm").expect("read cm");
        let bin_bytes = std::fs::read("/pkg/bin/hello_world_rust").expect("read bin");

        let package = fuchsia_pkg_testing::PackageBuilder::new_with_abi_revision(
            "hello-world-rust",
            0x9F57AD4916246E87.into(),
        )
        .add_resource_at("meta/hello-world-rust.cm", manifest_bytes.as_slice())
        .add_resource_at("bin/hello_world_rust", bin_bytes.as_slice())
        .build()
        .await
        .unwrap();

        let index = PathHashMapping::<Bootfs>::from([(
            "hello-world-rust/0".parse().unwrap(),
            *package.hash(),
        )]);
        let mut index_bytes = vec![];
        index.serialize(&mut index_bytes).unwrap();

        let mut meta_far_contents = vec![];
        package.meta_far().unwrap().read_to_end(&mut meta_far_contents).unwrap();

        let blob_dir = PseudoDir::new();
        blob_dir.add_entry(package.hash().to_string(), read_only(meta_far_contents)).unwrap();

        let (_, blobs) = package.contents();
        for (hash, bytes) in blobs {
            blob_dir.add_entry(hash.to_string(), vfs::file::vmo::read_only(bytes)).unwrap();
        }

        let root = vfs::pseudo_directory! {
            "data" => vfs::pseudo_directory! {
                "bootfs_packages" => vfs::file::vmo::read_only(index_bytes),
            },
            "blob" => blob_dir
        };

        let (_task, bootfs) = serve_vfs_dir(root);
        let resolver = FuchsiaBootResolver::new_from_directory(bootfs).await.unwrap();

        let url = "fuchsia-boot:///hello-world-rust#meta/hello-world-rust.cm".parse().unwrap();
        let component = resolver.resolve(&ComponentAddress::from_absolute_url(&url)?).await?;

        // Check that both the returned component manifest and the component manifest in
        // the returned package dir match the expected value. This also tests that
        // the resolver returned the right package dir.
        let ResolvedComponent { decl, package: resolved_package, abi_revision, .. } = component;
        version_history_data::HISTORY
            .check_abi_revision_for_runtime(
                abi_revision.expect("boot component should present ABI revision"),
            )
            .expect("ABI revision should be supported for boot component");

        let expected_program = Some(cm_rust::ProgramDecl {
            runner: Some("elf".parse().unwrap()),
            info: fdata::Dictionary {
                entries: Some(vec![
                    fdata::DictionaryEntry {
                        key: "binary".to_string(),
                        value: Some(Box::new(fdata::DictionaryValue::Str(
                            "bin/hello_world_rust".to_string(),
                        ))),
                    },
                    fdata::DictionaryEntry {
                        key: "forward_stderr_to".to_string(),
                        value: Some(Box::new(fdata::DictionaryValue::Str("log".to_string()))),
                    },
                    fdata::DictionaryEntry {
                        key: "forward_stdout_to".to_string(),
                        value: Some(Box::new(fdata::DictionaryValue::Str("log".to_string()))),
                    },
                ]),
                ..Default::default()
            },
        });

        // no need to check full decl as we just want to make
        // sure that we were able to resolve.
        assert_eq!(decl.program, expected_program);

        let ResolvedPackage { url: package_url, directory: package_dir, .. } =
            resolved_package.unwrap();

        // Ensure the returned package URL matches the new packaged format
        assert_eq!(package_url, "fuchsia-boot:///hello-world-rust");

        let dir_proxy = package_dir.into_proxy();
        let path = "meta/hello-world-rust.cm";
        let file_proxy =
            fuchsia_fs::directory::open_file_async(&dir_proxy, path, fio::PERM_READABLE)
                .expect("could not open cm");

        let decl = fuchsia_fs::file::read_fidl::<fdecl::Component>(&file_proxy)
            .await
            .expect("could not read cm");
        let decl: cm_rust::ComponentDecl = decl.fidl_into_native();

        assert_eq!(decl.program, expected_program);

        let bad_url = "fuchsia-boot:///missing-package#meta/hello-world-rust.cm".parse().unwrap();
        let err =
            resolver.resolve(&ComponentAddress::from_absolute_url(&bad_url)?).await.unwrap_err();
        assert_matches!(err, ResolverError::PackageNotFound { .. });
        Ok(())
    }

    #[fuchsia::test]
    async fn config_works() {
        let fake_checksum = cm_rust::ConfigChecksum::Sha256([0; 32]);
        let manifest = fdecl::Component {
            config: Some(
                cm_rust::ConfigDecl {
                    value_source: cm_rust::ConfigValueSource::PackagePath(
                        "meta/has_config.cvf".into(),
                    ),
                    fields: Box::from([cm_rust::ConfigField {
                        key: "foo".into(),
                        type_: cm_rust::ConfigValueType::String { max_size: 100 },
                        mutability: Default::default(),
                    }]),
                    checksum: fake_checksum.clone(),
                }
                .native_into_fidl(),
            ),
            ..Default::default()
        };
        let values_data = fdecl::ConfigValuesData {
            values: Some(vec![fdecl::ConfigValueSpec {
                value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::String(
                    "hello, world!".into(),
                ))),
                ..Default::default()
            }]),
            checksum: Some(fake_checksum.clone().native_into_fidl()),
            ..Default::default()
        };
        let manifest_encoded = persist(&manifest).unwrap();
        let values_data_encoded = persist(&values_data).unwrap();

        let package = fuchsia_pkg_testing::PackageBuilder::new("has_config_pkg")
            .add_resource_at("meta/has_config.cm", manifest_encoded.as_slice())
            .add_resource_at("meta/has_config.cvf", values_data_encoded.as_slice())
            .build()
            .await
            .unwrap();

        let index = PathHashMapping::<Bootfs>::from([(
            "has_config_pkg/0".parse().unwrap(),
            *package.hash(),
        )]);
        let mut index_bytes = vec![];
        index.serialize(&mut index_bytes).unwrap();

        let mut meta_far_contents = vec![];
        package.meta_far().unwrap().read_to_end(&mut meta_far_contents).unwrap();

        let blob_dir = PseudoDir::new();
        blob_dir.add_entry(package.hash().to_string(), read_only(meta_far_contents)).unwrap();

        let (_, blobs) = package.contents();
        for (hash, bytes) in blobs {
            blob_dir.add_entry(hash.to_string(), read_only(bytes)).unwrap();
        }

        let root = vfs::pseudo_directory! {
            "data" => vfs::pseudo_directory! {
                "bootfs_packages" => read_only(index_bytes),
            },
            "blob" => blob_dir
        };

        let (_task, bootfs) = serve_vfs_dir(root);
        let resolver = FuchsiaBootResolver::new_from_directory(bootfs).await.unwrap();

        let url = "fuchsia-boot:///has_config_pkg#meta/has_config.cm".parse().unwrap();
        let component =
            resolver.resolve(&ComponentAddress::from_absolute_url(&url).unwrap()).await.unwrap();

        let ResolvedComponent { decl, config_values, .. } = component;

        let config_decl = decl.config.unwrap();
        let config_values = config_values.unwrap();

        let observed_fields =
            config_encoder::ConfigFields::resolve(&config_decl, config_values, None).unwrap();
        let expected_fields = config_encoder::ConfigFields {
            fields: vec![config_encoder::ConfigField {
                key: "foo".into(),
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::String(
                    "hello, world!".into(),
                )),
                mutability: Default::default(),
            }],
            checksum: fake_checksum,
        };
        assert_eq!(observed_fields, expected_fields);
    }

    #[fuchsia::test]
    async fn config_requires_values() {
        let manifest = fdecl::Component {
            config: Some(
                cm_rust::ConfigDecl {
                    value_source: cm_rust::ConfigValueSource::PackagePath(
                        "meta/has_config.cvf".into(),
                    ),
                    fields: Box::from([cm_rust::ConfigField {
                        key: "foo".into(),
                        type_: cm_rust::ConfigValueType::String { max_size: 100 },
                        mutability: Default::default(),
                    }]),
                    checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                }
                .native_into_fidl(),
            ),
            ..Default::default()
        };

        let manifest_encoded = persist(&manifest).unwrap();

        let package = fuchsia_pkg_testing::PackageBuilder::new("has_config_pkg")
            .add_resource_at("meta/has_config.cm", manifest_encoded.as_slice())
            .build()
            .await
            .unwrap();

        let mut meta_far_contents = vec![];
        package.meta_far().unwrap().read_to_end(&mut meta_far_contents).unwrap();

        let index = PathHashMapping::<Bootfs>::from([(
            "has_config_pkg/0".parse().unwrap(),
            *package.hash(),
        )]);
        let mut index_bytes = vec![];
        index.serialize(&mut index_bytes).unwrap();

        let blob_dir = PseudoDir::new();
        blob_dir.add_entry(package.hash().to_string(), read_only(meta_far_contents)).unwrap();

        let bootfs = vfs::pseudo_directory! {
            "data" => vfs::pseudo_directory! {
                "bootfs_packages" => read_only(index_bytes),
            },
            "blob" => blob_dir,
        };

        let (_task, bootfs_proxy) = serve_vfs_dir(bootfs);
        let resolver = FuchsiaBootResolver::new_from_directory(bootfs_proxy).await.unwrap();

        let root = ComponentInstance::new_root(
            ComponentInput::default(),
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "fuchsia-boot:///#meta/root.cm".parse().unwrap(),
        )
        .await;

        let url = "fuchsia-boot:///has_config_pkg#meta/has_config.cm".parse().unwrap();

        let err = resolver
            .resolve(&ComponentAddress::from_url(&url, &root).await.unwrap())
            .await
            .unwrap_err();

        assert_matches!(err, ResolverError::ConfigValuesIo { .. });
    }

    #[fuchsia::test]
    async fn resolve_errors_test() {
        let manifest_encoded = persist(&fdecl::Component {
            program: Some(fdecl::Program {
                runner: None,
                info: Some(fdata::Dictionary { entries: Some(vec![]), ..Default::default() }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();

        let package = fuchsia_pkg_testing::PackageBuilder::new("invalid-pkg")
            .add_resource_at("meta/invalid.cm", manifest_encoded.as_slice())
            .build()
            .await
            .unwrap();

        let index =
            PathHashMapping::<Bootfs>::from([("invalid-pkg/0".parse().unwrap(), *package.hash())]);
        let mut index_bytes = vec![];
        index.serialize(&mut index_bytes).unwrap();

        let mut meta_far_contents = vec![];
        package.meta_far().unwrap().read_to_end(&mut meta_far_contents).unwrap();

        let blob_dir = PseudoDir::new();
        blob_dir.add_entry(package.hash().to_string(), read_only(meta_far_contents)).unwrap();

        let (_, blobs) = package.contents();
        for (hash, bytes) in blobs {
            blob_dir.add_entry(hash.to_string(), read_only(bytes)).unwrap();
        }

        let root = vfs::pseudo_directory! {
            "data" => vfs::pseudo_directory! {
                "bootfs_packages" => read_only(index_bytes),
            },
            "blob" => blob_dir,
        };

        let (_task, bootfs) = serve_vfs_dir(root);
        let resolver = FuchsiaBootResolver::new_from_directory(bootfs).await.unwrap();

        let root = ComponentInstance::new_root(
            ComponentInput::default(),
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "fuchsia-boot:///root-pkg#meta/root.cm".parse().unwrap(),
        )
        .await;

        let url = "fuchsia-boot:///invalid-pkg#meta/invalid.cm".parse().unwrap();

        let res = resolver.resolve(&ComponentAddress::from_url(&url, &root).await.unwrap()).await;

        assert_matches!(res, Err(ResolverError::ManifestInvalid { .. }));
    }

    #[fuchsia::test]
    async fn packaged_component_with_subsubpackage() {
        // Create the subsub component.
        let test_abi_revision: AbiRevision = AbiRevision::from_u64(1234567);
        let test_program_decl = cm_rust::ProgramDecl {
            runner: Some("kobold".parse().unwrap()),
            info: fdata::Dictionary {
                entries: Some(vec![
                    fdata::DictionaryEntry {
                        key: "binary".to_owned(),
                        value: Some(Box::new(fdata::DictionaryValue::Str(
                            "bin/your_bin".to_owned(),
                        ))),
                    },
                    fdata::DictionaryEntry {
                        key: "forward_stderr_to".to_owned(),
                        value: Some(Box::new(fdata::DictionaryValue::Str("branch".to_owned()))),
                    },
                    fdata::DictionaryEntry {
                        key: "forward_stdout_to".to_owned(),
                        value: Some(Box::new(fdata::DictionaryValue::Str("stump".to_owned()))),
                    },
                ]),
                ..Default::default()
            },
        };
        let subsubpackage_manifest: fidl_fuchsia_component_decl::Component = fdecl::Component {
            program: Some(test_program_decl.clone().native_into_fidl()),
            ..Default::default()
        };
        let subsubpackage_manifest_encoded = persist(&subsubpackage_manifest).unwrap();
        let subsubpackage = fuchsia_pkg_testing::PackageBuilder::new_with_abi_revision(
            "subsubpackage",
            test_abi_revision,
        )
        .add_resource_at("meta/the-subsubcomponent.cm", subsubpackage_manifest_encoded.as_slice())
        .build()
        .await
        .unwrap();
        let mut subsubpackage_far_contents = vec![];
        let _: usize =
            subsubpackage.meta_far().unwrap().read_to_end(&mut subsubpackage_far_contents).unwrap();

        // Create the sub component.
        let subpackage_manifest: fidl_fuchsia_component_decl::Component =
            fdecl::Component { ..Default::default() };
        let subpackage_manifest_encoded = persist(&subpackage_manifest).unwrap();
        let subpackage = fuchsia_pkg_testing::PackageBuilder::new("subpackage")
            .add_subpackage("my-subsubpackage", &subsubpackage)
            .add_resource_at("meta/the-subcomponent.cm", subpackage_manifest_encoded.as_slice())
            .build()
            .await
            .unwrap();
        let mut subpackage_far_contents = vec![];
        let _: usize =
            subpackage.meta_far().unwrap().read_to_end(&mut subpackage_far_contents).unwrap();

        // Create the component.
        let superpackage_manifest = fdecl::Component { ..Default::default() };
        let superpackage_manifest_encoded = persist(&superpackage_manifest).unwrap();
        let superpackage = fuchsia_pkg_testing::PackageBuilder::new("superpackage")
            .add_subpackage("my-subpackage", &subpackage)
            .add_resource_at("meta/the-component.cm", superpackage_manifest_encoded.as_slice())
            .build()
            .await
            .unwrap();
        let mut superpackage_far_contents = vec![];
        let _: usize =
            superpackage.meta_far().unwrap().read_to_end(&mut superpackage_far_contents).unwrap();

        // Create the test environment from the components.
        let index = PathHashMapping::<Bootfs>::from([(
            "superpackage/0".parse().unwrap(),
            *superpackage.hash(),
        )]);
        let mut index_bytes = vec![];
        let () = index.serialize(&mut index_bytes).unwrap();

        let blob_dir = PseudoDir::new();
        blob_dir
            .add_entry(superpackage.hash().to_string(), read_only(superpackage_far_contents))
            .unwrap();
        blob_dir
            .add_entry(subpackage.hash().to_string(), read_only(subpackage_far_contents))
            .unwrap();
        blob_dir
            .add_entry(subsubpackage.hash().to_string(), read_only(subsubpackage_far_contents))
            .unwrap();

        let bootfs = vfs::pseudo_directory! {
            "data" => vfs::pseudo_directory! {
                "bootfs_packages" => vfs::file::read_only(index_bytes.as_slice()),
            },
            "blob" => blob_dir,
        };
        let (_task, bootfs_proxy) = serve_vfs_dir(bootfs);
        let resolver = FuchsiaBootResolver::new_from_directory(bootfs_proxy).await.unwrap();

        // Resolve the chain of components.
        let supercomponent = resolver
            .resolve(
                &ComponentAddress::from_absolute_url(
                    &"fuchsia-boot:///superpackage#meta/the-component.cm".parse().unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let subcomponent = resolver
            .resolve(
                &resolving::ComponentAddress::new_relative_path(
                    "my-subpackage",
                    Some("meta/the-subcomponent.cm"),
                    SCHEME,
                    supercomponent.context_to_resolve_children.unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let subsubcomponent = resolver
            .resolve(
                &resolving::ComponentAddress::new_relative_path(
                    "my-subsubpackage",
                    Some("meta/the-subsubcomponent.cm"),
                    SCHEME,
                    subcomponent.context_to_resolve_children.unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        // Check that both the returned subsubcomponent manifest and the manifest in the returned
        // package dir match the expected value. This also tests that the resolver returned the
        // correct package dir.
        let ResolvedComponent { decl, package, abi_revision, .. } = subsubcomponent;
        assert_eq!(abi_revision, Some(test_abi_revision));

        // No need to check full decl as we just want to make sure that we were able to resolve.
        assert_eq!(decl.program.unwrap(), test_program_decl);

        let ResolvedPackage { url: package_url, directory: package_dir, .. } = package.unwrap();
        assert_eq!(package_url, "my-subsubpackage");

        let dir_proxy = package_dir.into_proxy();
        let file_proxy = fuchsia_fs::directory::open_file_async(
            &dir_proxy,
            "meta/the-subsubcomponent.cm",
            fio::PERM_READABLE,
        )
        .expect("could not open cm");
        let decl = fuchsia_fs::file::read_fidl::<fdecl::Component>(&file_proxy)
            .await
            .expect("could not read cm");
        let decl: cm_rust::ComponentDecl = decl.fidl_into_native();

        assert_eq!(decl.program.unwrap(), test_program_decl);
    }
}
