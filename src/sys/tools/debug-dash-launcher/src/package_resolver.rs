// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::DiscoverableProtocolMarker as _;
use log::{error, warn};
use std::collections::HashMap;
use {fidl_fuchsia_dash as fdash, fidl_fuchsia_io as fio, fidl_fuchsia_pkg as fpkg};

pub(crate) struct PackageResolver {
    fuchsia_pkg_resolver: fdash::FuchsiaPkgResolver,
    resolvers: HashMap<ResolverBackend, fpkg::PackageResolverProxy>,
}

// The available package resolver backends, selected between by the package URL scheme and the
// `fdash::FuchsiaPkgResolver` provided by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ResolverBackend {
    Boot,
    Base,
    Full,
}

impl ResolverBackend {
    fn capability_path(&self) -> String {
        let suffix = match self {
            Self::Boot => "boot",
            Self::Base => "base",
            Self::Full => "full",
        };
        format!("/svc/{}-{suffix}", fpkg::PackageResolverMarker::PROTOCOL_NAME)
    }
}

impl PackageResolver {
    /// Creates a `PackageResolver`.
    ///
    /// `fuchsia_pkg_resolver` is the resolver backend to use when resolving "fuchsia-pkg://"
    /// package URLs.
    pub(crate) fn new(fuchsia_pkg_resolver: fdash::FuchsiaPkgResolver) -> Self {
        Self { fuchsia_pkg_resolver, resolvers: HashMap::new() }
    }

    /// Resolves `url` and returns a proxy to the package directory.
    pub(crate) async fn resolve(&mut self, url: &str) -> Result<fio::DirectoryProxy, Error> {
        self.resolve_subpackage(url, &[]).await
    }

    /// Resolves `url` and the chain of `subpackages`, if any, and returns a proxy to the final
    /// package directory.
    pub(crate) async fn resolve_subpackage(
        &mut self,
        url: &str,
        subpackages: &[String],
    ) -> Result<fio::DirectoryProxy, Error> {
        let (mut dir, server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let resolver = self.get_resolver(url)?;
        let mut context = resolver.resolve(url, server).await?.map_err(Error::Application)?;
        for subpackage in subpackages {
            let (sub_dir, server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            context = resolver
                .resolve_with_context(subpackage, &context, server)
                .await?
                .map_err(Error::Application)?;
            dir = sub_dir;
        }
        Ok(dir)
    }

    fn get_resolver<'a>(&'a mut self, url: &str) -> Result<&'a fpkg::PackageResolverProxy, Error> {
        let backend = self.get_resolver_backend(url)?;
        use std::collections::hash_map::Entry::*;
        Ok(match self.resolvers.entry(backend) {
            Occupied(o) => o.into_mut(),
            Vacant(v) => {
                let resolver = fuchsia_component::client::connect_to_protocol_at_path::<
                    fpkg::PackageResolverMarker,
                >(&backend.capability_path())
                .map_err(|e| {
                    error!(
                        source:?=e, path=backend.capability_path().as_str(); "connecting to resolver"
                    );
                    Error::Application(fpkg::ResolveError::Internal)
                })?;
                v.insert(resolver)
            }
        })
    }

    fn get_resolver_backend(&self, url: &str) -> Result<ResolverBackend, Error> {
        let url = url::Url::parse(url).map_err(|e| {
            log::warn!(url:%, source:?=e; "invalid url");
            Error::Application(fpkg::ResolveError::InvalidUrl)
        })?;
        Ok(match url.scheme() {
            "fuchsia-pkg" => match self.fuchsia_pkg_resolver {
                fdash::FuchsiaPkgResolver::Base => ResolverBackend::Base,
                fdash::FuchsiaPkgResolver::Full => ResolverBackend::Full,
                fdash::FuchsiaPkgResolverUnknown!() => {
                    warn!(
                        "unknown fuchsia-pkg resolver: {}",
                        self.fuchsia_pkg_resolver.into_primitive()
                    );
                    return Err(Error::Application(fpkg::ResolveError::InvalidUrl));
                }
            },
            "fuchsia-boot" => ResolverBackend::Boot,
            scheme => {
                warn!(scheme:%, url:%; "unknown package url scheme");
                return Err(Error::Application(fpkg::ResolveError::InvalidUrl));
            }
        })
    }

    #[cfg(test)]
    /// Will use `resolver` to resolve `fuchsia-pkg://` URLs.
    pub(crate) fn new_test(resolver: fpkg::PackageResolverProxy) -> Self {
        let resolvers = HashMap::from([(ResolverBackend::Full, resolver)]);
        Self { fuchsia_pkg_resolver: fdash::FuchsiaPkgResolver::Full, resolvers }
    }
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("fuchsia.pkg/PackageResolver fidl error")]
    Fidl(#[from] fidl::Error),

    #[error("fuchsia.pkg/PackageResolver application error: {0:?}")]
    Application(fpkg::ResolveError),
}

impl Error {
    pub(crate) fn while_resolving_package_to_explore(self) -> fdash::LauncherError {
        match self {
            Self::Application(fpkg::ResolveError::PackageNotFound) => {
                fdash::LauncherError::ResolveTargetPackage
            }
            _ => fdash::LauncherError::PackageResolver,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async as fasync;
    use futures::stream::StreamExt as _;

    #[fuchsia::test]
    async fn chain_subpackage_resolves() {
        let (resolver, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fpkg::PackageResolverMarker>();
        let mut resolver = crate::package_resolver::PackageResolver::new_test(resolver);

        // A mock package resolver that records all requests in `requests`.
        let requests = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
        let requests_clone = requests.clone();
        fasync::Task::local(async move {
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fpkg::PackageResolverRequest::Resolve { package_url, dir: _, responder } => {
                        requests_clone.borrow_mut().push((package_url.clone(), None));
                        responder
                            .send(Ok(&fidl_fuchsia_pkg::ResolutionContext {
                                bytes: package_url.as_bytes().to_vec(),
                            }))
                            .unwrap();
                    }
                    fpkg::PackageResolverRequest::ResolveWithContext {
                        package_url,
                        context,
                        dir: _,
                        responder,
                    } => {
                        requests_clone.borrow_mut().push((package_url.clone(), Some(context)));
                        responder
                            .send(Ok(&fidl_fuchsia_pkg::ResolutionContext {
                                bytes: package_url.as_bytes().to_vec(),
                            }))
                            .unwrap();
                    }
                    req => panic!("unexpected request {req:?}"),
                }
            }
        })
        .detach();

        let _: fio::DirectoryProxy = resolver
            .resolve_subpackage("fuchsia-pkg://full-url", &["a".into(), "b".into()])
            .await
            .unwrap();

        assert_eq!(
            *requests.borrow_mut(),
            vec![
                ("fuchsia-pkg://full-url".into(), None),
                (
                    "a".into(),
                    Some(fpkg::ResolutionContext { bytes: "fuchsia-pkg://full-url".into() })
                ),
                ("b".into(), Some(fpkg::ResolutionContext { bytes: "a".into() })),
            ]
        );
    }
}
