// Copyright 2028 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{
    Features, MountAction, expose_root, parse_features, parse_numbered_handles,
    run_container_features, serve_component_runner, serve_container_controller,
    serve_container_info, serve_graphical_presenter, serve_lutex_controller,
};
use anyhow::{Context, Error, anyhow, bail};
use bootreason::get_or_init_android_bootreason;
use bstr::{BString, ByteSlice};
use devicetree::parser::parse_devicetree;
use devicetree::types::Devicetree;
use fidl::endpoints::{RequestStream, ServerEnd};
use fidl_fuchsia_boot as fboot;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_component_runner::{TaskProviderRequest, TaskProviderRequestStream};
use fidl_fuchsia_element as felement;
use fidl_fuchsia_feedback::CrashReporterMarker;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_mem as fmem;
use fidl_fuchsia_memory_attribution as fattribution;
use fidl_fuchsia_starnix_binder as fbinder;
use fidl_fuchsia_starnix_container as fstarcontainer;
use fidl_fuchsia_sysinfo as fsysinfo;
use fidl_fuchsia_time_external::AdjustMarker;
use fuchsia_async as fasync;
use fuchsia_async::DurationExt;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_sync};
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect as inspect;
use fuchsia_runtime as fruntime;
use futures::channel::oneshot;
use futures::{FutureExt, StreamExt, TryStreamExt};
use serde::Deserialize;
use starnix_container_structured_config::Config as ContainerStructuredConfig;
use starnix_core::device::remote_block_device::remote_block_device_init;
use starnix_core::execution::{
    create_init_process, create_system_task, execute_task_with_prerun_result,
};
use starnix_core::fs::fuchsia::new_remotefs_in_root;
use starnix_core::fs::tmpfs::TmpFs;
use starnix_core::security;
use starnix_core::task::container_namespace::ContainerNamespace;
use starnix_core::task::{
    CurrentTask, ExitStatus, Kernel, RoleOverrides, SchedulerManager, parse_cmdline,
};
use starnix_core::vfs::{FileSystemOptions, FsContext, LookupContext, Namespace, WhatToMount};
use starnix_logging::{
    CATEGORY_STARNIX, NAME_CREATE_CONTAINER, log_debug, log_error, log_info, log_warn,
};
use starnix_modules::{
    init_common_devices, register_common_file_systems, register_common_syscalls,
};
use starnix_modules_layeredfs::{LayeredFsBuilder, LayeredFsMounts};
use starnix_modules_magma::get_magma_params;
use starnix_modules_overlayfs::OverlayStack;
use starnix_modules_rtc::rtc_device_init;

use starnix_task_command::TaskCommand;
use starnix_uapi::errors::{ENOENT, SourceContext};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::resource_limits::Resource;
use starnix_uapi::{errno, tid_t};
use std::ffi::CString;

use std::sync::Arc;
use zx::Task as _;

use std::sync::Weak;

use crate::serve_memory_attribution_provider_container;
use attribution_server::{AttributionServer, AttributionServerHandle};

/// Manages the memory attribution protocol for a Starnix container.
struct ContainerMemoryAttributionManager {
    /// Holds state for the hanging-get attribution protocol.
    memory_attribution_server: AttributionServerHandle,
}

impl ContainerMemoryAttributionManager {
    /// Creates a new [ContainerMemoryAttributionManager] from a Starnix kernel and the moniker
    /// token of the container component.
    pub fn new(kernel: Weak<Kernel>, component_instance: zx::Event) -> Self {
        let memory_attribution_server = AttributionServer::new(Box::new(move || {
            let kernel_ref = match kernel.upgrade() {
                None => return vec![],
                Some(k) => k,
            };
            attribution_info_for_kernel(kernel_ref.as_ref(), &component_instance)
        }));

        ContainerMemoryAttributionManager { memory_attribution_server }
    }

    /// Creates a new observer for the attribution information from this container.
    pub fn new_observer(
        &self,
        control_handle: fattribution::ProviderControlHandle,
    ) -> attribution_server::Observer {
        self.memory_attribution_server.new_observer(control_handle)
    }
}

/// Generates the attribution information for the Starnix kernel ELF component. The attribution
/// information for the container is handled by the container component, not the kernel
/// component itself, even if both are hosted within the same kernel process.
fn attribution_info_for_kernel(
    kernel: &Kernel,
    component_instance: &zx::Event,
) -> Vec<fattribution::AttributionUpdate> {
    // Start the server to handle the memory attribution requests for the container, and provide
    // a handle to get detailed attribution. We start a new task as each incoming connection is
    // independent.
    let (client_end, server_end) =
        fidl::endpoints::create_request_stream::<fattribution::ProviderMarker>();
    fuchsia_async::Task::spawn(serve_memory_attribution_provider_container(server_end, kernel))
        .detach();

    let starnix_kernel_id = Some(1);
    let starnix_kernel_principal = fattribution::NewPrincipal {
        identifier: starnix_kernel_id,
        description: Some(fattribution::Description::Part("starnix_kernel".to_string())),
        principal_type: Some(fattribution::PrincipalType::Part),
        // This part is created for accounting. It holds the resource used for starnix
        // kernel operation. It neither has sub-principals, nor publishes attribution,
        // hence it does not need to be tied to a provider server end.
        detailed_attribution: None,
        ..Default::default()
    };

    let starnix_kernel_attribution = fattribution::UpdatedPrincipal {
        identifier: starnix_kernel_id, // Recipient.
        resources: Some(fattribution::Resources::Data(fattribution::Data {
            resources: vec![fattribution::Resource::ProcessMapped(fattribution::ProcessMapped {
                process: fuchsia_runtime::process_self().koid().unwrap().raw_koid(),
                base: 0, // Attribute all the range.
                len: u64::MAX,
                hint_skip_handle_table: false,
            })],
        })),
        ..Default::default()
    };

    let container_id = Some(2);
    let new_principal = fattribution::NewPrincipal {
        identifier: container_id,
        description: Some(fattribution::Description::Component(
            component_instance.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
        )),
        principal_type: Some(fattribution::PrincipalType::Runnable),
        detailed_attribution: Some(client_end),
        ..Default::default()
    };
    let attribution = fattribution::UpdatedPrincipal {
        identifier: container_id,
        resources: Some(fattribution::Resources::Data(fattribution::Data {
            resources: vec![fattribution::Resource::KernelObject(
                fuchsia_runtime::job_default().koid().unwrap().raw_koid(),
            )],
        })),
        ..Default::default()
    };

    vec![
        fattribution::AttributionUpdate::Add(new_principal),
        fattribution::AttributionUpdate::Add(starnix_kernel_principal),
        fattribution::AttributionUpdate::Update(attribution),
        fattribution::AttributionUpdate::Update(starnix_kernel_attribution),
    ]
}

