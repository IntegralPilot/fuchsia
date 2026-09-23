// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::conversion::convert_start_args;
use crate::loader::{Library, LoaderService};
use crate::modules::ModulesAndSymbols;
use crate::utils::*;
use fdf::{AutoReleaseDispatcher, OnDispatcher};
use fdf_component::Incoming;
use fidl::endpoints::{ClientEnd, RequestStream, ServerEnd};
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_driver_framework as fidl_fdf;
use fidl_fuchsia_driver_host as fdh;
use fidl_fuchsia_power_broker::{
    DependencyToken, ElementControlMarker, ElementRunnerMarker, ElementRunnerRequest,
    LeaseDependency, LeaseSchema, LessorProxy, TopologyProxy,
};
use fidl_next::ClientDispatcher;
use fidl_next_fuchsia_driver_framework as fidl_next_fdf;
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use futures::{FutureExt, TryStreamExt};
use log::warn;
use namespace::Namespace;
use std::ffi::c_char;
use std::ptr::NonNull;
use std::sync::{Arc, Weak};
use zx::Status;

unsafe extern "C" {
    fn driver_host_find_symbol(
        passive_abi: u64,
        so_name: *const c_char,
        so_name_len: usize,
        symbol_name: *const c_char,
        symbol_name_len: usize,
    ) -> u64;

}

type DriverClient = fidl_next::Client<
    fidl_next_fuchsia_driver_framework::Driver,
    fdf_fidl::DriverChannel<fdf::AsyncDispatcher>,
>;

struct LoadedDriver(u64);

impl LoadedDriver {
    fn get_hooks(&self, module_name: &str) -> Result<Hooks, Status> {
        const SYMBOL: &str = "__fuchsia_driver_registration__";
        let registration_addr = unsafe {
            driver_host_find_symbol(
                self.0,
                module_name.as_ptr() as *const c_char,
                module_name.len(),
                SYMBOL.as_ptr() as *const c_char,
                SYMBOL.len(),
            )
        };
        if registration_addr == 0 {
            return Err(Status::NOT_FOUND);
        }
        Hooks::new(registration_addr as *mut _)
    }

    fn get_symbols(
        &self,
        program: &fdata::Dictionary,
    ) -> Result<Vec<fidl_fdf::NodeSymbol>, Status> {
        let default = Vec::new();
        let modules = get_program_objvec(program, "modules")?.unwrap_or(&default);

        let mut symbols = Vec::new();

        for module in modules {
            let mut module_name = get_program_string(module, "module_name")?;
            // Special case for compat. The syntax could allow more more generic references to other
            // fields, but we don't need that for now, so we hardcode support for one specific field.
            if module_name == "#program.compat" {
                module_name = get_program_string(program, "compat")?;
            }
            let so_name = basename(module_name);

            // Lookup symbols specific to this module.
            let module_symbols = get_program_strvec(module, "symbols")?.ok_or(Status::NOT_FOUND)?;
            for symbol in module_symbols {
                let address = unsafe {
                    driver_host_find_symbol(
                        self.0,
                        so_name.as_ptr() as *const c_char,
                        so_name.len(),
                        symbol.as_ptr() as *const c_char,
                        symbol.len(),
                    )
                };
                if address == 0 {
                    return Err(Status::INVALID_ARGS);
                }
                symbols.push(fidl_fdf::NodeSymbol {
                    name: Some(symbol.to_string()),
                    address: Some(address),
                    module_name: Some(module_name.to_string()),
                    ..Default::default()
                });
            }
        }

        Ok(symbols)
    }
}

#[derive(Debug)]
struct Hooks(NonNull<fdf_sys::DriverRegistration>);
/// SAFETY: These hooks are just static pointers to code and therefore have no thread local state.
unsafe impl Send for Hooks {}
/// SAFETY: These hooks are valid on every thread after they are loaded.
unsafe impl Sync for Hooks {}

impl Hooks {
    fn new(registration: *mut fdf_sys::DriverRegistration) -> Result<Hooks, Status> {
        if registration.is_null() {
            log::error!("__fuchsia_driver_registration__ symbol not available in driver");
        }
        let registration: NonNull<fdf_sys::DriverRegistration> =
            NonNull::new(registration.cast()).ok_or(Status::NOT_FOUND)?;
        // SAFETY: The symbol is valid as long as the shared library is not closed. So its
        // lifetime must track that of |library| from above. We also do a null check to ensure
        // it's a valid pointer.
        let hooks = unsafe { registration.as_ref() };
        let version = hooks.version;
        if version < 1 || version > fdf_sys::DRIVER_REGISTRATION_VERSION_MAX {
            log::error!("Failed to start driver, unknown driver registration version: {version}");
            return Err(Status::WRONG_TYPE);
        }
        if hooks.v1.initialize.is_none() || hooks.v1.destroy.is_none() {
            log::error!("Failed to start driver, missing methods");
            return Err(Status::WRONG_TYPE);
        }
        Ok(Hooks(registration))
    }

    fn new_from_library(library: &Library) -> Result<Hooks, Status> {
        // SAFETY: The symbol is valid as long as the shared library is not closed. So its
        // lifetime must track that of |library| from above. We also do a null check to ensure
        // it's a valid pointer.
        Hooks::new(unsafe {
            libc::dlsym(library.ptr.as_ptr(), c"__fuchsia_driver_registration__".as_ptr())
        } as *mut _)
    }

