// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_test_configexample::{ConfigUserRequest, ConfigUserRequestStream};
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use power_config_client_config::Config;

// If you're implementing a component that actually uses fuchsia.power.SuspendEnabled, you will
// probably use an if/else block like the one below to modify your internal behavior, based on
// whether or not you are managing power. But in this very simplified example, the only behavior
// for us to change here is the value we report back to the test component.
async fn serve_config_user_protocol(config: &Config, mut stream: ConfigUserRequestStream) {
    let result: anyhow::Result<()> = async move {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                ConfigUserRequest::IsManagingPower { responder } => {
                    if config.should_manage_power {
                        log::info!("Let's pretend that I'm doing something to manage power!");
                        responder.send(true)?;
                    } else {
                        log::info!("I am not managing power, but I wish I were. :/");
                        responder.send(false)?;
                    }
                }
                ConfigUserRequest::_UnknownMethod { .. } => unimplemented!(),
            }
        }

        Ok(())
    }
    .await;

    if let Err(err) = result {
        log::error!("{:?}", err);
    }
}

#[fuchsia::main]
async fn main() -> anyhow::Result<()> {
    log::info!(
        "Reading structured configuration. I will determine whether to do power management based \
        on its `should_manage_power` field."
    );
    let config = Config::take_from_startup_handle();

    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: ConfigUserRequestStream| stream);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, |stream| serve_config_user_protocol(&config, stream)).await;
    Ok(())
}
