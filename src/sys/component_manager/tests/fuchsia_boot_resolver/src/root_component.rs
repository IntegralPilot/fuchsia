// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_pkg as fpkg;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt as _, TryStreamExt as _};

#[fuchsia::main]
async fn main() {
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::local(async move {
            run_package_resolution_checker_service(stream).await;
        })
        .detach();
    });
    fs.take_and_serve_directory_handle().expect("failed to serve outgoing directory");
    fs.collect::<()>().await;
}

async fn run_package_resolution_checker_service(
    mut stream: fidl_test_checker::CheckerRequestStream,
) {
    let resolver = fuchsia_component::client::connect_to_protocol_at_path::<
        fpkg::PackageResolverMarker,
    >("/svc/fuchsia.pkg.PackageResolver-boot")
    .unwrap();
    let (proxy, server) = fidl::endpoints::create_proxy();
    let _: fpkg::ResolutionContext =
        resolver.resolve("fuchsia-boot:///root_component_pkg", server).await.unwrap().unwrap();
    let sentinel_contents = fuchsia_fs::directory::read_file_to_string(
        &proxy,
        "data/bootfs_package_resolver_test_sentinel",
    )
    .await
    .unwrap();

    while let Some(req) = stream.try_next().await.unwrap() {
        let fidl_test_checker::CheckerRequest::SentinelFileContents { responder } = req;
        let () = responder.send(&sentinel_contents).unwrap();
    }
}
