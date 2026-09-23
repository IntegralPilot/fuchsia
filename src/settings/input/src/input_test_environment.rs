// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::build_input_default_settings;
use crate::input_device_configuration::InputConfiguration;
use crate::types::InputInfoSources;
use fidl_fuchsia_settings::{InputMarker, InputProxy, InputRequestStream};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component;
use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedSender};
use futures::lock::Mutex;
use settings_common::config::AgentType;
use settings_common::config::default_settings::DefaultSetting;
use settings_common::inspect::config_logger::InspectConfigLogger;
use settings_common::inspect::event::{
    ExternalEventPublisher, SettingValuePublisher, UsagePublisher,
};
use settings_common::inspect::listener_logger::ListenerInspectLogger;
use settings_common::service_context::ServiceContext;
use settings_test_common::fakes::camera3_service::Camera3Service;
use settings_test_common::fakes::input_device_registry_service::InputDeviceRegistryService;
use settings_test_common::fakes::service::ServiceRegistry;
use settings_test_common::storage::InMemoryStorageFactory;
use std::rc::Rc;

pub(crate) struct TestInputEnvironment {
    /// For sending requests to the input proxy.
    pub(crate) input_service: InputProxy,

    /// For sending media buttons changes.
    pub(crate) input_button_service: Rc<Mutex<InputDeviceRegistryService>>,

    /// For watching, connecting to, and making requests on the camera device.
    pub(crate) camera3_service: Rc<Mutex<Camera3Service>>,
}

pub(crate) struct TestInputEnvironmentBuilder {
    /// The initial InputInfoSources in the environment.
    starting_input_info_sources: Option<InputInfoSources>,

    /// The config to load from.
    input_device_config: Option<InputConfiguration>,

    /// The list of agents to include.
    agents: Vec<AgentType>,

    /// The list of additional media_button_event listeners.
    media_buttons_event_txs: Vec<UnboundedSender<settings_media_buttons::Event>>,
}

impl TestInputEnvironmentBuilder {
    pub(crate) fn new() -> Self {
        Self {
            starting_input_info_sources: None,
            input_device_config: None,
            agents: vec![AgentType::MediaButtons],
            media_buttons_event_txs: vec![],
        }
    }

    pub(crate) fn set_starting_input_info_sources(
        mut self,
        starting_input_info_sources: InputInfoSources,
    ) -> Self {
        self.starting_input_info_sources = Some(starting_input_info_sources);
        self
    }

    pub(crate) fn set_input_device_config(
        mut self,
        input_device_config: InputConfiguration,
    ) -> Self {
        self.input_device_config = Some(input_device_config);
        self
    }

    pub(crate) fn add_media_buttons_event_tx(
        mut self,
        tx: UnboundedSender<settings_media_buttons::Event>,
    ) -> Self {
        self.media_buttons_event_txs.push(tx);
        self
    }

    pub(crate) async fn build(mut self) -> TestInputEnvironment {
        let service_registry = ServiceRegistry::create();
        let storage_factory = Rc::new(if let Some(info) = self.starting_input_info_sources {
            InMemoryStorageFactory::with_initial_data(&info)
        } else {
            InMemoryStorageFactory::new()
        });

        // Register fake input device registry service.
        let input_button_service_handle = Rc::new(Mutex::new(InputDeviceRegistryService::new()));
        service_registry.lock().await.register_service(input_button_service_handle.clone());

        // Register fake camera3 service.
        let camera3_service_handle = Rc::new(Mutex::new(Camera3Service::new(true)));
        service_registry.lock().await.register_service(camera3_service_handle.clone());

        storage_factory.initialize_storage::<InputInfoSources>().await;

        let (value_tx, _value_rx) = mpsc::unbounded();
        let (usage_tx, _usage_rx) = mpsc::unbounded();
        let (event_tx, _event_rx) = mpsc::unbounded();
        let setting_value_publisher = SettingValuePublisher::new(value_tx);
        let usage_publisher = UsagePublisher::new(usage_tx, Rc::new(ListenerInspectLogger::new()));
        let external_publisher = ExternalEventPublisher::new(event_tx);

        let service_context =
            Rc::new(ServiceContext::new(Some(ServiceRegistry::serve(service_registry))));

        let crate::SetupResult {
            mut input_fidl_handler,
            camera_watcher_event_tx,
            media_buttons_event_tx,
            task,
        } = crate::setup_input_api(
            Rc::clone(&service_context),
            &mut (self
                .input_device_config
                .map(|config| {
                    DefaultSetting::new(Some(config), "/no/default/file", config_logger())
                })
                .unwrap_or_else(|| default_settings())),
            storage_factory,
            setting_value_publisher,
            usage_publisher,
            external_publisher.clone(),
        )
        .await
        .expect("configured correctly");
        task.detach();

        let camera_watcher_agent = self.agents.contains(&AgentType::CameraWatcher).then(|| {
            settings_camera_watcher_agent::CameraWatcherAgent::new(
                vec![camera_watcher_event_tx],
                external_publisher.clone(),
            )
        });
        let media_buttons_agent = self.agents.contains(&AgentType::MediaButtons).then(|| {
            self.media_buttons_event_txs.extend(Some(media_buttons_event_tx));
            settings_media_buttons_agent::MediaButtonsAgent::new(
                self.media_buttons_event_txs,
                external_publisher,
            )
        });

        if let Some(camera_watcher_agent) = camera_watcher_agent {
            camera_watcher_agent.spawn(&service_context).await.expect("camera watcher agent");
        }

        if let Some(media_buttons_agent) = media_buttons_agent {
            media_buttons_agent.spawn(&service_context).await.expect("media buttons agent");
        }

        let mut fs = ServiceFs::new_local();
        let mut service_dir = fs.root_dir();
        let _ = service_dir.add_fidl_service(move |stream: InputRequestStream| {
            input_fidl_handler.handle_stream(stream);
        });
        let connector = fs.create_protocol_connector().expect("connector not available");
        fasync::Task::local(fs.collect()).detach();

        let input_service = connector.connect_to_protocol::<InputMarker>().unwrap();

        TestInputEnvironment {
            input_service,
            input_button_service: input_button_service_handle,
            camera3_service: camera3_service_handle,
        }
    }
}

fn config_logger() -> Rc<std::sync::Mutex<InspectConfigLogger>> {
    Rc::new(std::sync::Mutex::new(InspectConfigLogger::new(component::inspector().root())))
}

pub(super) fn default_settings() -> DefaultSetting<InputConfiguration, &'static str> {
    build_input_default_settings(config_logger())
}
