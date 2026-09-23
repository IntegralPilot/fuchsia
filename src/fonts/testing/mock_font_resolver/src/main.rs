// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_pkg::{FontResolverRequest, FontResolverRequestStream};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_url::fuchsia_pkg::AbsolutePackageUrl;
use futures::{StreamExt, TryStreamExt};
use log::*;
use vfs::execution_scope::ExecutionScope;
use vfs::file::vmo::read_only;
use vfs::pseudo_directory;
use zx::Status;

#[fuchsia::main(logging_tags = ["mock_font_resolver"])]
async fn main() -> Result<(), Error> {
    info!("Starting mock FontResolver service.");

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::local(async move {
            run_resolver_service(stream).await.expect("Failed to run mock FontResolver.")
        })
        .detach();
    });
    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;
    Ok(())
}

async fn run_resolver_service(mut stream: FontResolverRequestStream) -> Result<(), Error> {
    while let Some(request) = stream.try_next().await? {
        debug!("FontResolver got request {:?}", request);
        let FontResolverRequest::Resolve { package_url, directory_request, responder } = request;
        let response = resolve(package_url, directory_request).await;
        responder.send(response.map_err(|s| s.into_raw()))?;
    }
    Ok(())
}

async fn resolve(
    package_url: String,
    directory_request: ServerEnd<fio::DirectoryMarker>,
) -> Result<(), Status> {
    AbsolutePackageUrl::parse(&package_url).map_err(|_| Status::INVALID_ARGS)?;

    // Serve fake directories with single font files, with the selection depending on the package
    // URL. These correspond to the fake fonts declared in ../tests/*.font_manifest.json.
    let root = match package_url.as_ref() {
        // From ephemeral.font_manifest.json
        "fuchsia-pkg://fuchsia.com/font-package-ephemeral-ttf" => pseudo_directory! {
            "Ephemeral.ttf" => read_only(b"not actually a font"),
        },

        // From aliases.font_manifest.json
        "fuchsia-pkg://fuchsia.com/font-package-alphasans-regular-ttf" => pseudo_directory! {
            "AlphaSans-Regular.ttf" => read_only(b"alpha"),
        },
        "fuchsia-pkg://fuchsia.com/font-package-alphasans-condensed-ttf" => pseudo_directory! {
            "AlphaSans-Condensed.ttf" => read_only(b"alpha"),
        },
        "fuchsia-pkg://fuchsia.com/font-package-alphasanshebrew-regular-ttf" => pseudo_directory! {
            "AlphaSansHebrew-Regular.ttf" => read_only(b"alpha"),
        },
        _ => {
            return Err(Status::NOT_FOUND);
        }
    };

    vfs::directory::serve_on(root, fio::PERM_READABLE, ExecutionScope::new(), directory_request);
    Ok(())
}
