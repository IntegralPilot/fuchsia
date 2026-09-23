// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ingress::fidl::{Interface, display};
use crate::{DisplayConfiguration, EnvironmentBuilder};
use anyhow::{Result, anyhow};
use fidl::endpoints::{DiscoverableProtocolMarker, ServerEnd};
use fidl_fuchsia_settings::{DisplayMarker, IntlMarker};
use fuchsia_inspect::component;
use futures::future::LocalBoxFuture;
use settings_common::config::default_settings::DefaultSetting;
use settings_common::inspect::config_logger::InspectConfigLogger;
use settings_display::build_display_default_settings;
use settings_test_common::storage::InMemoryStorageFactory;
use std::rc::Rc;

const ENV_NAME: &str = "settings_service_display_test_environment";

fn default_settings() -> DefaultSetting<DisplayConfiguration, &'static str> {
    let config_logger =
        Rc::new(std::sync::Mutex::new(InspectConfigLogger::new(component::inspector().root())));
    build_display_default_settings(config_logger)
}

// Makes sure that a failing display stream doesn't cause a failure for a different interface.
#[fuchsia::test(allow_stalls = false)]
async fn test_display_failure() {
    let service_gen =
        |service_name: &str, channel: zx::Channel| -> LocalBoxFuture<'static, Result<()>> {
            match service_name {
                fidl_fuchsia_ui_brightness::ControlMarker::PROTOCOL_NAME => {
                    // This stream is closed immediately
                    let _manager_stream_result =
                        ServerEnd::<fidl_fuchsia_ui_brightness::ControlMarker>::new(channel)
                            .into_stream();

                    Box::pin(async { Ok(()) })
                }
                _ => Box::pin(async { Err(anyhow!("unsupported")) }),
            }
        };

    let env = EnvironmentBuilder::new(Rc::new(InMemoryStorageFactory::new()))
        .service(Box::new(service_gen))
        .fidl_interfaces(&[Interface::Display(display::InterfaceFlags::BASE), Interface::Intl])
        .display_configuration(default_settings())
        .spawn_and_get_protocol_connector(ENV_NAME)
        .await
        .unwrap();

    let display_proxy = env.connect_to_protocol::<DisplayMarker>().expect("connected to service");

    let _settings_value = display_proxy.watch().await.expect("watch completed");

    let intl_service = env.connect_to_protocol::<IntlMarker>().unwrap();
    let _settings = intl_service.watch().await.expect("watch completed");
}
