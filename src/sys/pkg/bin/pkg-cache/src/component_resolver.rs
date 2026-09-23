// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use fidl::endpoints::Proxy as _;
use fidl_fuchsia_component_decl as fcomponent_decl;
use fidl_fuchsia_component_resolution as fcomponent_resolution;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_pkg as fpkg;
use fuchsia_url::fuchsia_pkg::{AbsolutePackageUrl, ComponentUrl, PackageUrl};
use futures::stream::TryStreamExt as _;
use log::{error, warn};
use std::sync::Arc;
use version_history::AbiRevision;

/// Abstracts package resolution from the component resolution process.
/// Component resolution is just package resolution plus copying some data out of the package into
/// the structure returned by the component resolution FIDL, so by abstracting package resolution
/// we can reuse the following component resolution FIDL serving code with each component resolver.
pub(crate) trait PackageResolver {
    type Error: ToFidlError + std::error::Error + Send + Sync + 'static;

    async fn resolve_and_serve(
        &self,
        url: &AbsolutePackageUrl,
        dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    ) -> Result<fpkg::ResolutionContext, Self::Error>;

    async fn resolve_with_context_and_serve(
        &self,
        url: &PackageUrl,
        context: fpkg::ResolutionContext,
        dir: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    ) -> Result<fpkg::ResolutionContext, Self::Error>;
}

/// This is just `impl Into<fcomponent_resolution::ResolverError> for &Self`, except the bound can
/// be placed on the error type item in the `PackageResolver` trait. Possibly this can be replaced
/// with higher rank bounds somehow.
pub(crate) trait ToFidlError {
    fn to_fidl_error(&self) -> fcomponent_resolution::ResolverError;
}

impl<T> ToFidlError for T
where
    for<'a> &'a T: Into<fcomponent_resolution::ResolverError>,
{
    fn to_fidl_error(&self) -> fcomponent_resolution::ResolverError {
        self.into()
    }
}

pub(crate) async fn serve_request_stream(
    stream: fcomponent_resolution::ResolverRequestStream,
    package_resolver: Arc<impl PackageResolver>,
    log_tag: &'static str,
) -> anyhow::Result<()> {
    stream
        .map_err(anyhow::Error::new)
        .try_for_each_concurrent(None, |req| async {
            match req {
                fcomponent_resolution::ResolverRequest::Resolve { component_url, responder } => {
                    responder
                        .send(
                            resolve(&component_url, package_resolver.as_ref())
                                .await
                                .map_err(|e| {
                                    let fidl_err = (&e).into();
                                    error!(
                                        "{log_tag} failed to resolve {component_url}: {:#}",
                                        anyhow::anyhow!(e)
                                    );
                                    fidl_err
                                }),
                        )
                        .with_context(|| format!("{log_tag} sending Resolve response"))
                }
                fcomponent_resolution::ResolverRequest::ResolveWithContext {
                    component_url,
                    context,
                    responder,
                } => responder
                    .send(
                        resolve_with_context(
                            &component_url,
                            context,
                            package_resolver.as_ref(),
                        )
                        .await
                        .map_err(|e| {
                            let fidl_err = (&e).into();
                            error!(
                                "{log_tag} failed to resolve with context {component_url}: {:#}",
                                anyhow::anyhow!(e)
                            );
                            fidl_err
                        }),
                    )
                    .with_context(|| format!("{log_tag} sending ResolveWithContext response")),
                fcomponent_resolution::ResolverRequest::_UnknownMethod { ordinal, .. } => {
                    warn!(ordinal:%; "{log_tag} received unknown Resolver request");
                    Ok(())
                }
            }
        })
        .await
}

async fn resolve(
    url: &str,
    package_resolver: &impl PackageResolver,
) -> Result<fcomponent_resolution::Component, Error> {
    let url = ComponentUrl::parse(url).map_err(Error::InvalidUrl)?;
    let (package, server_end) = fidl::endpoints::create_proxy();
    let context = package_resolver
        .resolve_and_serve(
            match url.package_url() {
                PackageUrl::Absolute(url) => url,
                PackageUrl::Relative(_) => Err(Error::AbsoluteUrlRequired)?,
            },
            server_end,
        )
        .await
        .map_err(|e| Error::PackageResolve(e.to_fidl_error(), anyhow::anyhow!(e)))?;
    resolve_from_package(&url, package, fcomponent_resolution::Context { bytes: context.bytes })
        .await
}

async fn resolve_with_context(
    url: &str,
    context: fcomponent_resolution::Context,
    package_resolver: &impl PackageResolver,
) -> Result<fcomponent_resolution::Component, Error> {
    let url = ComponentUrl::parse(url).map_err(Error::InvalidUrl)?;
    let (package, server_end) = fidl::endpoints::create_proxy();
    let context = package_resolver
        .resolve_with_context_and_serve(
            url.package_url(),
            fpkg::ResolutionContext { bytes: context.bytes },
            server_end,
        )
        .await
        .map_err(|e| Error::PackageResolve(e.to_fidl_error(), anyhow::anyhow!(e)))?;
    resolve_from_package(&url, package, fcomponent_resolution::Context { bytes: context.bytes })
        .await
}

