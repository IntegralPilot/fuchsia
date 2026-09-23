// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_hardware_pty::DeviceRequestStream;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use std::cell::RefCell;
use std::rc::Rc;

mod fifo;
#[cfg(test)]
mod integration_tests;
mod ptysvc;

use ptysvc::{Pty, run_server};

#[fuchsia::main(logging = false)]
async fn main() -> Result<(), Error> {
    if let Err(e) = stdout_to_debuglog::init().await {
        eprintln!("ptysvc: failed to init stdout to debuglog: {:?}", e);
        return Err(e);
    }

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(|stream: DeviceRequestStream| stream);
    fs.take_and_serve_directory_handle()?;

    while let Some(stream) = fs.next().await {
        let pty = Rc::new(RefCell::new(Pty::new()));
        fasync::Task::local(async move {
            run_server(pty, stream).await;
        })
        .detach();
    }

    Ok(())
}
