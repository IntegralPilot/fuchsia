// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, anyhow};
use fidl::endpoints::{ClientEnd, Proxy, create_proxy};
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_resolution as fresolution;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_pkg as fpkg;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_url::fuchsia_pkg::ComponentUrl;
use futures::prelude::*;
use log::warn;
use version_history::AbiRevision;

pub(crate) async fn serve(mut stream: fresolution::ResolverRequestStream) -> anyhow::Result<()> {
    let package_resolver = connect_to_protocol::<fpkg::PackageResolverMarker>()
        .context("failed to connect to PackageResolver service")?;
    while let Some(request) =
        stream.try_next().await.context("failed to read request from FIDL stream")?
    {
        match request {
            fresolution::ResolverRequest::Resolve { component_url, responder } => {
                let result = resolve_component_without_context(&component_url, &package_resolver)
                    .await
                    .map_err(|err| {
                        let fidl_err = (&err).into();
                        warn!(
                            "failed to resolve component URL {}: {:#}",
                            component_url,
                            anyhow!(err)
                        );
                        fidl_err
                    });
                responder.send(result).context("failed sending response")?;
            }
            fresolution::ResolverRequest::ResolveWithContext {
                component_url,
                context,
                responder,
            } => {
                let result =
                    resolve_component_with_context(&component_url, &context, &package_resolver)
                        .await
                        .map_err(|err| {
                            let fidl_err = (&err).into();
                            warn!(
                                "failed to resolve component URL {} with context {:?}: {:#}",
                                component_url,
                                context,
                                anyhow!(err)
                            );
                            fidl_err
                        });
                responder.send(result).context("failed sending response")?;
            }
            fresolution::ResolverRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "Unknown Resolver request");
            }
        }
    }
    Ok(())
}

async fn resolve_component_without_context(
    component_url: &str,
    package_resolver: &fpkg::PackageResolverProxy,
) -> Result<fresolution::Component, ResolverError> {
    let component_url = ComponentUrl::parse(component_url)?;
    let (dir, dir_server_end) = create_proxy::<fio::DirectoryMarker>();
    let outgoing_context = package_resolver
        .resolve(&component_url.package_url().to_string(), dir_server_end)
        .await
        .map_err(ResolverError::IoError)?
        .map_err(ResolverError::PackageResolve)?;
    resolve_component(&component_url, dir, outgoing_context).await
}

async fn resolve_component_with_context(
    component_url: &str,
    incoming_context: &fresolution::Context,
    package_resolver: &fpkg::PackageResolverProxy,
) -> Result<fresolution::Component, ResolverError> {
    let component_url = ComponentUrl::parse(component_url)?;
    let (dir, dir_server_end) = create_proxy::<fio::DirectoryMarker>();
    let outgoing_context = package_resolver
        .resolve_with_context(
            &component_url.package_url().to_string(),
            &fpkg::ResolutionContext { bytes: incoming_context.bytes.clone() },
            dir_server_end,
        )
        .await
        .map_err(ResolverError::IoError)?
        .map_err(ResolverError::PackageResolve)?;
    resolve_component(&component_url, dir, outgoing_context).await
}