#[derive(Debug)]
pub struct ContainerStartInfo {
    /// Configuration specified by the component's `program` block.
    pub program: ContainerProgram,

    pub config: ContainerStructuredConfig,

    /// The outgoing directory of the container, used to serve protocols on behalf of the container.
    /// For example, the starnix_kernel serves a component runner in the containers' outgoing
    /// directory.
    outgoing_dir: Option<zx::Channel>,

    /// Mapping of top-level namespace entries to an associated channel.
    /// For example, "/svc" to the respective channel.
    pub container_namespace: ContainerNamespace,

    /// The runtime directory of the container, used to provide CF introspection.
    runtime_dir: Option<ServerEnd<fio::DirectoryMarker>>,

    /// An eventpair that debuggers can use to defer the launch of the container.
    break_on_start: Option<zx::EventPair>,

    /// Component moniker token for the container component. This token is used in various protocols
    /// to uniquely identify a component.
    component_instance: Option<zx::Event>,
}

const MISSING_CONFIG_VMO_CONTEXT: &str = concat!(
    "Retrieving container config VMO. ",
    "If this fails, make sure your container CML includes ",
    "//src/starnix/containers/container.shard.cml.",
);

impl ContainerStartInfo {
    fn new(mut start_info: frunner::ComponentStartInfo) -> Result<Self, Error> {
        let program = start_info.program.as_ref().context("retrieving program block")?;
        let program: ContainerProgram =
            runner::serde::deserialize_program(&program).context("parsing program block")?;

        let encoded_config =
            start_info.encoded_config.as_ref().context(MISSING_CONFIG_VMO_CONTEXT)?;
        let config = match encoded_config {
            fmem::Data::Bytes(b) => ContainerStructuredConfig::from_bytes(b),
            fmem::Data::Buffer(b) => ContainerStructuredConfig::from_vmo(&b.vmo),
            other => anyhow::bail!("unknown Data variant {other:?}"),
        }
        .context("parsing container structured config")?;

        let ns = start_info.ns.take().context("retrieving container namespace")?;
        let container_namespace = ContainerNamespace::from(ns);

        let outgoing_dir = start_info.outgoing_dir.take().map(|dir| dir.into_channel());
        let component_instance = start_info.component_instance;

        Ok(Self {
            program,
            config,
            outgoing_dir,
            container_namespace,
            component_instance,
            break_on_start: start_info.break_on_start,
            runtime_dir: start_info.runtime_dir,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerProgram {
    /// The name of this container.
    name: String,

    /// The command line for the initial process for this container.
    init: Vec<String>,

    /// The command line for the kernel.
    #[serde(default)]
    kernel_cmdline: String,

    /// The specifications for the file system mounts for this container.
    #[serde(default)]
    mounts: Vec<String>,

    /// The features enabled for this container.
    #[serde(default)]
    pub features: Vec<String>,

    /// The resource limits to apply to this container.
    #[serde(default)]
    rlimits: Vec<String>,

    /// The path that the container will wait until exists before considering itself to have started.
    #[serde(default)]
    startup_file_path: String,

    /// The default seclabel that is applied to components that are instantiated in this container.
    ///
    /// Components can override this by setting the `seclabel` field in their program block.
    #[serde(default)]
    pub default_seclabel: Option<String>,

    /// The default uid that is applied to components that are instantiated in this container.
    ///
    /// Components can override this by setting the `uid` field in their program block.
    #[serde(default = "default_uid")]
    pub default_uid: runner::serde::StoreAsString<u32>,

    /// The default mount options to use when mounting directories from a component's namespace.
    ///
    /// Each string is expected to follow the format: "<namespace_path>:<mount_options>".
    pub default_ns_mount_options: Option<Vec<String>>,

    /// Specifies role names to use for "realtime" tasks based on their process & thread names.
    ///
    /// Zircon's scheduler doesn't support configuring tasks to always preempt non-"realtime"
    /// tasks without specifying a constant bandwidth profile. These profiles specify the period and
    /// expected runtime of a "realtime" task, bounding the amount of work it is allowed to perform
    /// at an elevated "realtime" priority.
    ///
    /// Because constant bandwidth profiles require workload-specific tuning, we can't uniformly
    /// apply a single profile for all "realtime" tasks. Instead, this container configuration
    /// allows us to specify different constant bandwidth profiles for different workloads.
    #[serde(default)]
    task_role_overrides: Vec<TaskSchedulerMapping>,
}

/// Specifies a role override for a class of tasks whose process and thread names match provided
/// patterns.
#[derive(Default, Deserialize)]
struct TaskSchedulerMapping {
    /// The role name to use for tasks matching the provided patterns.
    role: String,
    /// A regular expression that will be matched against the process' command.
    process: String,
    /// A regular expression that will be matched against the thread's command.
    thread: String,
    /// An optional regular expression that will be matched against the task's cgroup path.
    cgroup: Option<String>,
}

impl std::fmt::Debug for TaskSchedulerMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "process `{}` thread `{}` cgroup `{:?}` role `{}`",
            self.process, self.thread, self.cgroup, self.role
        )
    }
}

fn default_uid() -> runner::serde::StoreAsString<u32> {
    runner::serde::StoreAsString(42)
}

// Creates a CString from a String. Calling this with an invalid CString will panic.
fn to_cstr(str: &str) -> CString {
    CString::new(str.to_string()).unwrap()
}

#[must_use = "The container must run serve on this config"]
pub struct ContainerServiceConfig {
    start_info: ContainerStartInfo,
    request_stream: frunner::ComponentControllerRequestStream,
    receiver: oneshot::Receiver<Result<ExitStatus, Error>>,
}

pub struct Container {
    /// The `Kernel` object that is associated with the container.
    pub kernel: Arc<Kernel>,

