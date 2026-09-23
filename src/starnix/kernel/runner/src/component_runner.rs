// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{MountAction, run_component_features};
use anyhow::{Context, Error, anyhow, bail};
use fidl::AsyncChannel;
use fidl::endpoints::{RequestStream, ServerEnd};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_runner::{
    ComponentControllerMarker, ComponentControllerRequest, ComponentControllerRequestStream,
    ComponentStartInfo,
};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fprocess;
use fuchsia_async as fasync;
use fuchsia_runtime::{HandleInfo, HandleType};
use futures::channel::oneshot;
use futures::{FutureExt, StreamExt};
use rand::distr::Alphanumeric;
use rand::{RngExt as _, rng};
use serde::Deserialize;
use serde::de::Error as _;
use starnix_core::execution::{create_init_child_process, execute_task_with_prerun_result};
use starnix_core::fs::fuchsia::{RemoteFs, SyslogFile, create_file_from_handle};
use starnix_core::task::{CurrentTask, ExitStatus, Task};
use starnix_core::vfs::fs_args::MountParams;
use starnix_core::vfs::{
    FdNumber, FdTable, FileSystemOptions, FsString, LookupContext, NamespaceNode, WhatToMount,
};
use starnix_core::{security, signals};
use starnix_logging::{log_debug, log_error, log_info, log_warn};
use starnix_sync::{ComponentMountRecordLock, LockDepMutex};
use starnix_task_command::TaskCommand;
use starnix_uapi::auth::{Capabilities, Credentials};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errno;
use starnix_uapi::errors::{EEXIST, ENOTDIR, Errno};
use starnix_uapi::file_mode::mode;
use starnix_uapi::mount_flags::{MountFlags, MountpointFlags};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::signals::{SIGINT, SIGKILL};
use starnix_uapi::unmount_flags::UnmountFlags;
use std::ffi::CString;

use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::{Arc, Weak};

/// Component controller epitaph value used as the base value to pass non-zero error
/// codes to the calling component.
///
/// TODO(https://fxbug.dev/42081234): Cleanup this once we have a proper mechanism to
/// get Linux exit code from component runner.
const COMPONENT_EXIT_CODE_BASE: i32 = 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComponentProgram {
    binary: CString,

    #[serde(default)]
    args: Vec<String>,

    #[serde(default)]
    environ: Vec<String>,

    #[serde(default)]
    cwd: Option<String>,

    #[serde(default)]
    uid: Option<runner::serde::StoreAsString<u32>>,

    #[serde(default)]
    component_mounts: Vec<String>,

    #[serde(default)]
    features: Vec<String>,

    #[serde(default, deserialize_with = "parse_capabilities")]
    capabilities: Option<Capabilities>,

    #[serde(default)]
    seclabel: Option<CString>,

    #[serde(default, rename(deserialize = "test_target_kernel"))]
    _test_target_kernel: Option<String>,
}

impl ComponentProgram {
    fn resolve_templates(&mut self, component_path: &str, pkg_path: &str) {
        let resolve_template = |values: &mut Vec<String>| {
            for val in values {
                *val = val
                    .replace("{pkg_path}", &pkg_path)
                    .replace("{component_path}", &component_path);
            }
        };

        resolve_template(&mut self.args);
        resolve_template(&mut self.environ);
    }
}

fn parse_capabilities<'de, D>(deserializer: D) -> Result<Option<Capabilities>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut capabilities = Capabilities::empty();
    for cap in Vec::<String>::deserialize(deserializer)? {
        capabilities |= cap.parse().map_err(D::Error::custom)?;
    }
    Ok(Some(capabilities))
}

