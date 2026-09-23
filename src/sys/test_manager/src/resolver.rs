// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use diagnostics_log::Publisher;
use fidl::endpoints::DiscoverableProtocolMarker;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::LocalComponentHandles;
use fuchsia_url::fuchsia_pkg::{ComponentUrl, PackageUrl};
use futures::{StreamExt, TryStreamExt};
use itertools::Itertools;
use log::{Log, warn};
use std::collections::HashSet;
use std::sync::Arc;
use {
    diagnostics_log as flog, fidl_fuchsia_component_resolution as fresolution,
    fidl_fuchsia_logger as flogger, fidl_fuchsia_pkg as fpkg, fuchsia_async as fasync,
};

// The list of non-hermetic packages allowed to resolved by a test.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllowedPackages {
    // Strict list of allowed packages.
    pkgs: Arc<HashSet<String>>,
}

impl AllowedPackages {
    pub fn zero_allowed_pkgs() -> Self {
        Self { pkgs: HashSet::new().into() }
    }

    pub fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        Self { pkgs: Arc::new(HashSet::from_iter(iter)) }
    }
}

async fn validate_hermetic_package(
    component_url_str: &str,
    logger: OptionLogger,
    hermetic_test_package_name: &String,
    other_allowed_packages: &AllowedPackages,
) -> Result<(), fresolution::ResolverError> {
    let component_url = ComponentUrl::parse(component_url_str).map_err(|err| {
        warn!("cannot parse {}, {:?}", component_url_str, err);
        fresolution::ResolverError::InvalidArgs
    })?;

    match component_url.package_url() {
        PackageUrl::Absolute(pkg_url) => {
            let package_name = pkg_url.name();
            if hermetic_test_package_name != package_name.as_ref()
                && !other_allowed_packages.pkgs.contains(package_name.as_ref())
            {
                let s = format!("failed to resolve component {}: package {} is not in the test package allowlist: '{}, {}'
                \nSee https://fuchsia.dev/fuchsia-src/development/testing/components/test_runner_framework?hl=en#hermetic-resolver
                for more information.",
                &component_url_str, package_name, hermetic_test_package_name, other_allowed_packages.pkgs.iter().join(", "));
                // log in both test managers log sink and test's log sink so that it is easy to retrieve.

                let mut builder = log::Record::builder();
                builder.level(log::Level::Warn);
                logger.log(&builder.args(format_args!("{}", s)).build());
                warn!("{}", s);
                return Err(fresolution::ResolverError::PackageNotFound);
            }
        }
        PackageUrl::Relative(_url) => {
            // don't do anything as we don't restrict relative urls.
        }
    }
    Ok(())
}

async fn validate_hermetic_url(
    pkg_url_str: &str,
    logger: OptionLogger,
    hermetic_test_package_name: &String,
    other_allowed_packages: &AllowedPackages,
) -> Result<(), fpkg::ResolveError> {
    let pkg_url = PackageUrl::parse(pkg_url_str).map_err(|err| {
        warn!("cannot parse {}, {:?}", pkg_url_str, err);
        fpkg::ResolveError::InvalidUrl
    })?;

    match pkg_url {
        PackageUrl::Absolute(pkg_url) => {
            let package_name = pkg_url.name();
            if hermetic_test_package_name != package_name.as_ref()
                && !other_allowed_packages.pkgs.contains(package_name.as_ref())
            {
                let s = format!("failed to resolve component {}: package {} is not in the test package allowlist: '{}, {}'
                \nSee https://fuchsia.dev/fuchsia-src/development/testing/components/test_runner_framework?hl=en#hermetic-resolver
                for more information.",
                &pkg_url_str, package_name, hermetic_test_package_name, other_allowed_packages.pkgs.iter().join(", "));
                // log in both test managers log sink and test's log sink so that it is easy to retrieve.
                let mut builder = log::Record::builder();
                builder.level(log::Level::Warn);
                logger.log(&builder.args(format_args!("{}", s)).build());
                warn!("{}", s);
                return Err(fpkg::ResolveError::PackageNotFound);
            }
        }
        PackageUrl::Relative(_url) => {
            // don't do anything as we don't restrict relative urls.
        }
    }
    Ok(())
}