    memory_attribution_manager: ContainerMemoryAttributionManager,

    /// Inspect node holding information about the state of the container.
    _node: inspect::Node,

    /// Until negative trait bound are implemented, using `*mut u8` to prevent transferring
    /// Container across threads.
    _thread_bound: std::marker::PhantomData<*mut u8>,
}

impl Container {
    pub fn system_task(&self) -> &CurrentTask {
        self.kernel.kthreads.system_task()
    }

    async fn serve_outgoing_directory(
        &self,
        outgoing_dir: Option<zx::Channel>,
    ) -> Result<(), Error> {
        if let Some(outgoing_dir) = outgoing_dir {
            // Add `ComponentRunner` to the exposed services of the container, and then serve the
            // outgoing directory.
            let mut fs = ServiceFs::new_local();
            fs.dir("svc")
                .add_fidl_service(ExposedServices::ComponentRunner)
                .add_fidl_service(ExposedServices::ContainerController)
                .add_fidl_service(ExposedServices::GraphicalPresenter)
                .add_fidl_service(ExposedServices::LutexController)
                .add_fidl_service(ExposedServices::ContainerInfo);

            // Expose the root of the container's filesystem.
            let (fs_root, fs_root_server_end) = fidl::endpoints::create_proxy();
            fs.add_remote("fs_root", fs_root);
            expose_root(self.system_task(), fs_root_server_end)?;

            fs.serve_connection(outgoing_dir.into()).map_err(|_| errno!(EINVAL))?;

            fs.for_each_concurrent(None, |request_stream| async {
                match request_stream {
                    ExposedServices::ComponentRunner(request_stream) => {
                        match serve_component_runner(request_stream, self.system_task()).await {
                            Ok(_) => {}
                            Err(e) => {
                                log_error!("Error serving component runner: {:?}", e);
                            }
                        }
                    }
                    ExposedServices::ContainerController(request_stream) => {
                        serve_container_controller(request_stream, self.system_task())
                            .await
                            .expect("failed to start container.")
                    }
                    ExposedServices::GraphicalPresenter(request_stream) => {
                        serve_graphical_presenter(request_stream, &self.kernel)
                            .await
                            .expect("failed to start GraphicalPresenter.")
                    }
                    ExposedServices::LutexController(request_stream) => {
                        serve_lutex_controller(request_stream, self.system_task())
                            .await
                            .expect("failed to start LutexController.")
                    }
                    ExposedServices::ContainerInfo(request_stream) => {
                        serve_container_info(request_stream)
                            .await
                            .expect("failed to start ContainerInfo.")
                    }
                }
            })
            .await
        }
        Ok(())
    }

    pub async fn serve(&self, service_config: ContainerServiceConfig) -> Result<(), Error> {
        let (r, _) = futures::join!(
            self.serve_outgoing_directory(service_config.start_info.outgoing_dir),
            server_component_controller(
                self.kernel.clone(),
                service_config.request_stream,
                service_config.receiver
            )
        );
        r
    }