/// Starts a component inside the given container.
///
/// The component's `binary` can either:
///   - an absolute path, in which case the path is treated as a path into the root filesystem that
///     is mounted by the container's configuration
///   - relative path, in which case the binary is read from the component's package (which is
///     mounted at /container/component/{random}/pkg.)
///
/// The directories in the component's namespace are mounted at /container/component/{random}.
pub async fn start_component(
    mut start_info: ComponentStartInfo,
    controller: ServerEnd<ComponentControllerMarker>,
    system_task: &CurrentTask,
) -> Result<(), Error> {
    let url = start_info.resolved_url.clone().unwrap_or_else(|| "<unknown>".to_string());

    let (task_complete_sender, task_complete) = oneshot::channel::<TaskResult>();

    let weak_task = system_task.override_creds(
        security::creds_start_internal_operation(system_task),
        || {
            // TODO(https://fxbug.dev/42076551): We leak the directory created by this function.
            let component_path = generate_component_path(system_task)?;
            let pkg_path = format!("{component_path}/pkg");

            let mount_record =
                Arc::new(LockDepMutex::<_, ComponentMountRecordLock>::new(MountRecord::default()));

            let ns = start_info.ns.take().ok_or_else(|| anyhow!("Missing namespace"))?;

            let program = start_info.program.as_ref().context("reading program block")?;
            let mut program: ComponentProgram =
                runner::serde::deserialize_program(program).context("parsing program block")?;
            program.resolve_templates(&component_path, &pkg_path);
            log_debug!("start_component: {}\n{:#?}", url, program);

            let ns_mount_options = system_task.kernel().features.default_ns_mount_options.as_ref();
            let mut maybe_pkg = None;
            let mut maybe_svc = None;
            for entry in ns {
                if let (Some(dir_path), Some(dir_handle)) = (entry.path, entry.directory) {
                    let dir_path_str = dir_path.as_str();
                    let mount_options = ns_mount_options
                        .and_then(|mount_options| mount_options.get(dir_path_str).cloned());

                    match dir_path_str {
                        "/svc" => {
                            maybe_svc = Some(fio::DirectoryProxy::new(AsyncChannel::from_channel(
                                dir_handle.into_channel(),
                            )));
                        }
                        "/custom_artifacts" => {
                            // Mount custom_artifacts directory at root of container
                            // We may want to transition to have this directory unique per component
                            let dir_proxy =
                                fio::DirectorySynchronousProxy::new(dir_handle.into_channel());
                            mount_record
                                .lock()
                                .mount_remote(
                                    system_task,
                                    &dir_proxy,
                                    &dir_path,
                                    mount_options.as_ref(),
                                )
                                .with_context(|| {
                                    format!("failed to mount_remote on path {}", dir_path)
                                })?;
                        }
                        _ => {
                            let dir_proxy =
                                fio::DirectorySynchronousProxy::new(dir_handle.into_channel());
                            mount_record
                                .lock()
                                .mount_remote(
                                    system_task,
                                    &dir_proxy,
                                    &format!("{component_path}/{dir_path}"),
                                    mount_options.as_ref(),
                                )
                                .with_context(|| {
                                    format!(
                                        "failed to mount_remote on path {component_path}/{dir_path}"
                                    )
                                })?;
                            if dir_path == "/pkg" {
                                maybe_pkg = Some(dir_proxy);
                            }
                        }
                    }
                }
            }

            let pkg = maybe_pkg.ok_or_else(|| anyhow!("Missing /pkg entry in namespace"))?;

            let uid = program
                .uid
                .map(|uid| uid.0)
                .unwrap_or_else(|| system_task.kernel().features.default_uid);

            let mut credentials = Credentials::with_ids(uid, uid);
            if let Some(capabilities) = program.capabilities {
                credentials.cap_permitted = capabilities;
                credentials.cap_effective = capabilities;
                credentials.cap_inheritable = capabilities;
                credentials.cap_ambient = capabilities;
            }

            run_component_features(system_task.kernel(), &program.features, maybe_svc)
                .unwrap_or_else(|e| {
                    log_error!("failed to set component features for {} - {:?}", url, e);
                });

            let current_task = create_init_child_process(
                system_task.kernel(),
                TaskCommand::new(program.binary.as_bytes()),
                credentials,
                program.seclabel.as_ref(),
            )?;

            execute_task_with_prerun_result(
                current_task,
                {
                    let mount_record = mount_record.clone();
                    move |current_task| {
                        let cwd_path = FsString::from(program.cwd.unwrap_or(pkg_path));
                        let cwd = current_task.lookup_path(
                            &mut LookupContext::default(),
                            current_task.fs().root(),
                            cwd_path.as_ref(),
                        )?;
                        current_task.fs().chdir(current_task, cwd)?;

                        for mount in &program.component_mounts {
                            let action = MountAction::from_spec(current_task, &pkg, mount)
                                .map_err(|e| {
                                    log_error!("Error while mounting the filesystems: {e:?}");
                                    errno!(EINVAL)
                                })?;
                            let mount_point =
                                current_task.lookup_path_from_root(action.path.as_ref())?;
                            mount_record.lock().mount(
                                mount_point,
                                WhatToMount::Fs(action.fs),
                                action.flags,
                            )?;
                        }

                        let files = current_task.files();
                        parse_numbered_handles(current_task, start_info.numbered_handles, &files)
                            .map_err(|e| {
                            log_error!("Error while parsing the numbered handles: {e:?}");
                            errno!(EINVAL)
                        })?;

                        let mut argv = vec![program.binary.clone()];
                        for arg in program.args {
                            argv.push(CString::new(arg).map_err(|_| errno!(EINVAL))?);
                        }

                        let mut environ = vec![];
                        for env in program.environ {
                            environ.push(CString::new(env).map_err(|_| errno!(EINVAL))?);
                        }

                        let executable = current_task.open_file_for_exec(
                            FdNumber::AT_FDCWD,
                            program.binary.as_bytes().into(),
                            OpenFlags::empty(),
                        )?;
                        current_task.exec(executable, program.binary, argv, environ)?;

                        Ok(Arc::downgrade(&current_task.task))
                    }
                },
                move |result| {
                    // Unmount all the directories for this component.
                    std::mem::drop(mount_record);

                    // If the component controller server has gone away, there is nobody for us to
                    // report the result to.
                    let _ = task_complete_sender.send(result);
                },
                None,
            )
            .map_err(anyhow::Error::from)
        },
    )?;

    let controller = controller.into_stream();
    fasync::Task::local(serve_component_controller(controller, weak_task, task_complete)).detach();

    Ok(())
}

