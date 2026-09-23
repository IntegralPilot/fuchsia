// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod params;
mod ta_loader;
mod trusted_app;

use std::ffi::CString;
use std::fs::File;
use std::io::Read;

use anyhow::Error;
use fidl_fuchsia_tee::{ApplicationRequest, ApplicationRequestStream};
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;

fn read_ta_name() -> Result<CString, Error> {
    let mut f = File::open("/pkg/data/ta_name")
        .map_err(|e| anyhow::anyhow!("Could not open ta_name file: {e}"))?;
    let mut s = String::new();
    let _ = f
        .read_to_string(&mut s)
        .map_err(|e| anyhow::anyhow!("Could not read ta_name file: {e}"))?;
    s.push_str(".so");
    CString::new(s).map_err(|_| anyhow::anyhow!("nul found in name string"))
}

async fn run_application(mut stream: ApplicationRequestStream) -> Result<(), Error> {
    let name = read_ta_name()?;
    let interface = ta_loader::load_ta(&name)?;

    let mut ta = trusted_app::TrustedApp::new(interface)?;

    while let Some(request) = stream.next().await {
        match request {
            Ok(ApplicationRequest::OpenSession2 { parameter_set, responder }) => {
                let (session_id, op_result) = ta.open_session(parameter_set)?;
                responder.send(session_id, op_result)?;
            }
            Ok(ApplicationRequest::CloseSession { session_id, responder }) => {
                ta.close_session(session_id)?;
                responder.send()?;
            }
            Ok(ApplicationRequest::InvokeCommand {
                session_id,
                command_id,
                parameter_set,
                responder,
            }) => {
                let op_result = ta.invoke_command(session_id, command_id, parameter_set)?;
                responder.send(op_result)?;
            }
            Err(e) => {
                // TODO(https://fxbug.dev/332956721): If we get an unexpected message from the client
                // we're supposed to tell the TA to close any open sessions and then destroy it.
                // Same goes for errors in the handlers above.
                log::warn!("Unexpected request: {e}");
                break;
            }
        }
    }
    ta.destroy();
    Ok(())
}

enum IncomingService {
    Application(ApplicationRequestStream),
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new_local();
    let _ = fs.dir("svc").add_fidl_service(IncomingService::Application);

    let _ = fs.take_and_serve_directory_handle()?;

    fs.for_each(|IncomingService::Application(stream)| async {
        run_application(stream).await.unwrap()
    })
    .await;

    // Generate link-time references to the symbols that we want to expose to
    // the TA so that the definitions will be retained by the linker.
    let _ = std::hint::black_box(api_impl::binding_stubs::exposed_c_entry_points());

    Ok(())
}