    pub fn new_memory_attribution_observer(
        &self,
        control_handle: fattribution::ProviderControlHandle,
    ) -> attribution_server::Observer {
        self.memory_attribution_manager.new_observer(control_handle)
    }
}

/// The services that are exposed in the container component's outgoing directory.
enum ExposedServices {
    ComponentRunner(frunner::ComponentRunnerRequestStream),
    ContainerController(fstarcontainer::ControllerRequestStream),
    GraphicalPresenter(felement::GraphicalPresenterRequestStream),
    LutexController(fbinder::LutexControllerRequestStream),
    ContainerInfo(fstarcontainer::InfoRequestStream),
}

type TaskResult = Result<ExitStatus, Error>;

async fn server_component_controller(
    kernel: Arc<Kernel>,
    request_stream: frunner::ComponentControllerRequestStream,
    task_complete: oneshot::Receiver<TaskResult>,
) {
    *kernel.container_control_handle.lock() = Some(request_stream.control_handle());

    enum Event<T, U> {
        Controller(T),
        Completion(U),
    }

    let mut stream = futures::stream::select(
        request_stream.map(Event::Controller),
        task_complete.into_stream().map(Event::Completion),
    );

    while let Some(event) = stream.next().await {
        match event {
            Event::Controller(Ok(frunner::ComponentControllerRequest::Stop { .. })) => {
                log_info!("Stopping the container.");
            }
            Event::Controller(Ok(frunner::ComponentControllerRequest::Kill { control_handle })) => {
                log_info!("Killing the container's job.");
                control_handle.shutdown_with_epitaph(zx::Status::err_from_raw(
                    fcomponent::Error::InstanceDied.into_primitive() as i32,
                ));
                fruntime::job_default().kill().expect("Failed to kill job");
            }
            Event::Controller(Ok(frunner::ComponentControllerRequest::_UnknownMethod {
                ordinal,
                method_type,
                ..
            })) => {
                log_error!(ordinal, method_type:?; "Unknown component controller request received.");
            }
            Event::Controller(Err(e)) => {
                log_warn!(e:?; "Container component controller channel encountered an error.");
            }
            Event::Completion(result) => {
                log_info!(result:?; "init process exited.");
            }
        }

        // We treat any event in the stream as an invitation to shut down.
        if !kernel.is_shutting_down() {
            kernel.shut_down();
        }
    }

    log_debug!("done listening for container-terminating events");

    // In case the stream ended without an event, shut down the kernel here.
    if !kernel.is_shutting_down() {
        kernel.shut_down();
    }
}

pub async fn create_component_from_stream(
    mut request_stream: frunner::ComponentRunnerRequestStream,
    kernel_extra_features: Vec<String>,
) -> Result<(Container, ContainerServiceConfig), Error> {
    if let Some(event) = request_stream.try_next().await? {
        match event {
            frunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                let request_stream = controller.into_stream();
                let mut start_info = ContainerStartInfo::new(start_info)?;
                let (sender, receiver) = oneshot::channel::<TaskResult>();
                let container = create_container(&mut start_info, &kernel_extra_features, sender)
                    .await
                    .with_source_context(|| {
                        format!("creating container \"{}\"", start_info.program.name)
                    })?;
                let service_config =
                    ContainerServiceConfig { start_info, request_stream, receiver };
                return Ok((container, service_config));
            }
            frunner::ComponentRunnerRequest::_UnknownMethod { ordinal, .. } => {
                log_warn!("Unknown ComponentRunner request: {ordinal}");
            }
        }
    }
    bail!("did not receive Start request");
}

async fn get_bootargs(device_tree: &Devicetree) -> Result<String, Error> {
    device_tree
        .root_node
        .find("chosen")
        .and_then(|n| {
            n.get_property("bootargs").map(|p| {
                let end =
                    if p.value.last() == Some(&0) { p.value.len() - 1 } else { p.value.len() };
                match std::str::from_utf8(&p.value[..end]) {
                    Ok(s) => Ok(s.to_owned()),
                    Err(e) => {
                        log_warn!("Bootargs are not valid UTF-8: {e}");
                        Err(anyhow!("Bootargs are not valid UTF-8"))
                    }
                }
            })
        })
        .context("Couldn't find bootargs")?
}

async fn get_bootitems() -> Result<std::vec::Vec<u8>, Error> {
    let items =
        connect_to_protocol::<fboot::ItemsMarker>().context("Failed to connect to boot items")?;

    let items_response = items
        .get2(zbi::Type::Devicetree.into(), None)
        .await
        .context("FIDL: Failed to get devicetree item")?
        .map_err(|e| anyhow!("Failed to get devicetree item {:?}", e))?;

    let Some(item) = items_response.last() else {
        return Err(anyhow!("Failed to get items"));
    };

    let devicetree_vmo = &item.payload;
    let bytes = devicetree_vmo
        .read_to_vec(0, item.length as u64)
        .context("Failed to read devicetree vmo")?;

    Ok(bytes)
}

async fn get_serial_number() -> Result<String, Error> {
    let sysinfo = connect_to_protocol::<fsysinfo::SysInfoMarker>()
        .context("Failed to connect to fuchsia.sysinfo.SysInfo")?;
    sysinfo
        .get_serial_number()
        .await
        .context("FIDL: Failed to get serial number")?
        .map_err(|status| anyhow!("Failed to get serial number: {:?}", status))
}