type TaskResult = Result<ExitStatus, Error>;

/// Translates [ComponentControllerRequest] messages to signals on the `task`.
///
/// When a `Stop` request is received, it will send a `SIGINT` to the task.
/// When a `Kill` request is received, it will send a `SIGKILL` to the task and close the component
/// controller channel regardless if/how the task responded to the signal. Due to Linux's design,
/// this may not reliably cleanup everything that was started as a result of running the component.
///
/// If the task has completed, it will also close the controller channel.
async fn serve_component_controller(
    controller: ComponentControllerRequestStream,
    task: Weak<Task>,
    task_complete: oneshot::Receiver<TaskResult>,
) {
    let controller_handle = controller.control_handle();

    enum Event<T, U> {
        Controller(T),
        Completion(U),
    }

    let mut stream = futures::stream::select(
        controller.map(Event::Controller),
        task_complete.into_stream().map(Event::Completion),
    );

    while let Some(event) = stream.next().await {
        match event {
            Event::Controller(request) => match request {
                Ok(ComponentControllerRequest::Stop { .. }) => {
                    if let Some(task) = task.upgrade() {
                        signals::send_standard_signal(
                            task.as_ref(),
                            signals::SignalInfo::kernel(SIGINT),
                        );
                        log_info!("Sent SIGINT to program {}", task.command());
                    }
                }
                Ok(ComponentControllerRequest::Kill { .. }) => {
                    if let Some(task) = task.upgrade() {
                        signals::send_standard_signal(&task, signals::SignalInfo::kernel(SIGKILL));
                        log_info!("Sent SIGKILL to program {}", task.command());
                        controller_handle.shutdown_with_epitaph(zx::Status::err_from_raw(
                            fcomponent::Error::InstanceDied.into_primitive() as i32,
                        ));
                    }
                    return;
                }
                Ok(ComponentControllerRequest::_UnknownMethod { ordinal, .. }) => {
                    log_warn!("Unknown ComponentController request: {ordinal}");
                }
                Err(_) => {
                    return;
                }
            },
            Event::Completion(result) => match result {
                Ok(Ok(ExitStatus::Exit(0))) => controller_handle.shutdown_with_epitaph(Ok(())),
                Ok(Ok(ExitStatus::Exit(n))) => {
                    let epitaph =
                        zx::Status::try_from_raw(COMPONENT_EXIT_CODE_BASE + n as i32).unwrap();
                    controller_handle.shutdown_with_epitaph(Err(epitaph));
                }
                _ => {
                    let epitaph = zx::Status::try_from_raw(
                        fcomponent::Error::InstanceDied.into_primitive() as i32,
                    )
                    .unwrap();
                    controller_handle.shutdown_with_epitaph(Err(epitaph));
                }
            },
        }
    }
}

/// Returns /container/component/{random} that doesn't already exist
fn generate_component_path(system_task: &CurrentTask) -> Result<String, Error> {
    // Checking container directory already exists.
    // If this lookup fails, the container might not have the "container" feature enabled.
    let mount_point = system_task.lookup_path_from_root("/container/component/".into())?;

    // Find /container/component/{random} that doesn't already exist
    let component_path = loop {
        let random_string: String =
            rng().sample_iter(&Alphanumeric).take(10).map(char::from).collect();

        // This returns EEXIST if /container/component/{random} already exists.
        // If so, try again with another {random} string.
        match mount_point.create_node(
            system_task,
            random_string.as_str().into(),
            mode!(IFDIR, 0o755),
            DeviceId::NONE,
        ) {
            Ok(_) => break format!("/container/component/{random_string}"),
            Err(errno) if errno == EEXIST => {}
            Err(e) => bail!(e),
        };
    };

    Ok(component_path)
}

