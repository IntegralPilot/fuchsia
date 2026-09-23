// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_pkg::{
    PackageResolverRequest, PackageResolverRequestStream, ResolutionContext, ResolveError,
};
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use tempfile::TempDir;

enum IncomingRequest {
    PkgResolver(PackageResolverRequestStream),
}

#[fuchsia::main]
async fn main() {
    let mut service_fs = ServiceFs::new_local();
    service_fs.dir("svc").add_fidl_service(IncomingRequest::PkgResolver);
    service_fs.take_and_serve_directory_handle().unwrap();

    let temp_dir = TempDir::new().unwrap();
    let temp_dir_path = temp_dir.path();
    let file_path = temp_dir_path.join("foo");
    std::fs::write(&file_path, "hippos").unwrap();
    let temp_dir_path = temp_dir_path.display().to_string();
    let temp_dir = fuchsia_fs::directory::open_in_namespace(
        &temp_dir_path,
        fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
    )
    .unwrap();

    service_fs
        .for_each_concurrent(None, |IncomingRequest::PkgResolver(mut stream)| {
            let temp_dir = Clone::clone(&temp_dir);
            async move {
                while let Some(Ok(request)) = stream.next().await {
                    match request {
                        PackageResolverRequest::Resolve { package_url, dir, responder } => {
                            if package_url == "fuchsia-pkg://fuchsia.com/foo" {
                                // Clone the temp directory on the given channel.
                                let dir = dir.into_channel().into();
                                temp_dir.clone(dir).unwrap();
                                responder.send(Ok(&ResolutionContext { bytes: vec![] })).unwrap();
                            } else {
                                // The package doesn't exist.
                                responder.send(Err(ResolveError::PackageNotFound)).unwrap()
                            }
                        }
                        r => panic!("Unexpected request: {:?}", r),
                    }
                }
            }
        })
        .await;
}