    fn initialize(&self, channel_handle: fdf::DriverHandle) -> Token {
        // SAFETY: We know there are 0 other references to this. This is ref-safe because we know
        // the underlying memory is valid for the lifetime of the DriverInner object.
        let hooks = unsafe { self.0.as_ref() };
        let initialize_func = hooks.v1.initialize.unwrap();
        // SAFETY: We know it's safe to call initialize from the initial dispatcher.
        Token(unsafe { initialize_func(channel_handle.into_raw().get()) }.addr())
    }

    /// # Threading
    ///
    /// Must be called in the same sequence as initialize was called.
    fn destroy(&self, token: Token) {
        // SAFETY: We know there are 0 other references to this. This is ref-safe because we know
        // the underlying memory is valid for the lifetime of the DriverInner object.
        let hooks = unsafe { self.0.as_ref() };
        let destroy_func = hooks.v1.destroy.unwrap();
        // SAFETY: We need to call destroy if we've called initialize. This will be
        // synchronized with initialize to occur when the driver is destroyed in the shutdown
        // observer for the dispatcher.
        unsafe { destroy_func(token.0 as *mut _) };
    }
}

#[derive(Debug)]
struct Token(usize);

#[derive(Debug)]
struct LegacyDynamicallyLinkedState {
    #[allow(unused)]
    library: Library,