async fn create_container(
    start_info: &mut ContainerStartInfo,
    kernel_extra_features: &[String],
    task_complete: oneshot::Sender<TaskResult>,
) -> Result<Container, Error> {
    fuchsia_trace::duration!(CATEGORY_STARNIX, NAME_CREATE_CONTAINER);
    const DEFAULT_INIT: &str = "/container/init";

    let pkg_channel = start_info.container_namespace.get_namespace_channel("/pkg").unwrap();
    let pkg_dir_proxy = fio::DirectorySynchronousProxy::new(pkg_channel);

    let device_tree: Option<Devicetree> = match get_bootitems().await {
        Ok(items) => match parse_devicetree(&items) {
            Ok(device_tree) => Some(device_tree),
            Err(e) => {
                log_warn!("Failed to parse devicetree: {e:?}");
                None
            }
        },
        Err(e) => {
            log_warn!("Failed to get boot items for devicetree: {e:?}");
            None
        }
    };
    let mut features = parse_features(&start_info, kernel_extra_features)?;

    log_debug!("Creating container with {:#?}", features);
    let mut kernel_cmdline = BString::from(start_info.program.kernel_cmdline.as_bytes());
    let mut android_provided_bootreason = None;

    let mut bootargs_has_serialno = false;
    let mut bootargs_has_verifiedbootstate = false;
    // TODO(https://fxbug.dev/526770691): Newer versions of Android replace the 'androidboot.*'
    // kernel parameters with 'bootconfig'. We should expose this to the container to avoid having
    // to manage 'androidboot.*' parameters here.
    if features.android_serialno {
        if let Some(device_tree) = &device_tree {
            match get_bootargs(device_tree).await {
                Ok(args) => {
                    for item in parse_cmdline(&args) {
                        if item.starts_with("androidboot.force_normal_boot") {
                            // TODO(https://fxbug.dev/424152964): Support force_normal_boot.
                            continue;
                        }
                        if item.starts_with("androidboot.serialno") {
                            bootargs_has_serialno = true;
                        }
                        if item.starts_with("androidboot.verifiedbootstate") {
                            bootargs_has_verifiedbootstate = true;
                        }
                        if item.starts_with("androidboot.bootreason") && features.android_bootreason
                        {
                            // androidboot.bootreason is sourced from the Fuchsia reboot reason.
                            // It is still useful to log it from userspace to learn what the
                            // possible values are.
                            log_info!("Original devicetree bootarg {:?}", item);
                            if let Some((_, v)) = item.split_once('=') {
                                android_provided_bootreason = Some(v.to_string());
                            }
                            continue;
                        }
                        kernel_cmdline.extend(b" ");
                        kernel_cmdline.extend(item.bytes());
                    }
                }
                Err(err) => log_warn!("could not get bootargs: {err:?}"),
            }
        } else {
            log_warn!("No devicetree available to get bootargs for android.serialno");
        }

        if !bootargs_has_serialno {
            match get_serial_number().await {
                Ok(serial) => {
                    log_info!("Fell back to sysinfo serial number: {}", serial);
                    kernel_cmdline.extend(b" androidboot.serialno=");
                    kernel_cmdline.extend(serial.bytes());
                }
                Err(err) => {
                    log_warn!("Could not get serial number from sysinfo: {err:?}");
                }
            }
        }
        if !bootargs_has_verifiedbootstate {
            kernel_cmdline.extend(b" androidboot.verifiedbootstate=orange");
        }
    }
    if features.android_bootreason {
        kernel_cmdline.extend(b" androidboot.bootreason=");

        let tmp_channel = start_info.container_namespace.get_namespace_channel("/tmp_lifecycle");
        let tmp_proxy = match tmp_channel {
            Ok(channel) => {
                Some(fio::DirectoryProxy::new(fidl::AsyncChannel::from_channel(channel)))
            }
            _ => None,
        };

        match get_or_init_android_bootreason(tmp_proxy, android_provided_bootreason).await {
            Ok(reason) => {
                kernel_cmdline.extend(reason.bytes());
            }
            Err(err) => {
                log_warn!("could not get android bootreason: {err:?}. falling back to 'unknown'");
                kernel_cmdline.extend(b"unknown");
            }
        }
    }
    if let Some(supported_vendors) = &features.magma_supported_vendors {
        kernel_cmdline.extend(b" ");
        let params = get_magma_params(supported_vendors);
        kernel_cmdline.extend(&*params);
    }

    // Check whether we actually have access to a role manager by trying to set our own
    // thread's role.
    let mut task_mappings = RoleOverrides::new();
    for m in &start_info.program.task_role_overrides {
        task_mappings.add(m.process.clone(), m.thread.clone(), m.cgroup.clone(), m.role.clone());
    }
    let task_mappings = task_mappings.build().context("adding custom task role")?;
    let scheduler_manager = SchedulerManager::new(task_mappings);

    let crash_reporter = connect_to_protocol::<CrashReporterMarker>().unwrap();

    let node = inspect::component::inspector().root().create_child("container");
    let kernel_node = node.create_child("kernel");
    kernel_node.record_int("created_at", zx::MonotonicInstant::get().into_nanos());
    features.record_inspect(&kernel_node);

    let security_state = security::kernel_init_security(
        features.selinux.enabled,
        features.selinux.options.clone(),
        features.selinux.exceptions.clone(),
        &kernel_node,
    );

    // `config.enable_utc_time_adjustment` is set through config capability
    // `fuchsia.time.config.WritableUTCTime`.
    let time_adjustment_proxy = if features.enable_utc_time_adjustment {
        connect_to_protocol_sync::<AdjustMarker>()
            .map_err(|e| log_error!("could not connect to fuchsia.time.external/Adjust: {:?}", e))
            .ok()
    } else {
        // See the comment above. UTC adjustment is a per-product setting.
        log_info!("UTC adjustment is forbidden.");
        None
    };

    log_info!("final kernel cmdline: {kernel_cmdline:?}");
    kernel_node.record_string("cmdline", kernel_cmdline.to_str_lossy());

    let kernel = Kernel::new(
        kernel_cmdline,
        features.kernel.clone(),
        std::mem::take(&mut features.system_limits),
        start_info.container_namespace.try_clone()?,
        scheduler_manager,
        Some(crash_reporter),
        kernel_node,
        security_state,
        time_adjustment_proxy,
        device_tree,
    )
    .with_source_context(|| format!("creating Kernel: {}", start_info.program.name))?;
    let (fs_context, feature_mounts) =
        create_fs_context(&kernel, &features, start_info, &pkg_dir_proxy)
            .source_context("creating FsContext")?;
    let init_pid = kernel.pids.lock().allocate_pid().source_context("allocating init pid")?;
    let init_tid = init_pid.id;
    // Lots of software assumes that the pid for the init process is 1.
    debug_assert_eq!(init_tid, 1);

    let system_task = create_system_task(&kernel, Arc::clone(&fs_context))
        .source_context("create system task")?;
    // The system task gives pid 2. This value is less critical than giving
    // pid 1 to init, but this value matches what is supposed to happen.
    debug_assert_eq!(system_task.tid.id, 2);

    feature_mounts(&system_task).source_context("mounting feature filesystems")?;

    kernel.kthreads.init(system_task).source_context("initializing kthreads")?;
    let system_task = kernel.kthreads.system_task();

    kernel.syslog.init(&kernel).source_context("initializing syslog")?;

    kernel.hrtimer_manager.init(system_task).source_context("initializing HrTimer manager")?;

    log_info!("Initializing suspend resume manager.");
    if let Err(e) = kernel.suspend_resume_manager.init(&system_task) {
        log_warn!("Suspend/Resume manager initialization failed: ({e:?})");
    }

    // Real Time clock is present in all configuration.
    log_info!("Initializing RTC device.");
    rtc_device_init(&system_task).context("in starnix_kernel_runner, while initializing RTC")?;

    // Register common devices and add them in sysfs and devtmpfs.
    log_info!("Registering devices and filesystems.");
    init_common_devices(&kernel)?;
    register_common_file_systems(&kernel);

    register_common_syscalls(&kernel);

    log_info!("Mounting filesystems.");
    mount_filesystems(&system_task, start_info, &pkg_dir_proxy)
        .source_context("mounting filesystems")?;

    // Run all common features that were specified in the .cml.
    {
        log_info!("Running container features.");
        run_container_features(&kernel, &features)?;
    }

    log_info!("Initializing remote block devices.");
    init_remote_block_devices(&kernel).source_context("initalizing remote block devices")?;

    // If there is an init binary path, run it, optionally waiting for the
    // startup_file_path to be created. The task struct is still used
    // to initialize the system up until this point, regardless of whether
    // or not there is an actual init to be run.
    let argv = if start_info.program.init.is_empty() {
        vec![DEFAULT_INIT.to_string()]
    } else {
        start_info.program.init.clone()
    }
    .iter()
    .map(|s| to_cstr(s))
    .collect::<Vec<_>>();

    log_info!("Opening start_info file.");
    let executable = system_task
        .open_file_for_exec(
            starnix_core::vfs::FdNumber::AT_FDCWD,
            argv[0].as_bytes().into(),
            OpenFlags::empty(),
        )
        .with_source_context(|| format!("opening init: {:?}", argv[0]))?;

    let initial_name = if start_info.program.init.is_empty() {
        TaskCommand::default()
    } else {
        TaskCommand::new(start_info.program.init[0].as_bytes())
    };

    let rlimits = parse_rlimits(&start_info.program.rlimits)?;

    // Serve the runtime directory.
    log_info!("Starting runtime directory.");
    if let Some(runtime_dir) = start_info.runtime_dir.take() {
        kernel.kthreads.spawn_future(
            move || async move { serve_runtime_dir(runtime_dir).await },
            "serve_runtime_dir",
        );
    }

    // At this point the runtime environment has been prepared but nothing is actually running yet.
    // Pause here if a debugger needs time to attach to the job.
    if let Some(break_on_start) = start_info.break_on_start.take() {
        log_info!("Waiting for signal from debugger before spawning init process...");
        if let Err(e) =
            fuchsia_async::OnSignals::new(break_on_start, zx::Signals::EVENTPAIR_PEER_CLOSED).await
        {
            log_warn!(e:%; "Received break_on_start eventpair but couldn't wait for PEER_CLOSED.");
        }
    }

    log_info!("Creating init process.");
    let init_task =
        create_init_process(&kernel, init_pid, initial_name, Arc::clone(&fs_context), &rlimits)
            .with_source_context(|| format!("creating init task: {:?}", start_info.program.init))?;

    execute_task_with_prerun_result(
        init_task,
        move |init_task| {
            parse_numbered_handles(init_task, None, &init_task.files()).expect("");
            init_task.exec(executable, argv[0].clone(), argv.clone(), vec![])
        },
        move |result| {
            log_info!("Finished running init process: {:?}", result);
            let _ = task_complete.send(result);
        },
        None,
    )?;

    if !start_info.program.startup_file_path.is_empty() {
        wait_for_init_file(&start_info.program.startup_file_path, &system_task, init_tid).await?;
    };

    let memory_attribution_manager = ContainerMemoryAttributionManager::new(
        Arc::downgrade(&kernel),
        start_info.component_instance.take().ok_or_else(|| Error::msg("No component instance"))?,
    );

    Ok(Container {
        kernel,
        memory_attribution_manager,
        _node: node,
        _thread_bound: Default::default(),
    })
}

