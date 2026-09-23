// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fidl_test_compatibility::{ConfigRequest, ConfigRequestStream};
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;

const IMPLS: [&'static str; 5] = ["cpp", "hlcpp", "go", "llcpp", "rust"];

async fn config_server(stream: ConfigRequestStream) {
    stream
        .for_each_concurrent(None, move |request| async move {
            println!("GOT REQUEST: {request:?}!!!!");
            match request {
                Ok(ConfigRequest::GetImpls { responder }) => {
                    // LINT.IfChange
                    let impls = IMPLS
                        .iter()
                        .map(|s| format!("fidl-compatibility-test-{s}#meta/impl.cm"))
                        .collect::<Vec<_>>();
                    // LINT.ThenChange(BUILD.gn)
                    responder.send(&impls).expect("sent response");
                }
                _ => {}
            }
        })
        .await;
}

#[fuchsia::main]
async fn main() {
    println!("LAUNCHED TEST DRIVER !!!!");
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream| stream);
    fs.take_and_serve_directory_handle().expect("took incoming handle");
    fs.for_each_concurrent(None, config_server).await;
    println!("EXITING TEST DRIVER !!!!");
}
