// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://github.com/rust-lang/rust-clippy/issues/13885) Upstream issue
// Clippy lints on the license comment above.
#![allow(clippy::literal_string_with_formatting_args)]

use std::collections::HashSet;
use std::rc::Rc;

#[cfg(test)]
use anyhow::format_err;
use anyhow::{Context, Error};
use fidl_fuchsia_io::DirectoryProxy;
use fidl_fuchsia_settings::{
    AccessibilityRequestStream, AudioRequestStream, DisplayRequestStream,
    DoNotDisturbRequestStream, FactoryResetRequestStream, InputRequestStream, IntlRequestStream,
    KeyboardRequestStream, LightRequestStream, NightModeRequestStream, PrivacyRequestStream,
    SetupRequestStream,
};
use fidl_fuchsia_stash::StoreProxy;
use fuchsia_component::client::connect_to_protocol;
#[cfg(test)]
use fuchsia_component::server::ProtocolConnector;
use fuchsia_component::server::{ServiceFs, ServiceFsDir, ServiceObjLocal};
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::{StreamExt, TryStreamExt};
#[cfg(test)]
use log as _;
use serde::Deserialize;
use settings_accessibility::accessibility_controller::AccessibilityController;
use settings_audio::AudioInfoLoader;
use settings_audio::audio_controller::{AudioController, Request as AudioRequest};
use settings_audio::types::AudioInfo;
use settings_common::config::default_settings::DefaultSetting;
use settings_common::config::{AgentType, ControllerFlag};
use settings_common::inspect::event::{
    ExternalEventPublisher, SettingValuePublisher, UsageEvent, UsagePublisher,
};
use settings_common::inspect::listener_logger::ListenerInspectLogger;
use settings_common::service_context::{ExternalServiceEvent, GenerateService, ServiceContext};
use settings_display::display_controller::{
    DisplayController, DisplayInfoLoader, ExternalBrightnessControl,
};
use settings_do_not_disturb::do_not_disturb_controller::DoNotDisturbController;
use settings_factory_reset::factory_reset_controller::FactoryResetController;
use settings_input::input_controller::InputController;
use settings_intl::intl_controller::IntlController;
use settings_keyboard::keyboard_controller::KeyboardController;
use settings_light::light_controller::LightController;
use settings_night_mode::night_mode_controller::NightModeController;
use settings_privacy::privacy_controller::PrivacyController;
use settings_setup::setup_controller::SetupController;
use settings_storage::device_storage::DeviceStorage;
use settings_storage::fidl_storage::FidlStorage;
use settings_storage::storage_factory::{FidlStorageFactory, StorageFactory};
use {fidl_fuchsia_update_verify as fupdate, fuchsia_async as fasync};

pub use settings_display::display_configuration::DisplayConfiguration;
pub use settings_input::input_device_configuration::InputConfiguration;
pub use settings_light::light_hardware_configuration::LightHardwareConfiguration;

use crate::base::SettingType;
use crate::ingress::fidl;

mod clock;
mod storage_migrations;

pub mod agent;
pub mod base;
pub mod ingress;
pub(crate) mod migration;

/// A common trigger for exiting.
pub type ExitSender = futures::channel::mpsc::UnboundedSender<()>;