    // Modules listed in the program's module section.
    #[allow(unused)]
    modules_and_symbols: ModulesAndSymbols,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriverPowerState {
    Resumed,
    SuspendRequested,
    Suspended,
    ResumeRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerConfiguration {
    DriverOwned,
    RuntimeControlled,
    HostOwned,
}

#[derive(Debug)]
struct DriverInner {
    #[allow(unused)]
    legacy_state: Option<LegacyDynamicallyLinkedState>,

    // The hooks to initialize and destroy the driver. Backed by the registration symbol.
    hooks: Hooks,

    // The initial dispatcher of the driver.
    // This is where the initialize hook is called for the driver.
    dispatcher: Option<AutoReleaseDispatcher>,

    // The always-on interface to |dispatcher|.
    always_on_dispatcher: Option<AutoReleaseDispatcher>,

    // This is the handle we use to represent the driver to the driver runtime.
    runtime_handle: Option<fdf_env::Driver<Driver>>,

    // This is set through the initialize hook and passed into destroy.
    token: Option<Token>,

    // This signals to the driver_host that the driver has been shutdown.
    shutdown_signaler: Option<oneshot::Sender<Weak<Driver>>>,

    // This is the token representing the node of this driver in the driver manager.
    node_token: Option<fidl::Event>,

    // The power element configuration and state for this driver.
    power_element: PowerElementState,

    // The lease token that we give to the driver's resume hook to hold the
    // power element in an active/fully powered state.
    active_lease: Option<zx::EventPair>,

    // Keeps the power lease active during the stop sequence so the device doesn't
    // lose power until stopping completes.
    stop_lease: Option<zx::EventPair>,

    // The current power state/transition state of the driver.
    power_state: DriverPowerState,

    // A sender used to unblock the termination watcher when a pending resume
    // operation completes or is bypassed.
    resume_sender: Option<oneshot::Sender<()>>,

    // A sender used to signal an internal stop to the termination watcher
    // (e.g. if a power transition fails).
    internal_stop_tx: Option<oneshot::Sender<()>>,

    // The handle to the task running the power element runner for this driver.
    power_element_runner_task: Option<fuchsia_async::Task<()>>,

    resume_requester: Option<fdf_env::ResumeRequesterRegistration>,
}

#[allow(unused)]
#[derive(Debug)]
struct PowerElementHandles {
    control: Option<ClientEnd<ElementControlMarker>>,
    runner: Option<ServerEnd<ElementRunnerMarker>>,
    lessor: LessorProxy,
    element_token: DependencyToken,
    topology: Option<TopologyProxy>,
}

#[allow(unused)]
#[derive(Debug)]
enum PowerElementState {
    HostOwned(PowerElementHandles),
    DriverOwned(Option<ClientEnd<ElementControlMarker>>),
    RuntimeControlled(PowerElementHandles),
    NoElement,
}

impl Drop for DriverInner {
    fn drop(&mut self) {
        self.destroy();
    }
}

impl DriverInner {
    fn destroy(&mut self) {
        if let Some(token) = self.token.take() {
            self.hooks.destroy(token);
        }
    }
}

#[derive(Debug)]
pub(crate) struct Driver {
    url: String,
    node_name: String,
    inner: Mutex<DriverInner>,
}
impl Driver {
    fn process_power_elements(
        power_element_args: Option<fidl_fdf::PowerElementArgs>,
        power_config: PowerConfiguration,
        incoming: &Incoming,
    ) -> (PowerElementState, Option<fidl_fdf::PowerElementArgs>) {
        let Some(args) = power_element_args else {
            return (PowerElementState::NoElement, None);
        };

        let fidl_fdf::PowerElementArgs {
            control_client: Some(control_client),
            runner_server: Some(runner_server),
            lessor_client: Some(lessor_client),
            token: Some(token),
            ..
        } = args
        else {
            return (PowerElementState::NoElement, None);
        };

        match power_config {
            PowerConfiguration::DriverOwned => {
                let power_element = PowerElementState::DriverOwned(Some(control_client));
                let for_driver = Some(fidl_fdf::PowerElementArgs {
                    control_client: None,
                    runner_server: Some(runner_server),
                    lessor_client: Some(lessor_client),
                    token: Some(token),
                    ..Default::default()
                });
                (power_element, for_driver)
            }
            PowerConfiguration::RuntimeControlled => {
                let topology = match incoming.connect_protocol::<TopologyProxy>() {
                    Ok(t) => Some(t),
                    Err(e) => {
                        log::warn!("Failed to connect to Topology via incoming: {e:?}");
                        // Continue setup even if connecting to Topology fails.
                        None
                    }
                };
                let token_clone = token
                    .duplicate_handle(fidl::Rights::SAME_RIGHTS)
                    .expect("Failed to duplicate token");
                let power_element = PowerElementState::RuntimeControlled(PowerElementHandles {
                    control: Some(control_client),
                    runner: Some(runner_server),
                    lessor: lessor_client.into_proxy(),
                    element_token: token,
                    topology,
                });
                let for_driver = Some(fidl_fdf::PowerElementArgs {
                    token: Some(token_clone),
                    ..Default::default()
                });
                (power_element, for_driver)
            }
            PowerConfiguration::HostOwned => {
                let power_element = PowerElementState::HostOwned(PowerElementHandles {
                    control: Some(control_client),
                    runner: Some(runner_server),
                    lessor: lessor_client.into_proxy(),
                    element_token: token,
                    topology: None,
                });
                (power_element, None)
            }
        }
    }

    pub async fn load(
        env: &fdf_env::Environment,
        mut start_args: fidl_fdf::DriverStartArgs,
    ) -> Result<(Arc<Driver>, fidl_fdf::DriverStartArgs), Status> {
        // Parse out important
        let url = start_args.url.clone().ok_or(Status::INVALID_ARGS)?;
        let node_name = start_args.node_name.clone().unwrap_or_else(|| "unknown".to_string());
        let program = start_args.program.as_ref().ok_or(Status::INVALID_ARGS)?;
        let binary = get_program_string(program, "binary")?;
        let default_dispatcher_opts = DispatcherOpts::from(program);
        let scheduler_role = get_program_string(program, "default_dispatcher_scheduler_role")
            .unwrap_or("")
            .to_string();
        let allowed_scheduler_roles =
            get_program_strvec(program, "allowed_scheduler_roles")?.cloned();
        let memory_priority_role =
            get_program_string(program, "memory_priority_role").unwrap_or("");

        // Read binary from incoming namespace into vmo.
        let incoming = start_args.incoming.take().ok_or(Status::INVALID_ARGS)?;
        let incoming: Namespace = incoming.try_into().map_err(|_| Status::INVALID_ARGS)?;
        start_args.incoming = Some(incoming.clone().into());
        let incoming: Incoming = incoming.into();
        let vmo = get_file_vmo(&incoming, binary).await?;
        vmo.set_name(&zx::Name::new_lossy(basename(binary)))?;

        if binary != "driver/compat.so" {
            let symbols = driver_symbols::find_restricted_symbols(&vmo, &url)?;
            if !symbols.is_empty() {
                log::error!("Driver '{binary}' referenced restricted symbols: {symbols:?}");
                return Err(Status::NOT_SUPPORTED);
            }
        }
        let vmar = if !memory_priority_role.is_empty() {
            let flags = zx::VmarFlags::CAN_MAP_READ
                | zx::VmarFlags::CAN_MAP_WRITE
                | zx::VmarFlags::CAN_MAP_EXECUTE
                | zx::VmarFlags::CAN_MAP_SPECIFIC;
            // We choose 1GiB as the vmar size fairly arbitrarily. We aren't aware of any drivers
            // that would be larger than that.
            let vmar = fuchsia_runtime::vmar_root_self().allocate(0, 2_usize.pow(30), flags)?.0;
            if let Err(e) = fuchsia_scheduler::set_role_for_vmar(&vmar, memory_priority_role) {
                warn!("Failed to set vmar to priority role {memory_priority_role}: {e:?}");
            }
            Some(vmar)
        } else {
            None
        };

        let power_aware = get_program_string(program, "suspend_enabled")
            .map_or_else(|_| false, |val| matches!(val, "true"));

        let power_managed_dispatchers_enabled =
            get_program_string(program, "power_managed_dispatchers_enabled")
                .map_or_else(|_| false, |val| matches!(val, "true"));

        let power_config = match (power_aware, power_managed_dispatchers_enabled) {
            (_, true) => PowerConfiguration::RuntimeControlled,
            (true, false) => PowerConfiguration::DriverOwned,
            (false, false) => PowerConfiguration::HostOwned,
        };

        let library = LoaderService::try_load(vmo, &vmar).await?;
        start_args.vmar = vmar;
        let hooks = Hooks::new_from_library(&library)?;
        let modules_and_symbols = ModulesAndSymbols::load(program, &incoming).await?;
        modules_and_symbols.copy_to_start_args(&mut start_args);
        let legacy_state = Some(LegacyDynamicallyLinkedState { library, modules_and_symbols });
        let dispatcher_name = basename(&url);
        let node_token = start_args.node_token.as_ref().map(|t| {
            t.duplicate_handle(fidl::Rights::SAME_RIGHTS)
                .expect("Failed to duplicate node token handle.")
        });

        // All or nothing, either we get all the power element-related handles or we get none.
        let (power_element, for_driver) = Self::process_power_elements(
            start_args.power_element_args.take(),
            power_config,
            &incoming,
        );
        start_args.power_element_args = for_driver;

        let driver = Arc::new(Driver {
            url: url.clone(),
            node_name,
            inner: Mutex::new(DriverInner {
                legacy_state,
                hooks,
                dispatcher: None,
                always_on_dispatcher: None,
                runtime_handle: None,
                token: None,
                shutdown_signaler: None,
                node_token,
                power_element,
                active_lease: None,
                stop_lease: None,
                power_state: DriverPowerState::Resumed,
                resume_sender: None,
                internal_stop_tx: None,
                power_element_runner_task: None,
                resume_requester: None,
            }),
        });
        let driver_runtime_handle = env.new_driver(Arc::into_raw(driver.clone()));

        if !scheduler_role.is_empty() {
            driver_runtime_handle.add_allowed_scheduler_role(&scheduler_role);
        }
        if let Some(roles) = allowed_scheduler_roles {
            for role in roles {
                driver_runtime_handle.add_allowed_scheduler_role(&role);
            }
        }

        // The dispatcher must be shutdown before the dispatcher is destroyed.
        // Usually we will wait for the callback from |fdf_env::DriverShutdown| before destroying
        // the driver object (and hence the dispatcher).
        // In the case where we fail to start the driver, the driver object would be destructed
        // immediately, so here we hold an extra reference to the driver object to ensure the
        // dispatcher will not be destructed until shutdown completes.
        //
        // We do not destroy the dispatcher in the shutdown callback, to prevent crashes that
        // would happen if the driver attempts to access the dispatcher in its Stop hook.
        //
        // Currently we only support synchronized dispatchers for the default dispatcher.
        let driver_clone = driver.clone();
        let dispatcher = fdf::DispatcherBuilder::new()
            .name(&format!("{dispatcher_name}-default-{driver:p}"))
            .scheduler_role(&scheduler_role)
            .shutdown_observer(move |_| {
                let _ = driver_clone;
            });
        let dispatcher = match default_dispatcher_opts {
            DispatcherOpts::AllowSyncCalls => dispatcher.allow_thread_blocking(),
            DispatcherOpts::Default => dispatcher,
        };
        let dispatcher =
            AutoReleaseDispatcher::from(driver_runtime_handle.new_dispatcher(dispatcher)?);
        let always_on_dispatcher = dispatcher.always_on_dispatcher();
        driver.inner.lock().dispatcher = Some(dispatcher);
        driver.inner.lock().always_on_dispatcher = Some(always_on_dispatcher);
        driver.inner.lock().runtime_handle = Some(driver_runtime_handle);

        Ok((driver, start_args))
    }

    pub async fn initialize(
        env: &fdf_env::Environment,
        mut start_args: fidl_fdf::DriverStartArgs,
        dynamic_linking_abi: u64,
    ) -> Result<(Arc<Driver>, fidl_fdf::DriverStartArgs), Status> {
        // Parse out important fields.
        let url = start_args.url.clone().ok_or(Status::INVALID_ARGS)?;
        let node_name = start_args.node_name.clone().unwrap_or_else(|| "unknown".to_string());
        let program = start_args.program.as_ref().ok_or(Status::INVALID_ARGS)?;
        let binary = get_program_string(program, "binary")?;
        let default_dispatcher_opts = DispatcherOpts::from(program);
        let scheduler_role =
            get_program_string(program, "default_dispatcher_scheduler_role").unwrap_or("");
        let allowed_scheduler_roles = get_program_strvec(program, "allowed_scheduler_roles")?;

        let power_aware = get_program_string(program, "suspend_enabled")
            .map_or_else(|_| false, |val| matches!(val, "true"));

        let power_managed_dispatchers_enabled =
            get_program_string(program, "power_managed_dispatchers_enabled")
                .map_or_else(|_| false, |val| matches!(val, "true"));

        let power_config = match (power_aware, power_managed_dispatchers_enabled) {
            (_, true) => PowerConfiguration::RuntimeControlled,
            (true, false) => PowerConfiguration::DriverOwned,
            (false, false) => PowerConfiguration::HostOwned,
        };

        let incoming = start_args.incoming.take().ok_or(Status::INVALID_ARGS)?;
        let incoming: Namespace = incoming.try_into().map_err(|_| Status::INVALID_ARGS)?;
        start_args.incoming = Some(incoming.clone().into());
        let incoming: Incoming = incoming.into();

        let loaded_driver = LoadedDriver(dynamic_linking_abi);
        let hooks = loaded_driver.get_hooks(basename(binary))?;
        let mut symbols = loaded_driver.get_symbols(program)?;
        start_args.symbols.get_or_insert_default().append(&mut symbols);
        let dispatcher_name = basename(&url);
        let node_token = start_args.node_token.as_ref().map(|t| {
            t.duplicate_handle(fidl::Rights::SAME_RIGHTS)
                .expect("Failed to duplicate node token handle.")
        });

        // All or nothing, either we get all the power element-related handles or we get none.
        let (power_element, for_driver) = Self::process_power_elements(
            start_args.power_element_args.take(),
            power_config,
            &incoming,
        );
        start_args.power_element_args = for_driver;

        let driver = Arc::new(Driver {
            url: url.clone(),
            node_name,
            inner: Mutex::new(DriverInner {
                legacy_state: None,
                hooks,
                dispatcher: None,
                always_on_dispatcher: None,
                runtime_handle: None,
                token: None,
                shutdown_signaler: None,
                node_token,
                power_element,
                active_lease: None,
                stop_lease: None,
                power_state: DriverPowerState::Resumed,
                resume_sender: None,
                internal_stop_tx: None,
                power_element_runner_task: None,
                resume_requester: None,
            }),
        });
        let driver_runtime_handle = env.new_driver(Arc::into_raw(driver.clone()));

        if !scheduler_role.is_empty() {
            driver_runtime_handle.add_allowed_scheduler_role(scheduler_role);
        }
        if let Some(roles) = allowed_scheduler_roles {
            for role in roles {
                driver_runtime_handle.add_allowed_scheduler_role(role.as_str());
            }
        }

        // The dispatcher must be shutdown before the dispatcher is destroyed.
        // Usually we will wait for the callback from |fdf_env::DriverShutdown| before destroying
        // the driver object (and hence the dispatcher).
        // In the case where we fail to start the driver, the driver object would be destructed
        // immediately, so here we hold an extra reference to the driver object to ensure the
        // dispatcher will not be destructed until shutdown completes.
        //
        // We do not destroy the dispatcher in the shutdown callback, to prevent crashes that
        // would happen if the driver attempts to access the dispatcher in its Stop hook.
        //
        // Currently we only support synchronized dispatchers for the default dispatcher.
        let driver_clone = driver.clone();
        let dispatcher = fdf::DispatcherBuilder::new()
            .name(&format!("{dispatcher_name}-default-{driver:p}"))
            .scheduler_role(scheduler_role)
            .shutdown_observer(move |_| {
                let _ = driver_clone;
            });
        let dispatcher = match default_dispatcher_opts {
            DispatcherOpts::AllowSyncCalls => dispatcher.allow_thread_blocking(),
            DispatcherOpts::Default => dispatcher,
        };
        let dispatcher =
            AutoReleaseDispatcher::from(driver_runtime_handle.new_dispatcher(dispatcher)?);
        let always_on_dispatcher = dispatcher.always_on_dispatcher();
        driver.inner.lock().dispatcher = Some(dispatcher);
        driver.inner.lock().always_on_dispatcher = Some(always_on_dispatcher);
        driver.inner.lock().runtime_handle = Some(driver_runtime_handle);

        Ok((driver, start_args))
    }

    /// Must be called from the driver host main thread.
    pub async fn start(
        self: &Arc<Self>,
        start_args: fidl_fdf::DriverStartArgs,
        driver_request: ServerEnd<fdh::DriverMarker>,
        shutdown_signaler: oneshot::Sender<Weak<Driver>>,
        scope: &fuchsia_async::Scope,
    ) -> Result<(), Status> {
        self.inner.lock().shutdown_signaler = Some(shutdown_signaler);

        let weak_always_on = self
            .inner
            .lock()
            .always_on_dispatcher
            .as_ref()
            .expect("always on dispatcher not set")
            .as_async_dispatcher();
        let (client, server) = fdf_fidl::create_channel_with_dispatchers::<fidl_next_fdf::Driver, _>(
            weak_always_on.clone(),
            weak_always_on.clone(),
        );
        let client_dispatcher = ClientDispatcher::new(client);
        let client = client_dispatcher.client();

        let client_dispatcher_task = {
            let self_clone = self.clone();
            weak_always_on.compute(async move {
                {
                    let mut inner = self_clone.inner.lock();
                    let hooks = &inner.hooks;

                    inner.token =
                        Some(hooks.initialize(server.into_untyped().into_driver_handle()));
                };

                client_dispatcher.run_client().await
            })
        };

        let start_args_next = convert_start_args(start_args);
        let start_result = client.start(start_args_next).await;
        match start_result {
            Ok(Ok(())) => {}
            Ok(Err(status)) => {
                warn!("Driver failed to start: {status:?}");
                self.shutdown(driver_request);
                return Err(status.err().unwrap_or(Status::INTERNAL));
            }
            Err(e) => {
                warn!("Driver start FIDL error: {:?}", e);
                self.shutdown(driver_request);
                return Err(Status::INTERNAL);
            }
        }

        let (internal_stop_tx, internal_stop_rx) = oneshot::channel::<()>();
        {
            self.inner.lock().internal_stop_tx = Some(internal_stop_tx);
        }

        self.start_power_element_runner(client.clone(), scope);

        let weak_self = Arc::downgrade(self);
        scope.spawn_local(async move {
            Driver::run_termination_watcher(
                weak_self,
                driver_request,
                client_dispatcher_task,
                internal_stop_rx,
                client,
            )
            .await;
        });

        Ok(())
    }

    async fn run_termination_watcher<F, O, E>(
        weak_self: Weak<Self>,
        driver_request: ServerEnd<fdh::DriverMarker>,
        client_dispatcher_task: F,
        internal_stop_rx: oneshot::Receiver<()>,
        client: DriverClient,
    ) where
        F: std::future::Future<Output = Result<O, E>> + Send + Unpin + 'static,
        O: std::fmt::Debug,
        E: std::fmt::Debug,
    {
        // Wait for driver manager to issue stop or the driver to have dropped its end of the
        // driver channel.
        let mut driver_request_stream = driver_request.into_stream();
        let mut client_dispatcher_done = client_dispatcher_task.fuse();
        let mut internal_stop_receiver = internal_stop_rx.fuse();

        let (resume_sender, resume_receiver) = oneshot::channel();

        let should_stop = futures::select! {
            res = driver_request_stream.try_next().fuse() => {
                match res {
                    Ok(Some(request)) => match request {
                        fdh::DriverRequest::Stop { control_handle: _ } => {
                            Driver::handle_stop_request(weak_self.clone(), resume_sender)
                        }
                    },
                    Ok(None) => {
                        log::warn!("Driver request stream closed unexpectedly");
                        false
                    }
                    Err(e) => {
                        log::warn!("Error in driver request stream: {e:?}");
                        false
                    }
                }
            }
            res = internal_stop_receiver => {
                log::warn!("Power transition failed, stopping driver internally {:?}", res);
                // Unblock the resume_receiver.
                resume_sender.send(()).ok();
                true
            }
            res = client_dispatcher_done => {
                log::warn!("Client dispatcher finished unexpectedly: {:?}", res);
                false
            }
        };

        if should_stop {
            // Ignore the result as its possible we dropped the sender by not assigning it
            // if we did not need to wait for a resume (ie. the driver was already resumed).
            resume_receiver.await.ok();
            if let Err(e) = client.stop().await {
                log::warn!("Failed to send stop request: {e:?}");
            }
            if let Err(e) = client_dispatcher_done.await {
                log::error!("Client dispatcher failed: {e:?}");
            }
        }

        if let Some(driver) = weak_self.upgrade() {
            drop(driver.inner.lock().stop_lease.take());
        }

        let server_end = ServerEnd::new(
            Arc::into_inner(driver_request_stream.into_inner().0)
                .expect("outstanding references to channel, possibly unhandeled messages?")
                .into_channel()
                .into(),
        );

        if let Some(strong_self) = weak_self.upgrade() {
            strong_self.shutdown(server_end);
        } else {
            log::error!("Failed to upgrade weak pointer to driver");
        }
    }

    // Called by the driver manager when it wants to stop the driver.
    // Returns true if the Stop lifecycle hook should be called on the driver server.
    fn handle_stop_request(weak_self: Weak<Self>, resume_sender: oneshot::Sender<()>) -> bool {
        let Some(driver) = weak_self.upgrade() else {
            log::warn!("Driver no longer exists");
            return false;
        };

        // Note: We first have to completely resume the driver
        // (if its suspended) before we can stop the driver.
        let state = driver.inner.lock().power_state;
        match state {
            DriverPowerState::Suspended | DriverPowerState::SuspendRequested => {
                driver.inner.lock().resume_sender = Some(resume_sender);
                driver.request_power_lease();
                {
                    let mut inner = driver.inner.lock();
                    inner.stop_lease = inner.active_lease.take();
                }
            }
            DriverPowerState::ResumeRequested => {
                driver.inner.lock().resume_sender = Some(resume_sender);
                {
                    let mut inner = driver.inner.lock();
                    inner.stop_lease = inner.active_lease.take();
                }
            }
            DriverPowerState::Resumed => {
                // Nothing to do here if the driver is already in a Resumed state.
            }
        };

        true
    }

    fn start_power_element_runner(
        self: &Arc<Self>,
        client: DriverClient,
        scope: &fuchsia_async::Scope,
    ) {
        let mut inner = self.inner.lock();
        match &mut inner.power_element {
            PowerElementState::HostOwned(PowerElementHandles { runner, .. }) => {
                // Host-owned power element: simply acknowledges the level-change request.
                if let Some(runner_channel) = runner.take() {
                    scope.spawn_local(async move {
                        let mut element_runner_req_stream = runner_channel.into_stream();
                        while let Ok(Some(ElementRunnerRequest::SetLevel { level: _, responder })) =
                            element_runner_req_stream.try_next().await
                        {
                            responder.send().ok();
                        }
                    });
                } else {
                    log::error!(
                        "Host-owned power element: Failed to get power element runner handle for {}",
                        self.node_name
                    );
                }
            }
            PowerElementState::RuntimeControlled(PowerElementHandles { runner, .. }) => {
                // Runtime-controlled power element: Deeply integrated with the driver and the
                // driver runtime. Element changes propagate through to the driver runtime to
                // manage task dispatching, and call a lifecycle hook on the driver to manage
                // the state of the hardware.
                inner.power_element_runner_task = Some(self.spawn_power_element_runner(
                    runner.take().expect("runner"),
                    client,
                    scope,
                ));

                let weak_self = Arc::downgrade(self);
                let resume_requester = fdf_env::ResumeRequester::new(move || {
                    if let Some(driver) = weak_self.upgrade() {
                        driver.request_power_lease();
                        Ok(())
                    } else {
                        Err(zx::Status::PEER_CLOSED)
                    }
                });

                let registration = inner
                    .runtime_handle
                    .as_ref()
                    .expect("driver must have a runtime handle")
                    .register_resume_requester(resume_requester);
                inner.resume_requester = Some(registration);
            }
            _ => {}
        }
    }

    // Signals our local oneshots in case of a failure communicating
    // with the power broker to unblock driver shutdown.
    fn signal_shutdown_fallbacks(&self) {
        let mut inner = self.inner.lock();
        if let Some(sender) = inner.resume_sender.take() {
            sender.send(()).ok();
        }
        if let Some(sender) = inner.internal_stop_tx.take() {
            sender.send(()).ok();
        }
    }

    async fn handle_suspend_transition(self: &Arc<Self>, client: &DriverClient) -> Result<(), ()> {
        {
            let mut inner = self.inner.lock();
            inner.power_state = DriverPowerState::SuspendRequested;
        }
        if self.suspend_driver(client).await.is_err() {
            self.signal_shutdown_fallbacks();
            return Err(());
        }
        {
            let mut inner = self.inner.lock();
            inner.power_state = DriverPowerState::Suspended;
        }
        Ok(())
    }

    async fn handle_resume_transition(self: &Arc<Self>, client: &DriverClient) -> Result<(), ()> {
        let lease = {
            let mut inner = self.inner.lock();

            // Skip resume transition if the driver is already resumed or resume requested.
            if inner.power_state == DriverPowerState::Resumed
                || inner.power_state == DriverPowerState::ResumeRequested
            {
                return Ok(());
            }

            inner.power_state = DriverPowerState::ResumeRequested;
            inner.active_lease.take()
        };

        if self.resume_driver(client, lease).await.is_err() {
            self.signal_shutdown_fallbacks();
            return Err(());
        }

        {
            let mut inner = self.inner.lock();
            inner.power_state = DriverPowerState::Resumed;
            if let Some(sender) = inner.resume_sender.take() {
                sender.send(()).ok();
            }
        }
        Ok(())
    }

    fn spawn_power_element_runner(
        self: &Arc<Self>,
        runner_channel: ServerEnd<ElementRunnerMarker>,
        client: DriverClient,
        scope: &fuchsia_async::Scope,
    ) -> fuchsia_async::Task<()> {
        let mut element_runner_req_stream = runner_channel.into_stream();
        let weak_self = Arc::downgrade(self);
        scope.spawn_local(async move {
            while let Ok(Some(ElementRunnerRequest::SetLevel { level, responder })) =
                element_runner_req_stream.try_next().await
            {
                let Some(driver) = weak_self.upgrade() else {
                    log::warn!("spawn_power_element_runner: driver was freed");
                    break;
                };

                // TODO(https://fxbug.dev/515015623): Use enums instead of 0/1.
                if level == 0 {
                    if driver.handle_suspend_transition(&client).await.is_err() {
                        break;
                    }
                    if let Err(e) = responder.send() {
                        log::warn!("spawn_power_element_runner: failed to send SetLevel(0) response: {:?}", e);
                    }
                } else {
                    if level != 1 {
                        log::error!("Invalid power element level (treating as resume): {}", level);
                    }
                    if driver.handle_resume_transition(&client).await.is_err() {
                        break;
                    }
                    if let Err(e) = responder.send() {
                        log::warn!("spawn_power_element_runner: failed to send SetLevel({}) response: {:?}", level, e);
                    }
                }
            }
        }).into()
    }

    pub fn get_url(&self) -> &str {
        self.url.as_str()
    }

    pub fn duplicate_node_token(&self) -> Option<fidl::Event> {
        self.inner.lock().node_token.as_ref().map(|token| {
            token
                .duplicate_handle(fidl::Rights::SAME_RIGHTS)
                .expect("Failed to duplicate node token handle.")
        })
    }

    /// This function is called when the driver needs to be taken out of the suspended state.
    /// This is only supported when the power element is in `RuntimeControlled` state. This
    /// returns immediately and the work is done asynchronously. Eventually a `SetLevel(1)`
    /// request will arrive via the `ElementRunner` protocol where the driver will resume.
    ///
    /// If the power element is not in `RuntimeControlled` state, this function will do nothing.
    pub fn request_power_lease(self: &Arc<Self>) {
        let self_clone = self.clone();
        let remote_token;
        let dispatcher;
        {
            let mut inner = self.inner.lock();
            let PowerElementState::RuntimeControlled(..) = &inner.power_element else {
                return;
            };

            let (local_token, remote) = zx::EventPair::create();
            inner.active_lease = Some(local_token);
            remote_token = remote;
            dispatcher = inner
                .always_on_dispatcher
                .as_ref()
                .expect("dispatcher should always be valid")
                .as_async_dispatcher();
        }

        // This spawns the request to acquire a power lease to run under the driver dispatcher
        // asynchronously.
        dispatcher.spawn(async move {
            self_clone.acquire_power_lease(remote_token).await;
        });
    }

    async fn acquire_power_lease(&self, remote_token: zx::EventPair) {
        let (element_token, topology) = {
            let inner = self.inner.lock();
            let PowerElementState::RuntimeControlled(PowerElementHandles {
                element_token,
                topology,
                ..
            }) = &inner.power_element
            else {
                return;
            };
            let element_token = element_token
                .duplicate_handle(fidl::Rights::SAME_RIGHTS)
                .expect("Failed to duplicate token");
            let topology = topology.clone();
            (element_token, topology)
        };

        // Lease name limit is 64.
        let mut lease_name = format!("{}:{}", self.node_name, basename(&self.url));
        if lease_name.len() > 64 {
            let cut = lease_name.floor_char_boundary(64);
            lease_name.truncate(cut);
        }

        let lease_schema = LeaseSchema {
            lease_token: Some(remote_token),
            lease_name: Some(lease_name),
            dependencies: Some(vec![LeaseDependency {
                requires_token: Some(element_token),
                requires_level: Some(1),
                ..Default::default()
            }]),
            should_return_pending_lease: Some(true),
            ..Default::default()
        };

        if let Some(topology) = topology {
            match topology.lease(lease_schema).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    log::error!("Failed to request lease: {e:?}");
                    self.signal_shutdown_fallbacks();
                }
                Err(e) => {
                    log::error!("FIDL error requesting lease: {e:?}");
                    self.signal_shutdown_fallbacks();
                }
            }
        } else {
            log::error!("Topology proxy not available");
            self.signal_shutdown_fallbacks();
        }
    }