async fn serve_resolver(
    mut stream: fresolution::ResolverRequestStream,
    logger: OptionLogger,
    hermetic_test_package_name: Arc<String>,
    other_allowed_packages: AllowedPackages,
    full_resolver: Arc<fresolution::ResolverProxy>,
) {
    while let Some(request) = stream.try_next().await.expect("failed to serve component resolver") {
        match request {
            fresolution::ResolverRequest::Resolve { component_url, responder } => {
                let result = if let Err(err) = validate_hermetic_package(
                    &component_url,
                    logger.clone(),
                    &hermetic_test_package_name,
                    &other_allowed_packages,
                )
                .await
                {
                    Err(err)
                } else {
                    let logger = logger.clone();
                    full_resolver.resolve(&component_url).await.unwrap_or_else(|err| {
                        let mut builder = log::Record::builder();
                        builder.level(log::Level::Warn);
                        logger.log(
                            &builder
                                .args(format_args!(
                                    "failed to resolve component {}: {:?}",
                                    component_url, err
                                ))
                                .build(),
                        );
                        Err(fresolution::ResolverError::Internal)
                    })
                };
                if let Err(e) = responder.send(result) {
                    warn!("Failed sending load response for {}: {}", component_url, e);
                }
            }
            fresolution::ResolverRequest::ResolveWithContext {
                component_url,
                context,
                responder,
            } => {
                // We don't need to worry about validating context because it should have
                // been produced by Resolve call above.
                let result = if let Err(err) = validate_hermetic_package(
                    &component_url,
                    logger.clone(),
                    &hermetic_test_package_name,
                    &other_allowed_packages,
                )
                .await
                {
                    Err(err)
                } else {
                    let logger = logger.clone();
                    full_resolver
                        .resolve_with_context(&component_url, &context)
                        .await
                        .unwrap_or_else(|err| {
                            let mut builder = log::Record::builder();
                            builder.level(log::Level::Warn);
                            logger.log(
                                &builder
                                    .args(format_args!(
                                        "failed to resolve component {} with context {:?}: {:?}",
                                        component_url, context, err
                                    ))
                                    .build(),
                            );
                            Err(fresolution::ResolverError::Internal)
                        })
                };
                if let Err(e) = responder.send(result) {
                    warn!("Failed sending load response for {}: {}", component_url, e);
                }
            }
            fresolution::ResolverRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "Unknown Resolver request");
            }
        }
    }
}

