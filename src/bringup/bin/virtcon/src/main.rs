// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use carnelian::{App, AppAssistantPtr};
use fidl_fuchsia_boot::ReadOnlyLogMarker;
use fuchsia_async::LocalExecutor;
use fuchsia_component::client::connect_to_protocol;
use std::convert::TryFrom;
use virtcon_config::Config;
use virtual_console_lib::{VirtualConsoleAppAssistant, VirtualConsoleArgs};

fn main() -> Result<(), Error> {
    fuchsia_trace_provider::trace_provider_create_with_fdio();

    let init = async {
        // Redirect standard out to debuglog.
        stdout_to_debuglog::init().await?;
        Ok::<(), Error>(())
    };
    {
        let mut executor = LocalExecutor::default();
        executor.run_singlethreaded(init)?;
    }

    let config = Config::take_from_startup_handle();

    // Parse config into args
    let args = VirtualConsoleArgs::try_from(config)?;

    // Early out if virtcon should be disabled.
    if args.disable {
        println!("vc: disabled");
        return Ok(());
    }

    println!("vc: started with args {:?}", args);

    let get_read_only_debuglog = async {
        // Connect to read only log service.
        let read_only_log = connect_to_protocol::<ReadOnlyLogMarker>()?;

        // Request debuglog object.
        read_only_log
            .get()
            .await
            .map_err(|e| anyhow!("fidl error when requesting read only log: {:?}", e))
    };

    let read_only_debuglog = {
        let mut executor = LocalExecutor::default();
        executor.run_singlethreaded(get_read_only_debuglog)?
    };

    App::run(Box::new(|app_sender| {
        let f = async move {
            let assistant = Box::new(VirtualConsoleAppAssistant::new(
                app_sender,
                args,
                Some(read_only_debuglog),
            )?);
            Ok::<AppAssistantPtr, Error>(assistant)
        };
        Box::pin(f)
    }))
}