    async fn suspend_driver(self: &Arc<Self>, client: &DriverClient) -> Result<(), ()> {
        // Suspending a driver involves two steps:
        // 1: Suspend the driver from the perspective of the driver runtime. This will put the
        // dispatchers into the suspended state ensuring no more work will be performed for them.
        // 2: Send the suspend call to the driver via the Driver protocol. This will give the
        // driver the opportunity to suspend its hardware. This runs on the always-on dispatcher
        // so it bypasses the suspend we do in step 1.

        let (completer_sender, completer_receiver) = oneshot::channel();
        let completer = fdf_env::SuspendCompleter::new(move || {
            completer_sender.send(()).ok();
        });

        self.inner
            .lock()
            .runtime_handle
            .as_ref()
            .expect("driver must have a runtime handle")
            .driver_suspend(completer);

        match completer_receiver.await {
            Ok(()) => match client.suspend().await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => {
                    log::error!("Driver suspend application error for {}: {:?}", self.url, e,);
                    Err(())
                }
                Err(e) => {
                    log::error!("Driver suspend FIDL error for {}: {:?}", self.url, e);
                    Err(())
                }
            },
            Err(_) => {
                log::error!("Failed to receive suspend completion for {}", self.url);
                Err(())
            }
        }
    }

    async fn resume_driver(
        self: &Arc<Self>,
        client: &DriverClient,
        lease: Option<zx::EventPair>,
    ) -> Result<(), ()> {
        // Resuming a driver involves two steps:
        // 1: Send the resume call to the driver via the Driver protocol. This will give the
        // driver the opportunity to resume its hardware. This runs on the always-on dispatcher
        // so it bypasses the suspend we do in step 1 of suspend_driver.
        // 2: Resume the driver from the perspective of the driver runtime. This will put the
        // dispatchers into the running state ensuring that work is once again processed.
        match client.resume(lease).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                log::error!("Driver resume application error for {}: {:?}", self.url, e);
                return Err(());
            }
            Err(e) => {
                log::error!("Driver resume FIDL error for {}: {:?}", self.url, e);
                return Err(());
            }
        }

        self.inner
            .lock()
            .runtime_handle
            .as_ref()
            .expect("driver must have a runtime handle")
            .driver_resume();
        Ok(())
    }

    /// Shutdown the driver. The process is asynchronous. All references to the driver should be
    /// dropped prior to invoking this method.
    /// This method will do nothing if invoked multiple times or if runtime_handle was never
    /// initialized.
    fn shutdown(&self, driver_request: ServerEnd<fdh::DriverMarker>) {
        // Drop the power element runner task to prevent it from sending any
        // more requests on the runtime_handle which we are about to take.
        drop(self.inner.lock().power_element_runner_task.take());

        // Clean up our resume requester from the runtime. This is only available
        // in drivers that have RuntimeControlled power_elements.
        if let Some(registration) = self.inner.lock().resume_requester.take() {
            registration.unregister();
        }

        let runtime_handle = self.inner.lock().runtime_handle.take();
        let shutdown_signaler = self.inner.lock().shutdown_signaler.take();

        // Drop the element control channel to destroy the power element
        match &mut self.inner.lock().power_element {
            PowerElementState::HostOwned(PowerElementHandles { control, .. })
            | PowerElementState::RuntimeControlled(PowerElementHandles { control, .. })
            | PowerElementState::DriverOwned(control) => {
                drop(control.take());
            }
            _ => {}
        }

        if let Some(runtime_handle) = runtime_handle {
            runtime_handle.shutdown(move |runtime_handle| {
                // SAFETY: This is safe because we previously leaked the arc when creating the
                // driver. Recovering through the shutdown callback is the expected flow.
                let this = unsafe { Arc::from_raw(runtime_handle.0) };
                if let Some(shutdown_signaler) = shutdown_signaler {
                    // This can fail if start fails.
                    shutdown_signaler.send(Arc::downgrade(&this)).ok();
                }

                // Trigger destroy hook.
                // In theory this should be the last reference to the driver, however if shutdown
                // is invoked from the main driver_host thread, it's not guaranteed.
                this.inner.lock().destroy();

                let _ = driver_request.close_with_epitaph(Ok(()));
            });
        };
    }
}

impl PartialEq<fdf_env::UnownedDriver> for Driver {
    fn eq(&self, other: &fdf_env::UnownedDriver) -> bool {
        self.inner.lock().runtime_handle.as_ref().map(|h| h == other).unwrap_or(false)
    }
}
