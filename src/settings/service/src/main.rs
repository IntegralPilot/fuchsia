// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_stash::StoreMarker;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component;
use futures::lock::Mutex;
use settings::base::get_default_interfaces;
use settings::{
    AgentConfiguration, EnabledInterfacesConfiguration, EnvironmentBuilder, ServiceConfiguration,
    ServiceFlags,
};
use settings_audio::build_audio_default_settings;
use settings_common::config::default_settings::DefaultSetting;
use settings_common::config::get_default_agent_types;
use settings_common::inspect::config_logger::InspectConfigLogger;
use settings_common::inspect::listener_logger::ListenerInspectLogger;
use settings_display::build_display_default_settings;
use settings_input::build_input_default_settings;
use settings_light::build_light_default_settings;
use settings_storage::stash_logger::StashInspectLogger;
use settings_storage::storage_factory::StashDeviceStorageFactory;
use std::path::Path;
use std::rc::Rc;

const STASH_IDENTITY: &str = "settings_service";

#[fuchsia::main(logging_tags = ["setui-service"])]
fn main() -> Result<(), Error> {
    let executor = fasync::LocalExecutor::default();
    log::info!("Starting setui-service...");

    let inspector = component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default());

    // Serve stats about inspect in a lazy node.
    component::serve_inspect_stats();

    let stash_inspect_logger = StashInspectLogger::new(inspector.root());
    let listener_inspect_logger = ListenerInspectLogger::new();

    let default_enabled_interfaces_configuration =
        EnabledInterfacesConfiguration::with_interfaces(get_default_interfaces());

    let config_logger = Rc::new(std::sync::Mutex::new(InspectConfigLogger::new(inspector.root())));

    let input_configuration = build_input_default_settings(Rc::clone(&config_logger));
    let light_configuration = build_light_default_settings(Rc::clone(&config_logger));

    let enabled_interface_configuration = DefaultSetting::new(
        Some(default_enabled_interfaces_configuration),
        "/config/data/interface_configuration.json",
        Rc::clone(&config_logger),
    )
    .load_default_value()
    .expect("invalid default enabled interface configuration")
    .expect("no default enabled interfaces configuration");

    let flags = DefaultSetting::new(
        Some(ServiceFlags::default()),
        "/config/data/service_flags.json",
        Rc::clone(&config_logger),
    )
    .load_default_value()
    .expect("invalid service flag configuration")
    .expect("no default service flags");

    let display_configuration = build_display_default_settings(Rc::clone(&config_logger));
    let audio_configuration = build_audio_default_settings(Rc::clone(&config_logger));

    // Temporary solution for FEMU to have an agent config without camera agent.
    let agent_config = "/config/data/agent_configuration.json";
    let board_agent_config = "/config/data/board_agent_configuration.json";
    let agent_configuration_file_path =
        if Path::new(board_agent_config).exists() { board_agent_config } else { agent_config };

    let agent_types = DefaultSetting::new(
        Some(AgentConfiguration { agent_types: get_default_agent_types() }),
        agent_configuration_file_path,
        config_logger,
    )
    .load_default_value()
    .expect("invalid default agent configuration")
    .expect("no default agent types");

    let configuration =
        ServiceConfiguration::from(agent_types, enabled_interface_configuration, flags);

    let store_proxy = connect_to_protocol::<StoreMarker>().expect("failed to connect to stash");
    store_proxy.identify(STASH_IDENTITY).expect("should identify");
    let storage_factory = StashDeviceStorageFactory::new(
        store_proxy.clone(),
        Rc::new(Mutex::new(stash_inspect_logger)),
    );

    let storage_dir = fuchsia_fs::directory::open_in_namespace(
        "/data/storage",
        fuchsia_fs::PERM_READABLE
            | fuchsia_fs::PERM_WRITABLE
            | fuchsia_fs::Flags::FLAG_MAYBE_CREATE,
    )
    .context("unable to open data dir")?;
    // EnvironmentBuilder::spawn returns a future that can be awaited for the
    // result of the startup. Since main is a synchronous function, we cannot
    // block here and therefore continue without waiting for the result.

    let fs = ServiceFs::new_local();

    EnvironmentBuilder::new(Rc::new(storage_factory))
        .configuration(configuration)
        .display_configuration(display_configuration)
        .audio_configuration(audio_configuration)
        .input_configuration(input_configuration)
        .light_configuration(light_configuration)
        .listener_inspect_logger(Rc::new(listener_inspect_logger))
        .storage_dir(storage_dir)
        .store_proxy(store_proxy)
        .spawn(executor, fs)
        .context("Failed to spawn environment for setui")?;
    // setui_service should never exit. EnvironmentBuilder::spawn should never return because it
    // serves the component out dir to which CM will never close the client connection. Return error
    // to indicate that an assumption has been broken and to trigger the component's
    // `on_terminate: "reboot"` behavior.
    Err(anyhow::anyhow!("setui_service stopped"))
}