async fn load_config(
    decl: &fcomponent_decl::Component,
    package: &fio::DirectoryProxy,
) -> Result<Option<fidl_fuchsia_mem::Data>, Error> {
    let Some(config_decl) = decl.config.as_ref() else {
        return Ok(None);
    };
    let strategy = config_decl.value_source.as_ref().ok_or(Error::InvalidConfigSource)?;
    let config_path = match strategy {
        fcomponent_decl::ConfigValueSource::Capabilities(_) => return Ok(None),
        fcomponent_decl::ConfigValueSource::PackagePath(path) => path,
        other => return Err(Error::UnsupportedConfigSource(other.to_owned())),
    };

    Ok(Some(
        mem_util::open_file_data(package, config_path)
            .await
            .map_err(Error::ConfigValuesNotFound)?,
    ))
}

// TODO(https://fxbug.dev/548131664): Read manifest, etc. directly from the root_dir, not the proxy.
async fn resolve_from_package(
    url: &ComponentUrl,
    package: fio::DirectoryProxy,
    outgoing_context: fcomponent_resolution::Context,
) -> Result<fcomponent_resolution::Component, Error> {
    let data = mem_util::open_file_data(&package, url.resource())
        .await
        .map_err(Error::ComponentNotFound)?;
    let decl: fcomponent_decl::Component =
        fidl::unpersist(mem_util::bytes_from_data(&data).map_err(Error::ReadManifest)?.as_ref())
            .map_err(Error::ParsingManifest)?;
    let config_values = load_config(&decl, &package).await?;
    let abi_revision =
        fidl_fuchsia_component_abi_ext::read_abi_revision_optional(&package, AbiRevision::PATH)
            .await
            .map_err(Error::AbiRevision)?;
    Ok(fcomponent_resolution::Component {
        url: Some(url.to_string()),
        resolution_context: Some(outgoing_context),
        decl: Some(data),
        package: Some(fcomponent_resolution::Package {
            url: Some(url.package_url().to_string()),
            directory: Some(
                package
                    .into_channel()
                    .map_err(|_| Error::ConvertProxyToChannel)?
                    .into_zx_channel()
                    .into(),
            ),
            ..Default::default()
        }),
        config_values,
        abi_revision: abi_revision.map(Into::into),
        ..Default::default()
    })
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("invalid URL")]
    InvalidUrl(#[source] fuchsia_url::errors::ParseError),

    #[error("component not found")]
    ComponentNotFound(#[source] mem_util::FileError),

    #[error("couldn't parse component manifest")]
    ParsingManifest(#[source] fidl::Error),

    #[error("couldn't find config values")]
    ConfigValuesNotFound(#[source] mem_util::FileError),

    #[error("config source missing or invalid")]
    InvalidConfigSource,

    #[error("resolving the package")]
    PackageResolve(fcomponent_resolution::ResolverError, #[source] anyhow::Error),

    #[error("unsupported config source: {0:?}")]
    UnsupportedConfigSource(fcomponent_decl::ConfigValueSource),

    #[error("failed to read the manifest")]
    ReadManifest(#[source] mem_util::DataError),

    #[error("failed to read abi revision")]
    AbiRevision(#[source] fidl_fuchsia_component_abi_ext::AbiRevisionFileError),

    #[error("resolve must be called with an absolute (not relative) url")]
    AbsoluteUrlRequired,

    #[error("failed to convert proxy to channel")]
    ConvertProxyToChannel,
}

impl From<&Error> for fcomponent_resolution::ResolverError {
    fn from(err: &Error) -> fcomponent_resolution::ResolverError {
        use Error::*;
        use fcomponent_resolution::ResolverError as ferror;
        match err {
            InvalidUrl(_) | AbsoluteUrlRequired => ferror::InvalidArgs,
            ComponentNotFound(_) => ferror::ManifestNotFound,
            PackageResolve(fidl, _) => *fidl,
            ConfigValuesNotFound(_) => ferror::ConfigValuesNotFound,
            ParsingManifest(_) | UnsupportedConfigSource(_) | InvalidConfigSource => {
                ferror::InvalidManifest
            }
            ReadManifest(_) => ferror::Io,
            ConvertProxyToChannel => ferror::Internal,
            AbiRevision(_) => ferror::InvalidAbiRevision,
        }
    }
}

impl From<Error> for fcomponent_resolution::ResolverError {
    fn from(err: Error) -> fcomponent_resolution::ResolverError {
        (&err).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[derive(thiserror::Error, Debug)]
    enum BrokenPackageResolverError {}
    impl ToFidlError for BrokenPackageResolverError {
        fn to_fidl_error(&self) -> fcomponent_resolution::ResolverError {
            unimplemented!();
        }
    }

    struct BrokenPackageResolver;
    impl PackageResolver for BrokenPackageResolver {
        type Error = BrokenPackageResolverError;

        async fn resolve_and_serve(
            &self,
            _: &AbsolutePackageUrl,
            _: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
        ) -> Result<fpkg::ResolutionContext, BrokenPackageResolverError> {
            unimplemented!();
        }

        async fn resolve_with_context_and_serve(
            &self,
            _: &PackageUrl,
            _: fpkg::ResolutionContext,
            _: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
        ) -> Result<fpkg::ResolutionContext, BrokenPackageResolverError> {
            unimplemented!();
        }
    }

    #[fuchsia::test]
    async fn resolve_rejects_relative_url() {
        assert_matches!(
            resolve("relative#meta/missing", &BrokenPackageResolver,).await,
            Err(Error::AbsoluteUrlRequired)
        )
    }
}
