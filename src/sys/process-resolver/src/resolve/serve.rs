// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::resolve::get_binary_and_loader_from_pkg_dir;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_pkg as fpkg;
use fidl_fuchsia_process::{ResolverRequest, ResolverRequestStream};
use fuchsia_component::client::connect_to_protocol_at_path;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use futures::prelude::*;
use log::warn;

pub async fn serve(mut stream: ResolverRequestStream) {
    let boot_resolver = connect_to_protocol_at_path::<fpkg::PackageResolverMarker>(
        "/svc/fuchsia.pkg.PackageResolver-boot",
    )
    .expect("Could not connect to fuchsia.pkg.PackageResolver");
    let pkg_resolver = connect_to_protocol_at_path::<fpkg::PackageResolverMarker>(
        "/svc/fuchsia.pkg.PackageResolver-pkg",
    )
    .expect("Could not connect to fuchsia.pkg.PackageResolver");

    while let Some(Ok(request)) = stream.next().await {
        match request {
            ResolverRequest::Resolve { name, responder } => {
                match resolve(&boot_resolver, &pkg_resolver, &name).await {
                    Ok((vmo, ldsvc)) => {
                        let _ = responder.send(zx::sys::ZX_OK, Some(vmo), ldsvc);
                    }
                    Err(s) => {
                        let _ = responder.send(s.into_raw(), None, None);
                    }
                }
            }
        }
    }
}

async fn resolve(
    boot_resolver: &fpkg::PackageResolverProxy,
    pkg_resolver: &fpkg::PackageResolverProxy,
    url: &str,
) -> Result<
    (fidl::Vmo, Option<fidl::endpoints::ClientEnd<fidl_fuchsia_ldsvc::LoaderMarker>>),
    zx::Status,