fn create_fs_context(
    kernel: &Kernel,
    features: &Features,
    start_info: &ContainerStartInfo,
    pkg_dir_proxy: &fio::DirectorySynchronousProxy,
) -> Result<(Arc<FsContext>, LayeredFsMounts), Error> {
    // The mounts are applied in the order listed. Mounting will fail if the designated mount
    // point doesn't exist in a previous mount. The root must be first so other mounts can be
    // applied on top of it.
    let mut mounts_iter =
        start_info.program.mounts.iter().chain(start_info.config.additional_mounts.iter());
    let root = MountAction::new_for_root(
        kernel,
        pkg_dir_proxy,
        mounts_iter.next().ok_or_else(|| anyhow!("Mounts list is empty"))?,
    )?;
    if root.path != "/" {
        anyhow::bail!("First mount in mounts list is not the root");
    }

    let mut builder = LayeredFsBuilder::new(root.fs);
    if features.container {
        // /container/component will be a tmpfs where component using the starnix kernel will have their
        // package mounted.
        let component_tmpfs_options = FileSystemOptions {
            params: kernel
                .features
                .ns_mount_options("#component_tmpfs")
                .context("#component_tmpfs options")?,
            ..Default::default()
        };
        let component_tmpfs = TmpFs::new_fs_with_options(kernel, component_tmpfs_options)?;

        // /container will mount the container pkg
        let container_remotefs_options = FileSystemOptions {
            source: "data".into(),
            params: kernel.features.ns_mount_options("#container").context("#container options")?,
            ..Default::default()
        };
        let container_remotefs = new_remotefs_in_root(
            kernel,
            pkg_dir_proxy,
            container_remotefs_options,
            fio::PERM_READABLE | fio::PERM_EXECUTABLE,
        )?;

        builder.add("/container", container_remotefs);
        builder.add("/container/component", component_tmpfs);
    }
    if features.custom_artifacts {
        let mount_options = FileSystemOptions {
            params: kernel
                .features
                .ns_mount_options("#custom_artifacts")
                .context("#custom_artifacts options")?,
            ..Default::default()
        };
        let fs = TmpFs::new_fs_with_options(kernel, mount_options)?;
        builder.add("/custom_artifacts", fs);
    }
    if features.test_data {
        let mount_options = FileSystemOptions {
            params: kernel.features.ns_mount_options("#test_data").context("#test_data options")?,
            ..Default::default()
        };
        let fs = TmpFs::new_fs_with_options(kernel, mount_options)?;
        builder.add("/test_data", fs);
    }

    let (mut root_fs, feature_mounts) = builder.build(kernel);
    if features.rootfs_rw {
        root_fs = OverlayStack::wrap_fs_in_writable_layer(kernel, root_fs)?;
    }

    Ok((FsContext::new(Namespace::new_with_flags(root_fs, root.flags)), feature_mounts))
}