async fn resolve_component(
    component_url: &ComponentUrl,
    dir: fio::DirectoryProxy,
    outgoing_context: fpkg::ResolutionContext,
) -> Result<fresolution::Component, ResolverError> {
    // Read the component manifest (.cm file) from the package directory.
    let manifest_data = mem_util::open_file_data(&dir, component_url.resource())
        .await
        .map_err(ResolverError::ManifestNotFound)?;
    let manifest_bytes =
        mem_util::bytes_from_data(&manifest_data).map_err(ResolverError::ReadingManifest)?;
    let decl: fdecl::Component =
        fidl::unpersist(&manifest_bytes[..]).map_err(ResolverError::ParsingManifest)?;

    let config_values = if let Some(config_decl) = decl.config.as_ref() {
        let strategy =
            config_decl.value_source.as_ref().ok_or(ResolverError::MissingConfigSource)?;
        match strategy {
            // If we have to read the source from a package, do so.
            fdecl::ConfigValueSource::PackagePath(path) => Some(
                mem_util::open_file_data(&dir, path)
                    .await
                    .map_err(ResolverError::ConfigValuesNotFound)?,
            ),
            // We don't have to do anything for capability routing.
            fdecl::ConfigValueSource::Capabilities(_) => None,
            fdecl::ConfigValueSourceUnknown!() => {
                return Err(ResolverError::UnsupportedConfigStrategy(strategy.to_owned()));
            }
        }
    } else {
        None
    };
    let abi_revision =
        fidl_fuchsia_component_abi_ext::read_abi_revision_optional(&dir, AbiRevision::PATH).await?;
    let dir = ClientEnd::new(
        dir.into_channel().map_err(|_| ResolverError::DirectoryProxyIntoChannel)?.into_zx_channel(),
    );
    Ok(fresolution::Component {
        url: Some(component_url.to_string()),
        resolution_context: Some(fresolution::Context { bytes: outgoing_context.bytes }),
        decl: Some(manifest_data),
        package: Some(fresolution::Package {
            url: Some(component_url.package_url().to_string()),
            directory: Some(dir),
            ..Default::default()
        }),
        config_values,
        abi_revision: abi_revision.map(Into::into),
        ..Default::default()
    })
}