async fn serve_pkg_resolver(
    mut stream: fpkg::PackageResolverRequestStream,
    logger: OptionLogger,
    hermetic_test_package_name: Arc<String>,
    other_allowed_packages: AllowedPackages,
    pkg_resolver: Arc<fpkg::PackageResolverProxy>,
) {
    while let Some(request) = stream.try_next().await.expect("failed to serve component resolver") {
        match request {
            fpkg::PackageResolverRequest::Resolve { package_url, dir, responder } => {
                let result = if let Err(err) = validate_hermetic_url(
                    &package_url,
                    logger.clone(),
                    &hermetic_test_package_name,
                    &other_allowed_packages,
                )
                .await
                {
                    Err(err)
                } else {
                    let logger = logger.clone();
                    pkg_resolver.resolve(&package_url, dir).await.unwrap_or_else(|err| {
                        let mut builder = log::Record::builder();
                        builder.level(log::Level::Warn);
                        logger.log(
                            &builder
                                .args(format_args!(
                                    "failed to resolve pkg {}: {:?}",
                                    package_url, err
                                ))
                                .build(),
                        );
                        Err(fpkg::ResolveError::Internal)
                    })
                };
                let result_ref = result.as_ref();
                let result_ref = result_ref.map_err(|e| e.to_owned());
                if let Err(e) = responder.send(result_ref) {
                    warn!("Failed sending load response for {}: {}", package_url, e);
                }
            }
            fpkg::PackageResolverRequest::ResolveWithContext {
                package_url,
                context,
                dir,
                responder,
            } => {
                // We don't need to worry about validating context because it should have
                // been produced by Resolve call above.
                let result = if let Err(err) = validate_hermetic_url(
                    &package_url,
                    logger.clone(),
                    &hermetic_test_package_name,
                    &other_allowed_packages,
                )
                .await
                {
                    Err(err)
                } else {
                    let logger = logger.clone();
                    pkg_resolver
                        .resolve_with_context(&package_url, &context, dir)
                        .await
                        .unwrap_or_else(|err| {
                            let mut builder = log::Record::builder();
                            builder.level(log::Level::Warn);
                            logger.log(
                                &builder
                                    .args(format_args!(
                                        "failed to resolve pkg {} with context {:?}: {:?}",
                                        package_url, context, err
                                    ))
                                    .build(),
                            );
                            Err(fpkg::ResolveError::Internal)
                        })
                };
                let result_ref = result.as_ref();
                let result_ref = result_ref.map_err(|e| e.to_owned());
                if let Err(e) = responder.send(result_ref) {
                    warn!("Failed sending load response for {}: {}", package_url, e);
                }
            }
            fpkg::PackageResolverRequest::GetHash { package_url, responder } => {
                let result = if let Err(_err) = validate_hermetic_url(
                    package_url.url.as_str(),
                    logger.clone(),
                    &hermetic_test_package_name,
                    &other_allowed_packages,
                )
                .await
                {
                    Err(zx::Status::INTERNAL.into_raw())
                } else {
                    let logger = logger.clone();
                    pkg_resolver.get_hash(&package_url).await.unwrap_or_else(|err| {
                        let mut builder = log::Record::builder();
                        builder.level(log::Level::Warn);
                        logger.log(
                            &builder
                                .args(format_args!(
                                    "failed to resolve pkg {}: {:?}",
                                    package_url.url.as_str(),
                                    err
                                ))
                                .build(),
                        );
                        Err(zx::Status::INTERNAL.into_raw())
                    })
                };
                let result_ref = result.as_ref();
                let result_ref = result_ref.map_err(|e| e.to_owned());
                if let Err(e) = responder.send(result_ref) {
                    warn!("Failed sending load response for {}: {}", package_url.url.as_str(), e);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct OptionLogger(Option<Publisher>);

impl OptionLogger {
    pub fn log(&self, record: &log::Record<'_>) {
        if let Some(logger) = self.0.as_ref() {
            logger.log(record);
        }
    }
}

pub async fn serve_hermetic_resolver(
    handles: LocalComponentHandles,
    hermetic_test_package_name: Arc<String>,
    other_allowed_packages: AllowedPackages,
    full_resolver: Arc<fresolution::ResolverProxy>,
    pkg_resolver: Arc<fpkg::PackageResolverProxy>,
) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    let mut resolver_tasks = vec![];
    let mut pkg_resolver_tasks = vec![];
    let log_client = handles.connect_to_named_protocol(flogger::LogSinkMarker::PROTOCOL_NAME)?;
    let tags = ["test_resolver"];
    let log_publisher = match flog::Publisher::new_async(
        flog::PublisherOptions::default().tags(&tags).use_log_sink(log_client),
    )
    .await
    {
        Ok(publisher) => OptionLogger(Some(publisher)),
        Err(e) => {
            warn!("Error creating log publisher for resolver: {:?}", e);
            OptionLogger(None)
        }
    };

    let resolver_hermetic_test_package_name = hermetic_test_package_name.clone();
    let resolver_other_allowed_packages = other_allowed_packages.clone();
    let resolver_log_publisher = log_publisher.clone();

    let pkg_resolver_hermetic_test_package_name = hermetic_test_package_name.clone();
    let pkg_resolver_other_allowed_packages = other_allowed_packages.clone();
    let pkg_resolver_log_publisher = log_publisher.clone();

    fs.dir("svc").add_fidl_service(move |stream: fresolution::ResolverRequestStream| {
        let full_resolver = full_resolver.clone();
        let hermetic_test_package_name = resolver_hermetic_test_package_name.clone();
        let other_allowed_packages = resolver_other_allowed_packages.clone();
        let log_publisher = resolver_log_publisher.clone();
        resolver_tasks.push(fasync::Task::local(async move {
            serve_resolver(
                stream,
                log_publisher,
                hermetic_test_package_name,
                other_allowed_packages,
                full_resolver,
            )
            .await;
        }));
    });
    fs.dir("svc").add_fidl_service(move |stream: fpkg::PackageResolverRequestStream| {
        let pkg_resolver = pkg_resolver.clone();
        let hermetic_test_package_name = pkg_resolver_hermetic_test_package_name.clone();
        let other_allowed_packages = pkg_resolver_other_allowed_packages.clone();
        let log_publisher = pkg_resolver_log_publisher.clone();
        pkg_resolver_tasks.push(fasync::Task::local(async move {
            serve_pkg_resolver(
                stream,
                log_publisher,
                hermetic_test_package_name,
                other_allowed_packages,
                pkg_resolver,
            )
            .await;
        }));
    });
    fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use maplit::hashset;

    async fn respond_to_resolve_requests(mut stream: fresolution::ResolverRequestStream) {
        while let Some(request) =
            stream.try_next().await.expect("failed to serve component mock resolver")
        {
            match request {
                fresolution::ResolverRequest::Resolve { component_url, responder } => {
                    match component_url.as_str() {
                        "fuchsia-pkg://fuchsia.com/package-one#meta/comp.cm"
                        | "fuchsia-pkg://fuchsia.com/package-three#meta/comp.cm"
                        | "fuchsia-pkg://fuchsia.com/package-four#meta/comp.cm" => {
                            responder.send(Ok(fresolution::Component::default()))
                        }
                        "fuchsia-pkg://fuchsia.com/package-two#meta/comp.cm" => {
                            responder.send(Err(fresolution::ResolverError::ResourceUnavailable))
                        }
                        _ => responder.send(Err(fresolution::ResolverError::Internal)),
                    }
                    .expect("failed sending response");
                }
                fresolution::ResolverRequest::ResolveWithContext {
                    component_url,
                    context: _,
                    responder,
                } => {
                    match component_url.as_str() {
                        "fuchsia-pkg://fuchsia.com/package-one#meta/comp.cm" | "name#resource" => {
                            responder.send(Ok(fresolution::Component::default()))
                        }
                        _ => responder.send(Err(fresolution::ResolverError::PackageNotFound)),
                    }
                    .expect("failed sending response");
                }
                fresolution::ResolverRequest::_UnknownMethod { .. } => {
                    panic!("Unknown Resolver request");
                }
            }
        }
    }

    // Run hermetic resolver
    fn run_resolver(
        hermetic_test_package_name: Arc<String>,
        other_allowed_packages: AllowedPackages,
        mock_full_resolver: Arc<fresolution::ResolverProxy>,
    ) -> (fasync::Task<()>, fresolution::ResolverProxy) {
        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fresolution::ResolverMarker>();
        let logger = OptionLogger(None);
        let task = fasync::Task::local(async move {
            serve_resolver(
                stream,
                logger,
                hermetic_test_package_name,
                other_allowed_packages,
                mock_full_resolver,
            )
            .await;
        });
        (task, proxy)
    }

    #[fuchsia::test]
    async fn test_successful_resolve() {
        let pkg_name = "package-one".to_string();

        let (resolver_proxy, resolver_request_stream) =
            create_proxy_and_stream::<fresolution::ResolverMarker>();
        let _full_resolver_task = fasync::Task::spawn(async move {
            respond_to_resolve_requests(resolver_request_stream).await;
        });

        let (_task, hermetic_resolver_proxy) = run_resolver(
            pkg_name.into(),
            AllowedPackages::zero_allowed_pkgs(),
            Arc::new(resolver_proxy),
        );

        assert_eq!(
            hermetic_resolver_proxy
                .resolve("fuchsia-pkg://fuchsia.com/package-one#meta/comp.cm")
                .await
                .unwrap(),
            Ok(fresolution::Component::default())
        );
        let mock_context = fresolution::Context { bytes: vec![0] };
        assert_eq!(
            hermetic_resolver_proxy
                .resolve_with_context("name#resource", &mock_context)
                .await
                .unwrap(),
            Ok(fresolution::Component::default())
        );
        assert_eq!(
            hermetic_resolver_proxy
                .resolve_with_context("name#not_found", &mock_context)
                .await
                .unwrap(),
            Err(fresolution::ResolverError::PackageNotFound)
        );
        assert_eq!(
            hermetic_resolver_proxy
                .resolve_with_context(
                    "fuchsia-pkg://fuchsia.com/package-one#meta/comp.cm",
                    &mock_context
                )
                .await
                .unwrap(),
            Ok(fresolution::Component::default())
        );
    }

    #[fuchsia::test]
    async fn drop_connection_on_resolve() {
        let pkg_name = "package-one".to_string();

        let (resolver_proxy, resolver_request_stream) =
            create_proxy_and_stream::<fresolution::ResolverMarker>();
        let _full_resolver_task = fasync::Task::spawn(async move {
            respond_to_resolve_requests(resolver_request_stream).await;
        });

        let (_task, hermetic_resolver_proxy) = run_resolver(
            pkg_name.into(),
            AllowedPackages::zero_allowed_pkgs(),
            Arc::new(resolver_proxy),
        );

        let _ =
            hermetic_resolver_proxy.resolve("fuchsia-pkg://fuchsia.com/package-one#meta/comp.cm");
        drop(hermetic_resolver_proxy); // code should not crash
    }

    #[fuchsia::test]
    async fn test_package_not_allowed() {
        let (resolver_proxy, _) = create_proxy_and_stream::<fresolution::ResolverMarker>();

        let (_task, hermetic_resolver_proxy) = run_resolver(
            "package-two".to_string().into(),
            AllowedPackages::zero_allowed_pkgs(),
            Arc::new(resolver_proxy),
        );

        assert_eq!(
            hermetic_resolver_proxy
                .resolve("fuchsia-pkg://fuchsia.com/package-one#meta/comp.cm")
                .await
                .unwrap(),
            Err(fresolution::ResolverError::PackageNotFound)
        );
        let mock_context = fresolution::Context { bytes: vec![0] };
        assert_eq!(
            hermetic_resolver_proxy
                .resolve_with_context(
                    "fuchsia-pkg://fuchsia.com/package-one#meta/comp.cm",
                    &mock_context
                )
                .await
                .unwrap(),
            Err(fresolution::ResolverError::PackageNotFound)
        );
    }

    #[fuchsia::test]
    async fn other_packages_allowed() {
        let (resolver_proxy, resolver_request_stream) =
            create_proxy_and_stream::<fresolution::ResolverMarker>();

        let list = hashset!("package-three".to_string(), "package-four".to_string());

        let _full_resolver_task = fasync::Task::spawn(async move {
            respond_to_resolve_requests(resolver_request_stream).await;
        });

        let (_task, hermetic_resolver_proxy) = run_resolver(
            "package-two".to_string().into(),
            AllowedPackages::from_iter(list),
            Arc::new(resolver_proxy),
        );

        assert_eq!(
            hermetic_resolver_proxy
                .resolve("fuchsia-pkg://fuchsia.com/package-one#meta/comp.cm")
                .await
                .unwrap(),
            Err(fresolution::ResolverError::PackageNotFound)
        );

        assert_eq!(
            hermetic_resolver_proxy
                .resolve("fuchsia-pkg://fuchsia.com/package-three#meta/comp.cm")
                .await
                .unwrap(),
            Ok(fresolution::Component::default())
        );

        assert_eq!(
            hermetic_resolver_proxy
                .resolve("fuchsia-pkg://fuchsia.com/package-four#meta/comp.cm")
                .await
                .unwrap(),
            Ok(fresolution::Component::default())
        );

        assert_eq!(
            hermetic_resolver_proxy
                .resolve("fuchsia-pkg://fuchsia.com/package-two#meta/comp.cm")
                .await
                .unwrap(),
            // we return this error from our mock resolver for package-two.
            Err(fresolution::ResolverError::ResourceUnavailable)
        );
    }

    #[fuchsia::test]
    async fn test_failed_resolve() {
        let (resolver_proxy, resolver_request_stream) =
            create_proxy_and_stream::<fresolution::ResolverMarker>();
        let _full_resolver_task = fasync::Task::spawn(async move {
            respond_to_resolve_requests(resolver_request_stream).await;
        });

        let pkg_name = "package-two".to_string();
        let (_task, hermetic_resolver_proxy) = run_resolver(
            pkg_name.into(),
            AllowedPackages::zero_allowed_pkgs(),
            Arc::new(resolver_proxy),
        );

        assert_eq!(
            hermetic_resolver_proxy
                .resolve("fuchsia-pkg://fuchsia.com/package-two#meta/comp.cm")
                .await
                .unwrap(),
            Err(fresolution::ResolverError::ResourceUnavailable)
        );
    }

    #[fuchsia::test]
    async fn test_invalid_url() {
        let (resolver_proxy, _) = create_proxy_and_stream::<fresolution::ResolverMarker>();

        let pkg_name = "package-two".to_string();
        let (_task, hermetic_resolver_proxy) = run_resolver(
            pkg_name.into(),
            AllowedPackages::zero_allowed_pkgs(),
            Arc::new(resolver_proxy),
        );

        assert_eq!(
            hermetic_resolver_proxy.resolve("invalid_url").await.unwrap(),
            Err(fresolution::ResolverError::InvalidArgs)
        );
    }
}