fn parse_rlimits(rlimits: &[String]) -> Result<Vec<(Resource, u64)>, Error> {
    let mut res = Vec::new();

    for rlimit in rlimits {
        let (key, value) =
            rlimit.split_once('=').ok_or_else(|| anyhow!("Invalid rlimit: {rlimit}"))?;
        let value = value.parse::<u64>()?;
        let kv = match key {
            "RLIMIT_NOFILE" => (Resource::NOFILE, value),
            "RLIMIT_RTPRIO" => (Resource::RTPRIO, value),
            _ => bail!("Unknown rlimit: {key}"),
        };
        res.push(kv);
    }

    Ok(res)
}

fn mount_filesystems(
    system_task: &CurrentTask,
    start_info: &ContainerStartInfo,
    pkg_dir_proxy: &fio::DirectorySynchronousProxy,
) -> Result<(), Error> {
    // Skip the first mount, that was used to create the root filesystem.
    let mut mounts_iter =
        start_info.program.mounts.iter().chain(start_info.config.additional_mounts.iter());
    let _ = mounts_iter.next();
    for mount_spec in mounts_iter {
        let action = MountAction::from_spec(system_task, pkg_dir_proxy, mount_spec)
            .with_source_context(|| format!("creating filesystem from spec: {}", mount_spec))?;
        let mount_point = system_task
            .lookup_path_from_root(action.path.as_ref())
            .with_source_context(|| format!("lookup path from root: {}", action.path))?;
        mount_point.mount(WhatToMount::Fs(action.fs), action.flags)?;
    }
    Ok(())
}

fn init_remote_block_devices(kernel: &Kernel) -> Result<(), Error> {
    remote_block_device_init(kernel);
    let entries = match std::fs::read_dir("/block") {
        Ok(entries) => entries,
        Err(e) => {
            log_warn!("Failed to read block directory: {}", e);
            return Ok(());
        }
    };
    for entry in entries {
        let entry = entry?;
        let path_buf = entry.path();
        let path = path_buf.to_str().ok_or_else(|| anyhow!("Invalid block device path"))?;
        let (client_end, server_end) = fidl::endpoints::create_endpoints();
        match fdio::service_connect(
            &format!("{}/fuchsia.storage.block.Block", path),
            server_end.into(),
        ) {
            Ok(()) => (),
            Err(e) => {
                log_warn!("Failed to connect to block device at {}: {}", path, e);
                continue;
            }
        }
        let name = entry.file_name();
        let name_str = name.to_str().unwrap();
        kernel
            .remote_block_device_registry
            .create_remote_block_device(kernel, &name_str, client_end)
            .with_source_context(|| format!("creating remote block device: {name_str}"))?;
    }
    Ok(())
}