#[derive(thiserror::Error, Debug)]
enum ResolverError {
    #[error("invalid component URL")]
    InvalidUrl(#[from] fuchsia_url::errors::ParseError),

    #[error("manifest not found")]
    ManifestNotFound(#[source] mem_util::FileError),

    #[error("config values not found")]
    ConfigValuesNotFound(#[source] mem_util::FileError),

    #[error("IO error")]
    IoError(#[source] fidl::Error),

    #[error("failed to deal with fuchsia.mem.Data")]
    ReadingManifest(#[source] mem_util::DataError),

    #[error("failed to parse compiled manifest to check for config")]
    ParsingManifest(#[source] fidl::Error),

    #[error("component has config fields but does not have a config value lookup strategy")]
    MissingConfigSource,

    #[error("unsupported config value resolution strategy {_0:?}")]
    UnsupportedConfigStrategy(fdecl::ConfigValueSource),

    #[error("resolving the package {0:?}")]
    PackageResolve(fpkg::ResolveError),

    #[error("converting package directory proxy into an async channel")]
    DirectoryProxyIntoChannel,

    #[error("failed to read abi revision file")]
    AbiRevision(#[from] fidl_fuchsia_component_abi_ext::AbiRevisionFileError),
}

impl From<&ResolverError> for fresolution::ResolverError {
    fn from(err: &ResolverError) -> Self {
        use ResolverError::*;
        use fresolution::ResolverError as ferr;
        match err {
            DirectoryProxyIntoChannel => ferr::Internal,
            InvalidUrl(_) => ferr::InvalidArgs,
            ManifestNotFound { .. } => ferr::ManifestNotFound,
            ConfigValuesNotFound { .. } => ferr::ConfigValuesNotFound,
            ReadingManifest(_) | IoError(_) => ferr::Io,
            ParsingManifest(..) | MissingConfigSource | UnsupportedConfigStrategy(..) => {
                ferr::InvalidManifest
            }
            PackageResolve(e) => {
                use fidl_fuchsia_pkg::ResolveError as PkgErr;
                match e {
                    PkgErr::PackageNotFound | PkgErr::BlobNotFound => ferr::PackageNotFound,
                    PkgErr::RepoNotFound
                    | PkgErr::UnavailableBlob
                    | PkgErr::UnavailableRepoMetadata => ferr::ResourceUnavailable,
                    PkgErr::NoSpace => ferr::NoSpace,
                    PkgErr::AccessDenied | PkgErr::Internal => ferr::Internal,
                    PkgErr::Io => ferr::Io,
                    PkgErr::InvalidUrl | PkgErr::InvalidContext => ferr::InvalidArgs,
                }
            }
            AbiRevision(_) => ferr::InvalidAbiRevision,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Error;
    use assert_matches::assert_matches;
    use fidl::endpoints::ServerEnd;
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use fuchsia_component::server as fserver;
    use fuchsia_component_test::{
        Capability, ChildOptions, LocalComponentHandles, RealmBuilder, Ref, Route,
    };
    use futures::channel::mpsc;
    use futures::lock::Mutex;
    use std::sync;
    use std::sync::Arc;
    use vfs::execution_scope::ExecutionScope;
    use vfs::file::vmo::read_only;
    use vfs::pseudo_directory;

    type Trigger = mpsc::Sender<Result<(), Error>>;

    async fn mock_pkg_cache(
        trigger: Arc<Mutex<Option<Trigger>>>,
        handles: LocalComponentHandles,
    ) -> Result<(), Error> {
        let mut fs = fserver::ServiceFs::new();
        fs.dir("svc").add_fidl_service(move |mut req_stream: fpkg::PackageCacheRequestStream| {
            let tx = trigger.clone();
            fasync::Task::local(async move {
                while let Some(request) = req_stream.try_next().await.unwrap() {
                    match request {
                        fpkg::PackageCacheRequest::Get { responder, .. } => {
                            responder.send(Err(zx::Status::NOT_FOUND.into_raw())).unwrap();
                            {
                                let mut lock = tx.lock().await;
                                let mut c = lock.take().unwrap();
                                c.send(Ok(())).await.unwrap();
                                lock.replace(c);
                            }
                        }
                        fpkg::PackageCacheRequest::BasePackageIndex { iterator, .. } => {
                            mock_iterator(&[], iterator);
                        }
                        fpkg::PackageCacheRequest::CachePackageIndex { iterator, .. } => {
                            mock_iterator(&["test-pkg-request"], iterator);
                        }
                        r => {
                            panic!("unexpected pkg-cache request: {r:#?}");
                        }
                    }
                }
            })
            .detach();
        });

        fs.serve_connection(handles.outgoing_dir)?;
        fs.collect::<()>().await;
        Ok(())
    }

    fn mock_iterator(contents: &[&str], iterator: ServerEnd<fpkg::PackageIndexIteratorMarker>) {
        let contents: Vec<_> = contents
            .iter()
            .map(|name| fpkg::PackageIndexEntry {
                package_url: fpkg::PackageUrl { url: format!("fuchsia-pkg://fuchsia.com/{name}") },
                meta_far_blob_id: fpkg::BlobId { merkle_root: [0xff; 32] },
            })
            .collect();
        fasync::Task::local(async move {
            let mut iterator = iterator.into_stream();
            if !contents.is_empty() {
                let fpkg::PackageIndexIteratorRequest::Next { responder, .. } =
                    iterator.try_next().await.unwrap().unwrap();
                {
                    responder.send(&contents).unwrap();
                }
            }
            let fpkg::PackageIndexIteratorRequest::Next { responder, .. } =
                iterator.try_next().await.unwrap().unwrap();
            {
                responder.send(&[]).unwrap();
            }
        })
        .detach();
    }

    async fn component_requester(
        trigger: Arc<Mutex<Option<Trigger>>>,
        url: String,
        handles: LocalComponentHandles,
    ) -> Result<(), Error> {
        let resolver_proxy: fresolution::ResolverProxy = handles.connect_to_protocol()?;
        let _ = resolver_proxy.resolve(&url).await?;
        fasync::Task::local(async move {
            let mut lock = trigger.lock().await;
            let mut c = lock.take().unwrap();
            c.send(Ok(())).await.expect("sending oneshot from package requester failed");
            lock.replace(c);
        })
        .detach();
        Ok(())
    }

    #[fuchsia::test]
    async fn fidl_wiring_and_serving() {
        const OUT_DIR_FLAGS: fio::Flags =
            fio::PERM_READABLE.union(fio::PERM_WRITABLE).union(fio::PERM_EXECUTABLE);

        // Mocks for directory dependencies
        let directories_out_dir = vfs::pseudo_directory! {
            "config" => vfs::pseudo_directory! {
                "data" => vfs::pseudo_directory! {
                },
                "build-info" => vfs::pseudo_directory! {
                    "build" => read_only(b"test")
                },
            },
        };
        let directories_out_dir = sync::Mutex::new(Some(directories_out_dir));

        let (sender, mut receiver) = mpsc::channel(2);
        let sender = Arc::new(Mutex::new(Some(sender)));
        let builder = RealmBuilder::new().await.expect("Failed to create test realm builder");
        let pkg_resolver = builder
            .add_child("pkg-resolver", "#meta/pkg-resolver.cm", ChildOptions::new())
            .await
            .unwrap();
        let directories_component = builder
            .add_local_child(
                "directories",
                move |handles| {
                    let directories_out_dir = directories_out_dir
                        .lock()
                        .unwrap()
                        .take()
                        .expect("mock component should only be launched once");
                    let scope = vfs::ExecutionScope::new();
                    vfs::directory::serve_on(
                        directories_out_dir,
                        OUT_DIR_FLAGS,
                        scope.clone(),
                        handles.outgoing_dir,
                    );
                    async move {
                        scope.wait().await;
                        Ok(())
                    }
                    .boxed()
                },
                ChildOptions::new(),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::directory("config-data")
                            .path("/config/data")
                            .rights(fio::R_STAR_DIR),
                    )
                    .from(&directories_component)
                    .to(&pkg_resolver),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(
                        Capability::directory("build-info")
                            .rights(fio::R_STAR_DIR)
                            .path("/config/build-info"),
                    )
                    .from(&directories_component)
                    .to(&pkg_resolver),
            )
            .await
            .unwrap();

        let fake_pkg_cache = builder
            .add_local_child(
                "fake-pkg-cache",
                {
                    let sender = sender.clone();
                    move |handles: LocalComponentHandles| {
                        Box::pin(mock_pkg_cache(sender.clone(), handles))
                    }
                },
                ChildOptions::new(),
            )
            .await
            .expect("Failed adding base resolver mock");
        let requesting_component = builder
            .add_local_child(
                "requesting-component",
                {
                    let sender = sender.clone();
                    move |handles: LocalComponentHandles| {
                        Box::pin(component_requester(
                            sender.clone(),
                            "fuchsia-pkg://fuchsia.com/test-pkg-request#meta/test-component.cm"
                                .to_owned(),
                            handles,
                        ))
                    }
                },
                ChildOptions::new().eager(),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.pkg.PackageCache"))
                    .from(&fake_pkg_cache)
                    .to(&pkg_resolver),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name(
                        "fuchsia.component.resolution.Resolver",
                    ))
                    .from(&pkg_resolver)
                    .to(&requesting_component),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                    .from(Ref::parent())
                    .to(&pkg_resolver)
                    .to(&fake_pkg_cache)
                    .to(&requesting_component),
            )
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration(
                        "fuchsia.pkgresolver.TufNetworkHeaderTimeoutSeconds",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.pkgresolver.BlobNetworkHeaderTimeoutSeconds",
                    ))
                    .capability(Capability::configuration(
                        "fuchsia.pkgresolver.BlobNetworkBodyTimeoutSeconds",
                    ))
                    .from(Ref::void())
                    .to(&pkg_resolver),
            )
            .await
            .unwrap();
        let _test_topo = builder.build().await.unwrap();

        receiver.next().await.expect("Unexpected error waiting for response").expect("error sent");
        receiver.next().await.expect("Unexpected error waiting for response").expect("error sent");
    }

    #[fuchsia::test]
    async fn resolve_component_without_context_forwards_to_pkg_resolver_and_returns_context() {
        let (proxy, mut server) =
            fidl::endpoints::create_proxy_and_stream::<fpkg::PackageResolverMarker>();
        let server = async move {
            let cm_bytes = fidl::persist(&fdecl::Component::default().clone()).unwrap();
            let fs = pseudo_directory! {
                "meta" => pseudo_directory! {
                    "test.cm" => read_only(cm_bytes),
                },
            };
            match server.try_next().await.unwrap().expect("client makes one request") {
                fpkg::PackageResolverRequest::Resolve { package_url, dir, responder } => {
                    assert_eq!(package_url, "fuchsia-pkg://fuchsia.example/test");
                    vfs::directory::serve_on(
                        fs.clone(),
                        fio::PERM_READABLE,
                        ExecutionScope::new(),
                        dir,
                    );
                    responder
                        .send(Ok(&fpkg::ResolutionContext { bytes: b"context-contents".to_vec() }))
                        .unwrap();
                }
                _ => panic!("unexpected API call"),
            }
            assert_matches!(server.try_next().await, Ok(None));
        };
        let client = async move {
            assert_matches!(
                resolve_component_without_context(
                    "fuchsia-pkg://fuchsia.example/test#meta/test.cm",
                    &proxy
                )
                .await,
                Ok(fresolution::Component {
                    decl: Some(fidl_fuchsia_mem::Data::Buffer(fidl_fuchsia_mem::Buffer { .. })),
                    resolution_context: Some(fresolution::Context { bytes }),
                    ..
                })
                    if bytes == b"context-contents".to_vec()
            );
        };
        let ((), ()) = futures::join!(server, client);
    }

    #[fuchsia::test]
    async fn resolve_component_with_context_forwards_to_pkg_resolver_and_returns_context() {
        let (proxy, mut server) =
            fidl::endpoints::create_proxy_and_stream::<fpkg::PackageResolverMarker>();
        let server = async move {
            let cm_bytes = fidl::persist(&fdecl::Component::default().clone()).unwrap();
            let fs = pseudo_directory! {
                "meta" => pseudo_directory! {
                    "test.cm" => read_only(cm_bytes),
                },
            };
            match server.try_next().await.unwrap().expect("client makes one request") {
                fpkg::PackageResolverRequest::ResolveWithContext {
                    package_url,
                    context,
                    dir,
                    responder,
                } => {
                    assert_eq!(package_url, "fuchsia-pkg://fuchsia.example/test");
                    assert_eq!(
                        context,
                        fpkg::ResolutionContext { bytes: b"incoming-context".to_vec() }
                    );
                    vfs::directory::serve_on(
                        fs.clone(),
                        fio::PERM_READABLE,
                        ExecutionScope::new(),
                        dir,
                    );
                    responder
                        .send(Ok(&fpkg::ResolutionContext { bytes: b"outgoing-context".to_vec() }))
                        .unwrap();
                }
                _ => panic!("unexpected API call"),
            }
            assert_matches!(server.try_next().await, Ok(None));
        };
        let client = async move {
            assert_matches!(
                resolve_component_with_context(
                    "fuchsia-pkg://fuchsia.example/test#meta/test.cm",
                    &fresolution::Context{ bytes: b"incoming-context".to_vec()},
                    &proxy
                )
                .await,
                Ok(fresolution::Component {
                    decl: Some(fidl_fuchsia_mem::Data::Buffer(fidl_fuchsia_mem::Buffer { .. })),
                    resolution_context: Some(fresolution::Context { bytes }),
                    ..
                })
                    if bytes == b"outgoing-context".to_vec()
            );
        };
        let ((), ()) = futures::join!(server, client);
    }

    #[fuchsia::test]
    async fn resolve_component_without_context_fails_bad_connection() {
        let (proxy, _) = fidl::endpoints::create_proxy_and_stream::<fpkg::PackageResolverMarker>();
        assert_matches!(
            resolve_component_without_context(
                "fuchsia-pkg://fuchsia.example/test#meta/test.cm",
                &proxy
            )
            .await,
            Err(ResolverError::IoError(_))
        );
    }

    #[fuchsia::test]
    async fn resolve_component_with_context_fails_bad_connection() {
        let (proxy, _) = fidl::endpoints::create_proxy_and_stream::<fpkg::PackageResolverMarker>();
        assert_matches!(
            resolve_component_with_context(
                "fuchsia-pkg://fuchsia.example/test#meta/test.cm",
                &fresolution::Context { bytes: vec![] },
                &proxy
            )
            .await,
            Err(ResolverError::IoError(_))
        );
    }

    #[fuchsia::test]
    async fn resolve_component_without_context_fails_with_package_resolver_failure() {
        let (proxy, mut server) =
            fidl::endpoints::create_proxy_and_stream::<fpkg::PackageResolverMarker>();
        let server = async move {
            match server.try_next().await.unwrap().expect("client makes one request") {
                fpkg::PackageResolverRequest::Resolve { responder, .. } => {
                    responder.send(Err(fpkg::ResolveError::NoSpace)).unwrap();
                }
                _ => panic!("unexpected API call"),
            }
            assert_matches!(server.try_next().await, Ok(None));
        };
        let client = async move {
            assert_matches!(
                resolve_component_without_context(
                    "fuchsia-pkg://fuchsia.com/test#meta/test.cm",
                    &proxy
                )
                .await,
                Err(ResolverError::PackageResolve(fpkg::ResolveError::NoSpace))
            );
        };
        let ((), ()) = futures::join!(server, client);
    }

    #[fuchsia::test]
    async fn resolve_component_with_context_fails_with_package_resolver_failure() {
        let (proxy, mut server) =
            fidl::endpoints::create_proxy_and_stream::<fpkg::PackageResolverMarker>();
        let server = async move {
            match server.try_next().await.unwrap().expect("client makes one request") {
                fpkg::PackageResolverRequest::ResolveWithContext { responder, .. } => {
                    responder.send(Err(fpkg::ResolveError::NoSpace)).unwrap();
                }
                _ => panic!("unexpected API call"),
            }
            assert_matches!(server.try_next().await, Ok(None));
        };
        let client = async move {
            assert_matches!(
                resolve_component_with_context(
                    "fuchsia-pkg://fuchsia.com/test#meta/test.cm",
                    &fresolution::Context { bytes: vec![] },
                    &proxy
                )
                .await,
                Err(ResolverError::PackageResolve(fpkg::ResolveError::NoSpace))
            );
        };
        let ((), ()) = futures::join!(server, client);
    }

    #[fuchsia::test]
    async fn resolve_component_fails_with_component_not_found() {
        let fs = pseudo_directory! {};
        let dir = vfs::directory::serve_read_only(fs, ExecutionScope::new());
        assert_matches!(
            resolve_component(
                &"fuchsia-pkg://fuchsia.com/test#meta/test.cm".parse().unwrap(),
                dir,
                fpkg::ResolutionContext { bytes: vec![] }
            )
            .await,
            Err(ResolverError::ManifestNotFound(..))
        );
    }

    #[fuchsia::test]
    async fn resolve_component_succeeds_with_config() {
        let cm_bytes = fidl::persist(&fdecl::Component {
            config: Some(fdecl::ConfigSchema {
                value_source: Some(fdecl::ConfigValueSource::PackagePath(
                    "meta/test_with_config.cvf".to_owned(),
                )),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();
        let expected_config = fdecl::ConfigValuesData {
            values: Some(vec![fdecl::ConfigValueSpec {
                value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Uint8(3))),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let cvf_bytes = fidl::persist(&expected_config.clone()).unwrap();
        let fs = pseudo_directory! {
            "meta" => pseudo_directory! {
                "test_with_config.cm" => read_only(cm_bytes),
                "test_with_config.cvf" => read_only(cvf_bytes),
            }
        };
        let dir = vfs::directory::serve_read_only(fs, ExecutionScope::new());
        assert_matches!(
            resolve_component(
                &"fuchsia-pkg://fuchsia.example/test#meta/test_with_config.cm".parse().unwrap(),
                dir,
                fpkg::ResolutionContext{ bytes: vec![]}
            )
            .await
            .unwrap(),
            fresolution::Component {
                decl: Some(fidl_fuchsia_mem::Data::Buffer(fidl_fuchsia_mem::Buffer { .. })),
                config_values: Some(data),
                ..
            }
                if {
                    let raw_bytes = mem_util::bytes_from_data(&data).unwrap();
                    let actual_config: fdecl::ConfigValuesData = fidl::unpersist(&raw_bytes[..]).unwrap();
                    assert_eq!(actual_config, expected_config);
                    true
                }
        );
    }

    #[fuchsia::test]
    async fn resolve_component_fails_missing_config_value_file() {
        let cm_bytes = fidl::persist(&fdecl::Component {
            config: Some(fdecl::ConfigSchema {
                value_source: Some(fdecl::ConfigValueSource::PackagePath(
                    "meta/test_with_config.cvf".to_string(),
                )),
                ..Default::default()
            }),
            ..Default::default()
        })
        .unwrap();
        let fs = pseudo_directory! {
            "meta" => pseudo_directory! {
                "test_with_config.cm" => read_only(cm_bytes),
            },
        };
        let dir = vfs::directory::serve_read_only(fs, ExecutionScope::new());
        assert_matches!(
            resolve_component(
                &"fuchsia-pkg://fuchsia.example/test#meta/test_with_config.cm".parse().unwrap(),
                dir,
                fpkg::ResolutionContext { bytes: vec![] }
            )
            .await,
            Err(ResolverError::ConfigValuesNotFound(_))
        );
    }

    #[fuchsia::test]
    async fn resolve_component_fails_bad_config_strategy() {
        let cm_bytes = fidl::persist(&fdecl::Component {
            config: Some(fdecl::ConfigSchema::default().clone()),
            ..Default::default()
        })
        .unwrap();
        let cvf_bytes = fidl::persist(&fdecl::ConfigValuesData::default().clone()).unwrap();
        let fs = pseudo_directory! {
            "meta" => pseudo_directory! {
                "test_with_config.cm" => read_only(cm_bytes),
                "test_with_config.cvf" => read_only(cvf_bytes),
            },
        };
        let dir = vfs::directory::serve_read_only(fs, ExecutionScope::new());
        assert_matches!(
            resolve_component(
                &"fuchsia-pkg://fuchsia.com/test#meta/test_with_config.cm".parse().unwrap(),
                dir,
                fpkg::ResolutionContext { bytes: vec![] }
            )
            .await,
            Err(ResolverError::MissingConfigSource)
        );
    }

    #[fuchsia::test]
    async fn resolve_component_sets_pkg_abi_revision() {
        let cm_bytes = fidl::persist(&fdecl::Component::default().clone())
            .expect("failed to encode ComponentDecl FIDL");
        let fs = pseudo_directory! {
            "meta" => pseudo_directory! {
                "test.cm" => read_only(cm_bytes),
                "fuchsia.abi" => pseudo_directory! {
                  "abi-revision" => read_only(1u64.to_le_bytes()),
                }
            },
        };
        let dir = vfs::directory::serve_read_only(fs, ExecutionScope::new());
        let resolved_component = resolve_component(
            &"fuchsia-pkg://fuchsia.com/test#meta/test.cm".parse().unwrap(),
            dir,
            fpkg::ResolutionContext { bytes: vec![] },
        )
        .await
        .unwrap();
        assert_matches!(resolved_component, fresolution::Component { abi_revision: Some(1), .. });
    }
}