> {
    // Parse the URL
    let boot_url = fuchsia_url::boot::AbsoluteComponentUrl::parse(url);
    let pkg_url = AbsoluteComponentUrl::parse(url);
    let (resolver, pkg_url, bin_path) = match (boot_url, pkg_url) {
        (Ok(url), _) => {
            // Break it into a package URL and binary path
            let pkg_url = url.to_package_url().to_string();
            let bin_path = url.resource().to_string();
            (boot_resolver, pkg_url, bin_path)
        }
        (_, Ok(url)) => {
            // Break it into a package URL and binary path
            let pkg_url = url.package_url().to_string();
            let bin_path = url.resource().to_string();
            (pkg_resolver, pkg_url, bin_path)
        }
        (_, _) => {
            // There is a CTS test that attempts to resolve `fuchsia-pkg://example.com/mustnotexist`.
            // and expects to see INTERNAL, instead of the more suitable INVALID_ARGS.
            return Err(zx::Status::INTERNAL);
        }
    };

    // Resolve the package URL
    let (pkg_dir, server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    let _subpackage_context =
        resolver.resolve(&pkg_url, server).await.map_err(|_| zx::Status::INTERNAL)?.map_err(
            |e| {
                if e == fpkg::ResolveError::PackageNotFound {
                    zx::Status::NOT_FOUND
                } else {
                    warn!("Could not resolve {}: {:?}", pkg_url, e);
                    zx::Status::INTERNAL
                }
            },
        )?;

    get_binary_and_loader_from_pkg_dir(&pkg_dir, &bin_path, &pkg_url).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async as fasync;

    fn serve_pkg_resolver_success() -> fpkg::PackageResolverProxy {
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fpkg::PackageResolverMarker>();
        fasync::Task::spawn(async move {
            let mut stream = server_end.into_stream();
            let (package_url, dir, responder) =
                if let fpkg::PackageResolverRequest::Resolve { package_url, dir, responder } =
                    stream.next().await.unwrap().unwrap()
                {
                    (package_url, dir, responder)
                } else {
                    panic!("Unexpected call to PackageResolver");
                };
            assert_eq!(package_url, "fuchsia-pkg://fuchsia.com/foo");

            // Return the test's own pkg directory. This is guaranteed to support
            // the readable + executable rights needed by this test.
            fuchsia_fs::directory::open_channel_in_namespace(
                "/pkg",
                fio::PERM_READABLE | fio::PERM_EXECUTABLE,
                dir,
            )
            .unwrap();

            responder.send(Ok(&fpkg::ResolutionContext { bytes: vec![] })).unwrap();
        })
        .detach();
        proxy
    }

    fn serve_boot_pkg_resolver_success() -> fpkg::PackageResolverProxy {
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fpkg::PackageResolverMarker>();
        fasync::Task::spawn(async move {
            let mut stream = server_end.into_stream();
            let (package_url, dir, responder) =
                if let fpkg::PackageResolverRequest::Resolve { package_url, dir, responder } =
                    stream.next().await.unwrap().unwrap()
                {
                    (package_url, dir, responder)
                } else {
                    panic!("Unexpected call to PackageResolver");
                };
            assert_eq!(package_url, "fuchsia-boot:///foo");

            // Return the test's own pkg directory. This is guaranteed to support
            // the readable + executable rights needed by this test.
            fuchsia_fs::directory::open_channel_in_namespace(
                "/pkg",
                fio::PERM_READABLE | fio::PERM_EXECUTABLE,
                dir,
            )
            .unwrap();

            responder.send(Ok(&fpkg::ResolutionContext { bytes: vec![] })).unwrap();
        })
        .detach();
        proxy
    }

    fn serve_pkg_resolver_fail(error: fpkg::ResolveError) -> fpkg::PackageResolverProxy {
        let (proxy, server_end) = fidl::endpoints::create_proxy::<fpkg::PackageResolverMarker>();
        fasync::Task::spawn(async move {
            let mut stream = server_end.into_stream();
            let (package_url, responder) =
                if let fpkg::PackageResolverRequest::Resolve { package_url, responder, .. } =
                    stream.next().await.unwrap().unwrap()
                {
                    (package_url, responder)
                } else {
                    panic!("Unexpected call to PackageResolver");
                };
            assert_eq!(package_url, "fuchsia-pkg://fuchsia.com/foo");
            responder.send(Err(error)).unwrap();
        })
        .detach();
        proxy
    }

    #[fuchsia::test]
    async fn success_resolve() {
        let pkg_resolver = serve_pkg_resolver_success();
        let (vmo, _) = resolve(
            &pkg_resolver,
            &pkg_resolver,
            "fuchsia-pkg://fuchsia.com/foo#bin/process_resolver_bin_test",
        )
        .await
        .unwrap();

        // We don't know much about this test binary. Make a basic assertion.
        assert!(vmo.get_content_size().unwrap() > 0);

        // We can't make any assumptions about the libraries, especially in variations like ASAN.
    }

    #[fuchsia::test]
    async fn success_bootfs_resolve() {
        let pkg_resolver = serve_pkg_resolver_success();
        let boot_pkg_resolver = serve_boot_pkg_resolver_success();
        let (vmo, _) = resolve(
            &boot_pkg_resolver,
            &pkg_resolver,
            "fuchsia-boot:///foo#bin/process_resolver_bin_test",
        )
        .await
        .unwrap();

        // We don't know much about this test binary. Make a basic assertion.
        assert!(vmo.get_content_size().unwrap() > 0);

        // We can't make any assumptions about the libraries, especially in variations like ASAN.
    }

    #[fuchsia::test]
    async fn malformed_url() {
        let pkg_resolver = serve_pkg_resolver_success();
        let boot_pkg_resolver = serve_boot_pkg_resolver_success();
        let status = resolve(&boot_pkg_resolver, &pkg_resolver, "fuchsia-pkg://fuchsia.com/foo")
            .await
            .unwrap_err();
        assert_eq!(status, zx::Status::INTERNAL);
        let status = resolve(&pkg_resolver, &pkg_resolver, "abcd").await.unwrap_err();
        assert_eq!(status, zx::Status::INTERNAL);
    }

    #[fuchsia::test]
    async fn package_resolver_not_found() {
        let pkg_resolver = serve_pkg_resolver_fail(fpkg::ResolveError::PackageNotFound);
        let status = resolve(&pkg_resolver, &pkg_resolver, "fuchsia-pkg://fuchsia.com/foo#bin/bar")
            .await
            .unwrap_err();
        assert_eq!(status, zx::Status::NOT_FOUND);
    }

    #[fuchsia::test]
    async fn package_resolver_internal() {
        let pkg_resolver = serve_pkg_resolver_fail(fpkg::ResolveError::NoSpace);
        let status = resolve(&pkg_resolver, &pkg_resolver, "fuchsia-pkg://fuchsia.com/foo#bin/bar")
            .await
            .unwrap_err();
        assert_eq!(status, zx::Status::INTERNAL);
    }

    #[fuchsia::test]
    async fn binary_not_found() {
        let pkg_resolver = serve_pkg_resolver_success();
        let boot_pkg_resolver = serve_boot_pkg_resolver_success();
        let status = resolve(
            &boot_pkg_resolver,
            &pkg_resolver,
            "fuchsia-pkg://fuchsia.com/foo#bin/does_not_exist",
        )
        .await
        .unwrap_err();
        assert_eq!(status, zx::Status::NOT_FOUND);
    }
}