/// Runtime defines where the environment will exist. Service is meant for
/// production environments and will hydrate components to be discoverable as
/// an environment service. Nested creates a service only usable in the scope
/// of a test.
#[derive(PartialEq)]
enum Runtime {
    Service,
    #[cfg(test)]
    Nested(&'static str),
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct AgentConfiguration {
    pub agent_types: HashSet<AgentType>,
}

#[derive(PartialEq, Debug, Clone, Deserialize)]
pub struct EnabledInterfacesConfiguration {
    pub interfaces: HashSet<fidl::InterfaceSpec>,
}

impl EnabledInterfacesConfiguration {
    pub fn with_interfaces(interfaces: HashSet<fidl::InterfaceSpec>) -> Self {
        Self { interfaces }
    }
}

#[derive(Default, Debug, Clone, Deserialize)]
pub struct ServiceFlags {
    pub controller_flags: HashSet<ControllerFlag>,
}

#[derive(PartialEq, Debug, Default, Clone)]
pub struct ServiceConfiguration {
    agent_types: HashSet<AgentType>,
    fidl_interfaces: HashSet<fidl::Interface>,
    controller_flags: HashSet<ControllerFlag>,
}

impl ServiceConfiguration {
    pub fn from(
        agent_types: AgentConfiguration,
        interfaces: EnabledInterfacesConfiguration,
        flags: ServiceFlags,
    ) -> Self {
        let fidl_interfaces: HashSet<fidl::Interface> =
            interfaces.interfaces.into_iter().map(|x| x.into()).collect();

        Self {
            agent_types: agent_types.agent_types,
            fidl_interfaces,
            controller_flags: flags.controller_flags,
        }
    }

    fn set_fidl_interfaces(&mut self, interfaces: HashSet<fidl::Interface>) {
        self.fidl_interfaces = interfaces;
    }
}

/// Environment is handed back when an environment is spawned from the
/// EnvironmentBuilder. A nested environment (if available) is returned,
/// along with a receiver to be notified when initialization/setup is
/// complete.
#[cfg(test)]
pub struct Environment {
    pub connector: Option<ProtocolConnector>,
    pub settings: HashSet<SettingType>,
}

#[cfg(test)]
impl Environment {
    pub fn new(
        connector: Option<ProtocolConnector>,
        settings: HashSet<SettingType>,
    ) -> Environment {
        Environment { connector, settings }
    }
}

#[cfg(test)]
fn init_storage_dir() -> DirectoryProxy {
    let tempdir = tempfile::tempdir().expect("failed to create tempdir");
    fuchsia_fs::directory::open_in_namespace(
        tempdir.path().to_str().expect("tempdir path is not valid UTF-8"),
        fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
    )
    .expect("failed to open connection to tempdir")
}

#[cfg(not(test))]
fn init_storage_dir() -> DirectoryProxy {
    panic!("migration dir must be specified");
}

/// The [EnvironmentBuilder] aggregates the parameters surrounding an [environment](Environment) and
/// ultimately spawns an environment based on them.
pub struct EnvironmentBuilder<T: StorageFactory<Storage = DeviceStorage>> {
    configuration: Option<ServiceConfiguration>,
    storage_factory: Rc<T>,
    generate_service: Option<GenerateService>,
    active_listener_inspect_logger: Option<Rc<ListenerInspectLogger>>,
    storage_dir: Option<DirectoryProxy>,
    store_proxy: Option<StoreProxy>,
    display_configuration: Option<DefaultSetting<DisplayConfiguration, &'static str>>,
    audio_configuration: Option<DefaultSetting<AudioInfo, &'static str>>,
    input_configuration: Option<DefaultSetting<InputConfiguration, &'static str>>,
    light_configuration: Option<DefaultSetting<LightHardwareConfiguration, &'static str>>,
}

impl<T: StorageFactory<Storage = DeviceStorage> + 'static> EnvironmentBuilder<T> {
    /// Construct a new [EnvironmentBuilder] using `storage_factory` to construct the storage for
    /// the future [Environment].
    pub fn new(storage_factory: Rc<T>) -> Self {
        EnvironmentBuilder {
            configuration: None,
            storage_factory,
            generate_service: None,
            active_listener_inspect_logger: None,
            storage_dir: None,
            store_proxy: None,
            display_configuration: None,
            audio_configuration: None,
            input_configuration: None,
            light_configuration: None,
        }
    }

    /// A service generator to be used as an overlay on the ServiceContext.
    pub fn service(mut self, generate_service: GenerateService) -> Self {
        self.generate_service = Some(generate_service);
        self
    }

    /// A preset configuration to load preset parameters as a base. Note that this will override
    /// any configuration modifications made by [EnvironmentBuilder::fidl_interface],
    /// [EnvironmentBuilder::policies], and [EnvironmentBuilder::flags].
    pub fn configuration(mut self, configuration: ServiceConfiguration) -> Self {
        self.configuration = Some(configuration);
        self
    }

    pub fn display_configuration(
        mut self,
        display_configuration: DefaultSetting<DisplayConfiguration, &'static str>,
    ) -> Self {
        self.display_configuration = Some(display_configuration);
        self
    }

    pub fn audio_configuration(
        mut self,
        audio_configuration: DefaultSetting<AudioInfo, &'static str>,
    ) -> Self {
        self.audio_configuration = Some(audio_configuration);
        self
    }

    pub fn input_configuration(
        mut self,
        input_configuration: DefaultSetting<InputConfiguration, &'static str>,
    ) -> Self {
        self.input_configuration = Some(input_configuration);
        self
    }

    pub fn light_configuration(
        mut self,
        light_configuration: DefaultSetting<LightHardwareConfiguration, &'static str>,
    ) -> Self {
        self.light_configuration = Some(light_configuration);
        self
    }

    /// Will override all fidl interfaces in the [ServiceConfiguration].
    pub fn fidl_interfaces(mut self, interfaces: &[fidl::Interface]) -> Self {
        if self.configuration.is_none() {
            self.configuration = Some(ServiceConfiguration::default());
        }

        if let Some(c) = self.configuration.as_mut() {
            c.set_fidl_interfaces(interfaces.iter().copied().collect());
        }

        self
    }

    /// Sets the inspect node for setting proxy inspect information and any required
    /// inspect loggers.
    pub fn listener_inspect_logger(
        mut self,
        active_listener_inspect_logger: Rc<ListenerInspectLogger>,
    ) -> Self {
        self.active_listener_inspect_logger = Some(active_listener_inspect_logger);
        self
    }

    pub fn storage_dir(mut self, storage_dir: DirectoryProxy) -> Self {
        self.storage_dir = Some(storage_dir);
        self
    }

    pub fn store_proxy(mut self, store_proxy: StoreProxy) -> Self {
        self.store_proxy = Some(store_proxy);
        self
    }

    /// Prepares an environment so that it may be spawned. This ensures that all necessary
    /// components are spawned and ready to handle events and FIDL requests.
    async fn prepare_env(
        self,
        mut fs: ServiceFs<ServiceObjLocal<'_, ()>>,
        runtime: Runtime,
    ) -> Result<(ServiceFs<ServiceObjLocal<'_, ()>>, HashSet<SettingType>), Error> {
        let mut service_dir = match runtime {
            Runtime::Service => fs.dir("svc"),
            #[cfg(test)]
            Runtime::Nested(_) => fs.root_dir(),
        };

        let _ = service_dir.add_fidl_service(
            move |mut stream: fupdate::ComponentOtaHealthCheckRequestStream| {
                fasync::Task::local(async move {
                    while let Some(fupdate::ComponentOtaHealthCheckRequest::GetHealthStatus {
                        responder,
                    }) = stream.try_next().await.expect("error running health check service")
                    {
                        // We always respond healthy because the health check can only be served
                        // if the environment is able to spawn which in turn guarantees that no agents
                        // have returned an error.
                        responder
                            .send(fupdate::HealthStatus::Healthy)
                            .expect("failed to send health status");
                    }
                })
                .detach();
            },
        );

        let (agent_types, fidl_interfaces, flags) = match self.configuration {
            Some(configuration) => (
                configuration.agent_types,
                configuration.fidl_interfaces,
                configuration.controller_flags,
            ),
            _ => (HashSet::new(), HashSet::new(), HashSet::new()),
        };

        let settings: HashSet<_> = fidl_interfaces.into_iter().map(SettingType::from).collect();
        let fidl_storage_factory = Rc::new(
            Self::initialize_fidl_storage_factory(&settings, self.storage_dir, self.store_proxy)
                .await
                .context("initializing fidl storage factory")?,
        );
        let service_context = Rc::new(ServiceContext::new(self.generate_service));
        let audio_info_loader = self.audio_configuration.map(AudioInfoLoader::new);
        Self::initialize_storage(
            &settings,
            &fidl_storage_factory,
            &*self.storage_factory,
            audio_info_loader.clone(),
            self.display_configuration.map(DisplayInfoLoader::new),
        )
        .await;

        let (external_event_tx, external_event_rx) = mpsc::unbounded();
        let external_publisher = ExternalEventPublisher::new(external_event_tx);

        let listener_logger = self
            .active_listener_inspect_logger
            .unwrap_or_else(|| Rc::new(ListenerInspectLogger::new()));

        let RegistrationResult {
            camera_watcher_event_txs,
            media_buttons_event_txs,
            setting_value_rx,
            usage_event_rx,
            audio_request_tx,
            tasks,
        } = Self::register_controllers(
            &settings,
            Rc::clone(&service_context),
            fidl_storage_factory,
            self.storage_factory,
            &flags,
            audio_info_loader,
            self.input_configuration,
            self.light_configuration,
            &mut service_dir,
            Rc::clone(&listener_logger),
            external_publisher.clone(),
        )
        .await;
        for task in tasks {
            task.detach();
        }

        let agent_result = create_agents(
            &settings,
            agent_types,
            camera_watcher_event_txs,
            media_buttons_event_txs,
            setting_value_rx,
            external_event_rx,
            external_publisher,
            usage_event_rx,
            audio_request_tx,
        );

        run_agents(agent_result, service_context).await;

        Ok((fs, settings))
    }

    /// Spawn an [Environment] on the supplied [fasync::LocalExecutor] so that it may process
    /// incoming FIDL requests.
    pub fn spawn(
        self,
        mut executor: fasync::LocalExecutor,
        fs: ServiceFs<ServiceObjLocal<'_, ()>>,
    ) -> Result<(), Error> {
        let (mut fs, ..) = executor
            .run_singlethreaded(self.prepare_env(fs, Runtime::Service))
            .context("Failed to prepare env")?;

        let _ = fs.take_and_serve_directory_handle().expect("could not service directory handle");
        executor.run_singlethreaded(fs.collect::<()>());
        Ok(())
    }

    /// Spawn a nested [Environment] so that it can be used for tests.
    #[cfg(test)]
    pub async fn spawn_nested(self, env_name: &'static str) -> Result<Environment, Error> {
        let (mut fs, entities) = self
            .prepare_env(ServiceFs::new_local(), Runtime::Nested(env_name))
            .await
            .context("Failed to prepare env")?;
        let connector = Some(fs.create_protocol_connector()?);
        fasync::Task::local(fs.collect()).detach();

        Ok(Environment::new(connector, entities))
    }

    /// Spawns a nested environment and returns the associated
    /// ProtocolConnector. Note that this is a helper function that provides a
    /// shortcut for calling EnvironmentBuilder::name() and
    /// EnvironmentBuilder::spawn().
    #[cfg(test)]
    pub async fn spawn_and_get_protocol_connector(
        self,
        env_name: &'static str,
    ) -> Result<ProtocolConnector, Error> {
        let environment = self.spawn_nested(env_name).await?;

        environment.connector.ok_or_else(|| format_err!("connector not created"))
    }

    async fn initialize_fidl_storage_factory(
        settings: &HashSet<SettingType>,
        storage_dir: Option<DirectoryProxy>,
        store_proxy: Option<StoreProxy>,
    ) -> Result<FidlStorageFactory, Error> {
        let (migration_id, storage_dir) = if let Some(storage_dir) = storage_dir {
            let store_proxy = store_proxy.unwrap_or_else(|| {
                let store_proxy = connect_to_protocol::<fidl_fuchsia_stash::StoreMarker>()
                    .expect("failed to connect to stash");
                store_proxy
                    .identify("setting_service")
                    .expect("should be able to identify to stash");
                store_proxy
            });

            let migration_manager = storage_migrations::register_migrations(
                settings,
                Clone::clone(&storage_dir),
                store_proxy,
            )
            .context("failed to register migrations")?;
            let migration_id = match migration_manager.run_migrations().await {
                Ok(id) => {
                    log::info!("migrated storage to {id:?}");
                    id
                }
                Err((id, e)) => {
                    log::error!("Settings migration failed: {e:?}");
                    id
                }
            };
            let migration_id = migration_id.map(|migration| migration.migration_id);
            (migration_id, storage_dir)
        } else {
            (None, init_storage_dir())
        };
        Ok(FidlStorageFactory::new(migration_id.unwrap_or(0), storage_dir))
    }
}

struct RegistrationResult {
    camera_watcher_event_txs: Vec<UnboundedSender<bool>>,
    media_buttons_event_txs: Vec<UnboundedSender<settings_media_buttons::Event>>,
    setting_value_rx: UnboundedReceiver<(&'static str, String)>,
    usage_event_rx: UnboundedReceiver<UsageEvent>,
    audio_request_tx: Option<UnboundedSender<AudioRequest>>,
    tasks: Vec<fasync::Task<()>>,
}

impl<T: StorageFactory<Storage = DeviceStorage> + 'static> EnvironmentBuilder<T> {
    async fn initialize_storage<D>(
        components: &HashSet<SettingType>,
        fidl_storage_factory: &FidlStorageFactory,
        device_storage_factory: &D,
        audio_info_loader: Option<AudioInfoLoader>,
        display_loader: Option<DisplayInfoLoader>,
    ) where
        D: StorageFactory<Storage = DeviceStorage>,
    {
        if components.contains(&SettingType::Accessibility) {
            device_storage_factory
                .initialize::<AccessibilityController>()
                .await
                .expect("storage should still be initializing");
        }

        if components.contains(&SettingType::Audio) {
            device_storage_factory
                .initialize_with_loader::<AudioController, _>(
                    audio_info_loader.expect("Audio storage requires audio configuration"),
                )
                .await
                .expect("storage should still be initializing");
        }

        if components.contains(&SettingType::Display) {
            device_storage_factory
                .initialize_with_loader::<DisplayController, _>(
                    display_loader.expect("Display storage requires display configuration"),
                )
                .await
                .expect("storage should still be initializing");
        }

        if components.contains(&SettingType::DoNotDisturb) {
            device_storage_factory
                .initialize::<DoNotDisturbController>()
                .await
                .expect("storage should still be initializing");
        }

        if components.contains(&SettingType::FactoryReset) {
            device_storage_factory
                .initialize::<FactoryResetController>()
                .await
                .expect("storage should still be initializing");
        }

        if components.contains(&SettingType::Input) {
            device_storage_factory
                .initialize::<InputController>()
                .await
                .expect("storage should still be initializing");
        }

        if components.contains(&SettingType::Intl) {
            device_storage_factory
                .initialize::<IntlController>()
                .await
                .expect("storage should still be initializing");
        }

        if components.contains(&SettingType::Keyboard) {
            device_storage_factory
                .initialize::<KeyboardController>()
                .await
                .expect("storage should still be initializing");
        }

        if components.contains(&SettingType::Light) {
            fidl_storage_factory
                .initialize::<LightController>()
                .await
                .expect("storage should still be initializing");
        }

        if components.contains(&SettingType::NightMode) {
            device_storage_factory
                .initialize::<NightModeController>()
                .await
                .expect("storage should still be initializing");
        }

        if components.contains(&SettingType::Privacy) {
            device_storage_factory
                .initialize::<PrivacyController>()
                .await
                .expect("storage should still be initializing");
        }

        if components.contains(&SettingType::Setup) {
            device_storage_factory
                .initialize::<SetupController>()
                .await
                .expect("storage should still be initializing");
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn register_controllers<F, D>(
        components: &HashSet<SettingType>,
        service_context: Rc<ServiceContext>,
        fidl_storage_factory: Rc<F>,
        device_storage_factory: Rc<D>,
        controller_flags: &HashSet<ControllerFlag>,
        audio_info_loader: Option<AudioInfoLoader>,
        input_configuration: Option<DefaultSetting<InputConfiguration, &'static str>>,
        light_configuration: Option<DefaultSetting<LightHardwareConfiguration, &'static str>>,
        service_dir: &mut ServiceFsDir<'_, ServiceObjLocal<'_, ()>>,
        listener_logger: Rc<ListenerInspectLogger>,
        external_publisher: ExternalEventPublisher,
    ) -> RegistrationResult
    where
        F: StorageFactory<Storage = FidlStorage>,
        D: StorageFactory<Storage = DeviceStorage>,
    {
        let (setting_value_tx, setting_value_rx) = mpsc::unbounded();
        let (usage_event_tx, usage_event_rx) = mpsc::unbounded();
        let mut camera_watcher_event_txs = vec![];
        let mut media_buttons_event_txs = vec![];
        let mut tasks = vec![];

        // Start handlers for all components.
        if components.contains(&SettingType::Accessibility) {
            let settings_accessibility::SetupResult { mut accessibility_fidl_handler, task } =
                settings_accessibility::setup_accessibility_api(
                    Rc::clone(&device_storage_factory),
                    SettingValuePublisher::new(setting_value_tx.clone()),
                    UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                )
                .await;
            tasks.push(task);
            let _ = service_dir.add_fidl_service(move |stream: AccessibilityRequestStream| {
                accessibility_fidl_handler.handle_stream(stream)
            });
        }

        let audio_request_tx = if components.contains(&SettingType::Audio) {
            let settings_audio::SetupResult {
                mut audio_fidl_handler,
                request_tx: audio_request_tx,
                task,
            } = settings_audio::setup_audio_api(
                Rc::clone(&service_context),
                audio_info_loader.expect("Audio controller requires audio configuration"),
                Rc::clone(&device_storage_factory),
                SettingValuePublisher::new(setting_value_tx.clone()),
                UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                external_publisher.clone(),
            )
            .await;
            tasks.push(task);
            let _ = service_dir.add_fidl_service(move |stream: AudioRequestStream| {
                audio_fidl_handler.handle_stream(stream)
            });
            Some(audio_request_tx)
        } else {
            None
        };

        if components.contains(&SettingType::Display) {
            let result = if controller_flags.contains(&ControllerFlag::ExternalBrightnessControl) {
                settings_display::setup_display_api::<D, ExternalBrightnessControl>(
                    &service_context,
                    Rc::clone(&device_storage_factory),
                    SettingValuePublisher::new(setting_value_tx.clone()),
                    UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                    external_publisher.clone(),
                )
                .await
            } else {
                settings_display::setup_display_api::<D, ()>(
                    &service_context,
                    Rc::clone(&device_storage_factory),
                    SettingValuePublisher::new(setting_value_tx.clone()),
                    UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                    external_publisher.clone(),
                )
                .await
            };
            match result {
                Ok(settings_display::SetupResult { mut display_fidl_handler, task }) => {
                    tasks.push(task);
                    let _ = service_dir.add_fidl_service(move |stream: DisplayRequestStream| {
                        display_fidl_handler.handle_stream(stream)
                    });
                }
                Err(e) => {
                    log::error!("Failed to setup display api: {e:?}");
                }
            }
        }

        if components.contains(&SettingType::DoNotDisturb) {
            let settings_do_not_disturb::SetupResult { mut do_not_disturb_fidl_handler, task } =
                settings_do_not_disturb::setup_do_not_disturb_api(
                    Rc::clone(&device_storage_factory),
                    SettingValuePublisher::new(setting_value_tx.clone()),
                    UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                )
                .await;
            tasks.push(task);
            let _ = service_dir.add_fidl_service(move |stream: DoNotDisturbRequestStream| {
                do_not_disturb_fidl_handler.handle_stream(stream)
            });
        }

        if components.contains(&SettingType::FactoryReset) {
            match settings_factory_reset::setup_factory_reset_api(
                &service_context,
                Rc::clone(&device_storage_factory),
                SettingValuePublisher::new(setting_value_tx.clone()),
                UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                external_publisher.clone(),
            )
            .await
            {
                Ok(settings_factory_reset::SetupResult {
                    mut factory_reset_fidl_handler,
                    task,
                }) => {
                    tasks.push(task);
                    let _ =
                        service_dir.add_fidl_service(move |stream: FactoryResetRequestStream| {
                            factory_reset_fidl_handler.handle_stream(stream)
                        });
                }
                Err(e) => {
                    log::error!("Failed to setup factory reset api: {e:?}");
                }
            }
        }

        if components.contains(&SettingType::Input) {
            let mut input_configuration =
                input_configuration.expect("Input controller requires an input configuration");
            match settings_input::setup_input_api(
                Rc::clone(&service_context),
                &mut input_configuration,
                Rc::clone(&device_storage_factory),
                SettingValuePublisher::new(setting_value_tx.clone()),
                UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                external_publisher.clone(),
            )
            .await
            {
                Ok(settings_input::SetupResult {
                    mut input_fidl_handler,
                    camera_watcher_event_tx,
                    media_buttons_event_tx,
                    task,
                }) => {
                    camera_watcher_event_txs.push(camera_watcher_event_tx);
                    media_buttons_event_txs.push(media_buttons_event_tx);
                    tasks.push(task);
                    let _ = service_dir.add_fidl_service(move |stream: InputRequestStream| {
                        input_fidl_handler.handle_stream(stream)
                    });
                }
                Err(e) => {
                    log::error!("Failed to setup input api: {e:?}");
                }
            }
        }

        if components.contains(&SettingType::Light) {
            let mut light_configuration =
                light_configuration.expect("Light controller requires a light configuration");
            match settings_light::setup_light_api(
                Rc::clone(&service_context),
                &mut light_configuration,
                fidl_storage_factory,
                SettingValuePublisher::new(setting_value_tx.clone()),
                UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                external_publisher.clone(),
            )
            .await
            {
                Ok(settings_light::SetupResult {
                    mut light_fidl_handler,
                    media_buttons_event_tx,
                    task,
                }) => {
                    media_buttons_event_txs.push(media_buttons_event_tx);
                    tasks.push(task);
                    let _ = service_dir.add_fidl_service(move |stream: LightRequestStream| {
                        light_fidl_handler.handle_stream(stream)
                    });
                }
                Err(e) => {
                    log::error!("Failed to setup light api: {e:?}");
                }
            }
        }

        if components.contains(&SettingType::Intl) {
            let settings_intl::SetupResult { mut intl_fidl_handler, task } =
                settings_intl::setup_intl_api(
                    Rc::clone(&device_storage_factory),
                    SettingValuePublisher::new(setting_value_tx.clone()),
                    UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                )
                .await;
            tasks.push(task);
            let _ = service_dir.add_fidl_service(move |stream: IntlRequestStream| {
                intl_fidl_handler.handle_stream(stream)
            });
        }

        if components.contains(&SettingType::Keyboard) {
            let settings_keyboard::SetupResult { mut keyboard_fidl_handler, task } =
                settings_keyboard::setup_keyboard_api(
                    Rc::clone(&device_storage_factory),
                    SettingValuePublisher::new(setting_value_tx.clone()),
                    UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                )
                .await;
            tasks.push(task);
            let _ = service_dir.add_fidl_service(move |stream: KeyboardRequestStream| {
                keyboard_fidl_handler.handle_stream(stream)
            });
        }

        if components.contains(&SettingType::NightMode) {
            let settings_night_mode::SetupResult { mut night_mode_fidl_handler, task } =
                settings_night_mode::setup_night_mode_api(
                    Rc::clone(&device_storage_factory),
                    SettingValuePublisher::new(setting_value_tx.clone()),
                    UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                )
                .await;
            tasks.push(task);
            let _ = service_dir.add_fidl_service(move |stream: NightModeRequestStream| {
                night_mode_fidl_handler.handle_stream(stream)
            });
        }

        if components.contains(&SettingType::Privacy) {
            let settings_privacy::SetupResult { mut privacy_fidl_handler, task } =
                settings_privacy::setup_privacy_api(
                    Rc::clone(&device_storage_factory),
                    SettingValuePublisher::new(setting_value_tx.clone()),
                    UsagePublisher::new(usage_event_tx.clone(), Rc::clone(&listener_logger)),
                )
                .await;
            tasks.push(task);
            let _ = service_dir.add_fidl_service(move |stream: PrivacyRequestStream| {
                privacy_fidl_handler.handle_stream(stream)
            });
        }

        if components.contains(&SettingType::Setup) {
            let settings_setup::SetupResult { mut setup_fidl_handler, task } =
                settings_setup::setup_setup_api(
                    service_context,
                    device_storage_factory,
                    SettingValuePublisher::new(setting_value_tx),
                    UsagePublisher::new(usage_event_tx, listener_logger),
                    external_publisher,
                )
                .await;
            tasks.push(task);
            let _ = service_dir.add_fidl_service(move |stream: SetupRequestStream| {
                setup_fidl_handler.handle_stream(stream)
            });
        }

        RegistrationResult {
            camera_watcher_event_txs,
            media_buttons_event_txs,
            setting_value_rx,
            usage_event_rx,
            audio_request_tx,
            tasks,
        }
    }
}

struct AgentResult {
    earcons_agent: Option<agent::earcons::agent::Agent>,
    camera_watcher_agent: Option<settings_camera_watcher_agent::CameraWatcherAgent>,
    media_buttons_agent: Option<settings_media_buttons_agent::MediaButtonsAgent>,
    inspect_settings_values_agent: Option<agent::inspect::setting_values::AgentSetup>,
    inspect_external_apis_agent: Option<agent::inspect::external_apis::ExternalApiInspectAgent>,
    inspect_setting_proxy_agent: Option<agent::inspect::setting_proxy::SettingProxyInspectAgent>,
    inspect_usages_agent: Option<agent::inspect::usage_counts::SettingTypeUsageInspectAgent>,
}

#[allow(clippy::too_many_arguments)]
fn create_agents(
    settings: &HashSet<SettingType>,
    agent_types: HashSet<AgentType>,
    camera_watcher_event_txs: Vec<UnboundedSender<bool>>,
    media_buttons_event_txs: Vec<UnboundedSender<settings_media_buttons::Event>>,
    setting_value_rx: UnboundedReceiver<(&'static str, String)>,
    external_event_rx: UnboundedReceiver<ExternalServiceEvent>,
    external_publisher: ExternalEventPublisher,
    mut usage_router_rx: UnboundedReceiver<UsageEvent>,
    audio_request_tx: Option<UnboundedSender<AudioRequest>>,
) -> AgentResult {
    let (proxy_event_tx, proxy_event_rx) = mpsc::unbounded();
    let (usage_event_tx, usage_event_rx) = mpsc::unbounded();

    // Route general inspect requests to specific inspect agents.
    fasync::Task::local(async move {
        while let Some(usage_event) = usage_router_rx.next().await {
            let _ = proxy_event_tx.unbounded_send(usage_event.clone());
            let _ = usage_event_tx.unbounded_send(usage_event);
        }
    })
    .detach();
    let earcons_agent = agent_types
        .contains(&AgentType::Earcons)
        .then(|| agent::earcons::agent::Agent::new(audio_request_tx, external_publisher.clone()));
    let camera_watcher_agent = agent_types.contains(&AgentType::CameraWatcher).then(|| {
        settings_camera_watcher_agent::CameraWatcherAgent::new(
            camera_watcher_event_txs,
            external_publisher.clone(),
        )
    });
    let media_buttons_agent = agent_types.contains(&AgentType::MediaButtons).then(|| {
        settings_media_buttons_agent::MediaButtonsAgent::new(
            media_buttons_event_txs,
            external_publisher,
        )
    });
    let inspect_settings_values_agent = agent_types
        .contains(&AgentType::InspectSettingValues)
        .then(|| {
            agent::inspect::setting_values::SettingValuesInspectAgent::new(
                settings.iter().map(|setting| format!("{setting:?}")).collect(),
                setting_value_rx,
            )
        })
        .and_then(|opt| opt);
    let inspect_external_apis_agent = agent_types
        .contains(&AgentType::InspectExternalApis)
        .then(|| agent::inspect::external_apis::ExternalApiInspectAgent::new(external_event_rx));
    let inspect_setting_proxy_agent = agent_types
        .contains(&AgentType::InspectSettingProxy)
        .then(|| agent::inspect::setting_proxy::SettingProxyInspectAgent::new(proxy_event_rx));
    let inspect_usages_agent = agent_types
        .contains(&AgentType::InspectSettingTypeUsage)
        .then(|| agent::inspect::usage_counts::SettingTypeUsageInspectAgent::new(usage_event_rx));

    AgentResult {
        earcons_agent,
        camera_watcher_agent,
        media_buttons_agent,
        inspect_settings_values_agent,
        inspect_external_apis_agent,
        inspect_setting_proxy_agent,
        inspect_usages_agent,
    }
}

async fn run_agents(agent_result: AgentResult, service_context: Rc<ServiceContext>) {
    if let Some(earcons_agent) = agent_result.earcons_agent {
        earcons_agent.initialize(Rc::clone(&service_context)).await;
    }

    if let Some(inspect_settings_values_agent) = agent_result.inspect_settings_values_agent {
        inspect_settings_values_agent.initialize(
            #[cfg(test)]
            None,
        );
    }

    if let Some(inspect_external_apis_agent) = agent_result.inspect_external_apis_agent {
        inspect_external_apis_agent.initialize();
    }

    if let Some(inspect_setting_proxy_agent) = agent_result.inspect_setting_proxy_agent {
        inspect_setting_proxy_agent.initialize();
    }

    if let Some(inspect_usages_agent) = agent_result.inspect_usages_agent {
        inspect_usages_agent.initialize();
    }

    if let Some(camera_watcher_agent) = agent_result.camera_watcher_agent
        && let Err(e) = camera_watcher_agent.spawn(&service_context).await
    {
        log::error!("Failed to spawn camera watcher agent: {e:?}");
    }

    if let Some(media_buttons_agent) = agent_result.media_buttons_agent
        && let Err(e) = media_buttons_agent.spawn(&service_context).await
    {
        log::error!("Failed to spawn media buttons agent: {e:?}");
    }
}

#[cfg(test)]
mod tests;