async fn wait_for_init_file(
    startup_file_path: &str,
    current_task: &CurrentTask,
    init_tid: tid_t,
) -> Result<(), Error> {
    // TODO(https://fxbug.dev/42178400): Use inotify machinery to wait for the file.
    loop {
        fasync::Timer::new(fasync::MonotonicDuration::from_millis(100).after_now()).await;

        let creds = security::creds_start_internal_operation(current_task);
        if let Some(result) = current_task.override_creds(creds, || {
            let root = current_task.fs().root();
            let mut context = LookupContext::default();

            match current_task.lookup_path(&mut context, root, startup_file_path.into()) {
                Ok(_) => return Some(Ok(())),
                Err(error) if error == ENOENT => {}
                Err(error) => return Some(Err(anyhow::Error::from(error))),
            };

            if current_task.get_task(init_tid).is_err() {
                return Some(Err(anyhow!(
                    "Init task terminated before startup_file_path was ready"
                )));
            }

            None
        }) {
            return result;
        }
    }
}

async fn serve_runtime_dir(runtime_dir: ServerEnd<fio::DirectoryMarker>) {
    let mut fs = fuchsia_component::server::ServiceFs::new();
    match create_job_id_vmo() {
        Ok(vmo) => {
            fs.dir("elf").add_vmo_file_at("job_id", vmo);
        }
        Err(e) => log_error!(e:%; "failed to create vmo with job id for debuggers"),
    }
    match fs.serve_connection(runtime_dir) {
        Ok(_) => {
            fs.add_fidl_service(|job_requests: TaskProviderRequestStream| {
                fuchsia_async::Task::local(async move {
                    if let Err(e) = serve_task_provider(job_requests).await {
                        log_warn!(e:?; "Error serving TaskProvider");
                    }
                })
                .detach();
            });
            fs.collect::<()>().await;
        }
        Err(e) => log_error!("Couldn't serve runtime directory: {e:?}"),
    }
}

fn create_job_id_vmo() -> Result<zx::Vmo, Error> {
    let job_id = fuchsia_runtime::job_default().koid().context("reading own job koid")?;
    let job_id_str = job_id.raw_koid().to_string();
    let job_id_vmo = zx::Vmo::create(job_id_str.len() as u64).context("creating job id vmo")?;
    job_id_vmo.write(job_id_str.as_bytes(), 0).context("write job id to vmo")?;
    Ok(job_id_vmo)
}

async fn serve_task_provider(mut job_requests: TaskProviderRequestStream) -> Result<(), Error> {
    while let Some(request) = job_requests.next().await {
        match request.context("getting next TaskProvider request")? {
            TaskProviderRequest::GetJob { responder } => {
                responder
                    .send(
                        fuchsia_runtime::job_default()
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .map_err(|s| s.into_raw()),
                    )
                    .context("sending job for runtime dir")?;
            }
            unknown => bail!("Unknown TaskProvider method {unknown:?}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::wait_for_init_file;

    use futures::{SinkExt, StreamExt};
    use starnix_core::testing::spawn_kernel_and_run;
    use starnix_core::vfs::FdNumber;
    use starnix_uapi::CLONE_FS;
    use starnix_uapi::file_mode::FileMode;
    use starnix_uapi::open_flags::OpenFlags;
    use starnix_uapi::signals::SIGCHLD;
    use starnix_uapi::vfs::ResolveFlags;

    #[fuchsia::test]
    async fn test_init_file_already_exists() {
        spawn_kernel_and_run(async move |current_task| {
            let path = "/path";
            current_task
                .open_file_at(
                    FdNumber::AT_FDCWD,
                    path.into(),
                    OpenFlags::CREAT,
                    FileMode::default(),
                    ResolveFlags::empty(),
                )
                .expect("Failed to create file");

            wait_for_init_file(path, current_task, current_task.get_tid())
                .await
                .expect("failed to wait for file");
        })
        .await;
    }
    #[fuchsia::test]
    async fn test_init_file_wait_required() {
        spawn_kernel_and_run(async move |current_task| {
            let (mut sender, mut receiver) = futures::channel::mpsc::unbounded();

            let init_task = current_task.clone_task_for_test(CLONE_FS as u64, Some(SIGCHLD));
            let path = "/path";

            let test_init_tid = current_task.get_tid();

            let wait_fut = async {
                sender.send(()).await.expect("failed to send message");
                wait_for_init_file(path, &init_task, test_init_tid)
                    .await
                    .expect("failed to wait for file");
                sender.send(()).await.expect("failed to send message");
            };

            let create_fut = async {
                assert!(receiver.next().await.is_some());
                current_task
                    .open_file_at(
                        FdNumber::AT_FDCWD,
                        path.into(),
                        OpenFlags::CREAT,
                        FileMode::default(),
                        ResolveFlags::empty(),
                    )
                    .expect("Failed to create file");
                assert!(receiver.next().await.is_some());
            };

            futures::join!(wait_fut, create_fut);
        })
        .await;
    }
    #[fuchsia::test]
    async fn test_init_exits_before_file_exists() {
        spawn_kernel_and_run(async move |current_task| {
            let (mut sender, mut receiver) = futures::channel::mpsc::unbounded();

            let init_task = current_task.clone_task_for_test(CLONE_FS as u64, Some(SIGCHLD));
            const STARTUP_FILE_PATH: &str = "/path";

            let test_init_tid = init_task.get_tid();

            let wait_fut = async {
                sender.send(()).await.expect("failed to send message");
                wait_for_init_file(STARTUP_FILE_PATH, current_task, test_init_tid)
                    .await
                    .expect_err("Did not detect init exit");
                sender.send(()).await.expect("failed to send message");
            };

            let exit_fut = async {
                assert!(receiver.next().await.is_some());
                std::mem::drop(init_task);
                assert!(receiver.next().await.is_some());
            };

            futures::join!(wait_fut, exit_fut);
        })
        .await;
    }
}