/// Adds the given startup handles to a CurrentTask.
///
/// The `numbered_handles` of type `HandleType::FileDescriptor` are used to
/// create files, and the handles are required to be of type `zx::Socket`.
///
/// If there is a `numbered_handles` of type `HandleType::User0`, that is
/// interpreted as the server end of the ShellController protocol.
pub fn parse_numbered_handles(
    current_task: &CurrentTask,
    numbered_handles: Option<Vec<fprocess::HandleInfo>>,
    files: &FdTable,
) -> Result<(), Error> {
    if let Some(numbered_handles) = numbered_handles {
        for numbered_handle in numbered_handles {
            let info = HandleInfo::try_from(numbered_handle.id)?;
            if info.handle_type() == HandleType::FileDescriptor {
                let file = create_file_from_handle(current_task, numbered_handle.handle)?;
                files.insert(current_task, FdNumber::from_raw(info.arg().into()), file)?;
            }
        }
    }

    let stdio = SyslogFile::new_file(current_task);
    // If no numbered handle is provided for each stdio handle, default to syslog.
    for i in [0, 1, 2] {
        if files.get(FdNumber::from_raw(i)).is_err() {
            files.insert(current_task, FdNumber::from_raw(i), stdio.clone())?;
        }
    }

    Ok(())
}

/// A record of the mounts created when starting a component.
///
/// When the record is dropped, the mounts are unmounted.
#[derive(Default)]
struct MountRecord {
    /// The namespace nodes at which we have crated mounts for this component.
    mounts: Vec<NamespaceNode>,
}

impl MountRecord {
    fn mount(
        &mut self,
        mount_point: NamespaceNode,
        what: WhatToMount,
        flags: MountpointFlags,
    ) -> Result<(), Errno> {
        mount_point.mount(what, flags.into())?;
        self.mounts.push(mount_point);
        Ok(())
    }

    fn mount_remote(
        &mut self,
        system_task: &CurrentTask,
        directory: &fio::DirectorySynchronousProxy,
        path: &str,
        mount_options: Option<&String>,
    ) -> Result<(), Error> {
        // The incoming dir_path might not be top level, e.g. it could be /foo/bar.
        // Iterate through each component directory starting from the parent and
        // create it if it doesn't exist.
        let mut current_node =
            system_task.lookup_path_from_root(".".into()).context("looking up '.'")?;
        let mut context = LookupContext::default();

        // Extract each component using Path::new(path).components(). For example,
        // Path::new("/foo/bar").components() will return [RootDir, Normal("foo"), Normal("bar")].
        // We're not interested in the RootDir, so we drop the prefix "/" if it exists.
        let path = if let Some(path) = path.strip_prefix('/') { path } else { path };

        for sub_dir in Path::new(path).components() {
            let sub_dir_bytes = sub_dir.as_os_str().as_bytes();
            current_node = match current_node.create_node(
                system_task,
                sub_dir_bytes.into(),
                mode!(IFDIR, 0o755),
                DeviceId::NONE,
            ) {
                Ok(node) => node,
                Err(errno) if errno == EEXIST || errno == ENOTDIR => current_node
                    .lookup_child(system_task, &mut context, sub_dir_bytes.into())
                    .with_context(|| format!("looking up {sub_dir:?}"))?,
                Err(e) => bail!(e),
            };
        }

        let flags = directory
            .get_flags(zx::MonotonicInstant::INFINITE)
            .context("transport error")?
            .map_err(zx::Status::err_from_raw)
            .context("get_flags")?;
        let rights = flags.intersection(fio::MASK_KNOWN_PERMISSIONS);

        let (client_end, server_end) = zx::Channel::create();
        directory.clone(ServerEnd::new(server_end)).context("cloning directory")?;

        // If a filesystem security label argument was provided then apply it to all files via
        // mountpoint-labeling, with a "context=..." mount option.
        let params = if let Some(mount_options) = mount_options {
            MountParams::parse(mount_options.as_str().into())
                .expect("failed to parse default_ns_mount_options")
        } else {
            MountParams::default()
        };

        let fs = RemoteFs::new_fs(
            system_task.kernel(),
            client_end,
            FileSystemOptions { source: path.into(), params, ..Default::default() },
            rights,
        )
        .context("making remote fs")?;

        security::file_system_resolve_security(system_task, &fs).context("resolving security")?;

        // Fuchsia doesn't specify mount flags in the incoming namespace, so we need to make
        // up some flags.
        let flags = MountFlags::NOSUID | MountFlags::NODEV | MountFlags::RELATIME;
        current_node.mount(WhatToMount::Fs(fs), flags.mountpoint_flags()).context("mounting fs")?;
        self.mounts.push(current_node);

        Ok(())
    }

    fn unmount(&mut self) -> Result<(), Errno> {
        while let Some(node) = self.mounts.pop() {
            node.unmount(UnmountFlags::DETACH)?;
        }
        Ok(())
    }
}

impl Drop for MountRecord {
    fn drop(&mut self) {
        match self.unmount() {
            Ok(()) => {}
            Err(e) => log_error!("failed to unmount during component exit: {:?}", e),
        }
    }
}
