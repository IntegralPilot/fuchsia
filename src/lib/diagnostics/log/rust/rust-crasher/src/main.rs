// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_diagnostics_crasher::CrasherRequestStream;
use fuchsia_async::Scope;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};

#[fuchsia::main]
async fn main() {
    let mut fs = ServiceFs::new_local();
    let scope = Scope::new();
    fs.dir("svc").add_fidl_service(|r: CrasherRequestStream| r);
    fs.take_and_serve_directory_handle().unwrap();

    while let Some(incoming) = fs.next().await {
        scope.spawn(run_crasher(incoming));
    }
    scope.join().await;
}

async fn run_crasher(mut requests: CrasherRequestStream) {
    let next = requests.try_next().await.unwrap().unwrap();
    match next {
        fidl_fuchsia_diagnostics_crasher::CrasherRequest::Crash { message, responder } => {
            // Let the client close the connection if it wants to.
            let _ = responder.send();
            panic!("{message}");
        }
        fidl_fuchsia_diagnostics_crasher::CrasherRequest::_UnknownMethod { .. } => {
            panic!("Not yet implemented.")
        }
    }
}
