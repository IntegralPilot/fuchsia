// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::StartError;
use crate::loader::{CArray, Hooks, Library, Loader};
use crate::util;
use async_trait::async_trait;
use derivative::Derivative;
use elf_runner::ElfComponentLaunchInfo;
use elf_runner::config::ElfProgramConfig;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_process as fprocess;
use fidl_fuchsia_process_lifecycle as flifecycle;
use fuchsia_async as fasync;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use futures::prelude::*;
use futures::select;
use log::{info, warn};
use namespace::Namespace;
use runner::component::StopInfo;
use runner::{StartInfo, component as runner_component};
use std::ffi::CString;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::{mem, ptr, thread};
use vfs::ExecutionScope;

pub(super) type TerminateCallback = Box<dyn FnOnce(&str) + Send>;

pub(super) async fn start(
    start_info: frunner::ComponentStartInfo,
    controller_server: ServerEnd<frunner::ComponentControllerMarker>,
    env: &fdf_env::Environment,
    thread_role: &str,
    scope: &fasync::ScopeHandle,
    terminate_cb: TerminateCallback,
) -> Result<(), StartError> {
    let LaunchInfo { controller, exit_fut } =
        match Component::launch(start_info, &env, thread_role, terminate_cb).await {
            Ok(c) => c,
            Err(e) => {
                _ = controller_server.close_with_epitaph(zx::Status::from(&e));
                return Err(e);
            }
        };

    let (requests, control) = controller_server.into_stream_and_control_handle();
    let url = controller.url.clone();
    let controller = runner_component::Controller::new(controller, requests, control);
    scope.spawn_local(async move {
        info!(url:%; "Started component");
        _ = controller.serve(pin!(exit_fut)).await;
        info!(url:%; "Component stopped");
    });
    Ok(())
}

#[derive(Derivative)]
#[derivative(Debug)]
struct Component {
    url: String,
    argv: Vec<String>,
    environ: Vec<String>,
    ns: Namespace,
    handle_infos: Vec<fprocess::HandleInfo>,
    lifecycle_client: flifecycle::LifecycleProxy,
    library: Library,
    local_scope: ExecutionScope,
    control: InnerControl,
}

struct LaunchInfo {
    controller: Controller,
    exit_fut: Pin<Box<dyn Future<Output = StopInfo> + 'static>>,
}

impl Component {
    async fn launch(
        start_info: frunner::ComponentStartInfo,
        env: &fdf_env::Environment,
        thread_role: &str,
        terminate_routine: TerminateCallback,
    ) -> Result<LaunchInfo, StartError> {
        let dso_path = runner::get_program_string(&start_info, "binary")
            .ok_or(StartError::InvalidArgs)?
            .to_string();

        let mut start_info = StartInfo::try_from(start_info)?;
        let mut config = ElfProgramConfig::parse_and_check(&start_info.program, None)?;
        // DSO components should always use their lifecycle handle so always give them one.
        config.notify_lifecycle_stop = true;
        let ElfComponentLaunchInfo {
            ns,
            handle_infos,
            utc_clock: _,
            lifecycle_client,
            outgoing_directory: _,
            local_scope,
        } = ElfComponentLaunchInfo::new(&mut start_info, &config, None)?;
        let lifecycle_client = lifecycle_client.expect("lifecycle missing");
        let argv = runner::get_program_args_from_dict(&start_info.program)?;
        let environ = runner::get_environ(&start_info.program)?.unwrap_or_default();
        let is_async = runner::get_bool(&start_info.program, "async")?;
        let sched_role = runner::get_string(&start_info.program, "scheduler_role");

        let dso_vmo = util::get_pkg_file_vmo(&ns, &dso_path).await?;
        let dso_name = util::basename(&dso_path);
        dso_vmo.set_name(&zx::Name::new_lossy(dso_name)).map_err(|_| StartError::Internal)?;
        let lib_dir = util::open_pkg_path(&ns, "lib").await?;

        let library = Loader::install(dso_name, dso_vmo, lib_dir)
            .map_err(|err| StartError::LoadDso { name: dso_name.into(), err })?;

        let url = start_info.resolved_url.clone();

        let driver = Arc::new(());
        let name = dso_name.to_string();
        let control;
        let dispatcher;
        let shutdown_done_rx;
        if is_async {
            let sched_role = sched_role.ok_or_else(|| {
                warn!(url:%, is_async:%; "Component missing scheduler role");
                StartError::InvalidArgs
            })?;
            if sched_role != thread_role {
                warn!(url:%, sched_role:%, is_async:%; "Component using unsupported scheduler role");
                return Err(StartError::InvalidArgs);
            }

            // Thread pool may or may not exist yet so configure it now
            env.set_thread_limit(thread_role, 1).expect("set thread limit");
            let role_opts = env.get_scheduler_role_opts(thread_role);
            env.set_scheduler_role_opts(
                thread_role,
                role_opts | fdf_env::Environment::SCHEDULER_ROLE_OPTION_NO_SYNC_CALLS,
            )
            .expect("set scheduler role opts");

            let fdf_dispatcher = fdf::DispatcherBuilder::new()
                .scheduler_role(sched_role)
                .name(dso_name)
                .no_thread_migration()
                .shutdown_observer(move |_| {
                    info!(name:%; "dispatcher shutdown");
                });
            let driver = Arc::into_raw(driver);
            let runtime_handle = env.new_driver(driver);
            runtime_handle.add_allowed_scheduler_role(sched_role);
            // SAFETY: This is safe because `driver` was obtained from [`Arc::into_raw`] above.
            let driver = unsafe { Arc::from_raw(driver) };
            let fdf_dispatcher = runtime_handle.new_dispatcher(fdf_dispatcher);
            let fdf_dispatcher = match fdf_dispatcher {
                Ok(d) => d,
                Err(err) => {
                    // Otherwise drop(runtime_handle) will panic because the dispatcher was
                    // not shutdown. Since this is the case where `new_dispatcher` failed
                    // there's nothing to shutdown
                    mem::forget(runtime_handle);
                    return Err(StartError::CreateDispatcher { name: dso_name.into(), err });
                }
            };
            let (tx, rx) = oneshot::channel();
            control = InnerControl::Async(Some(Box::new(AsyncControl {
                runtime_handle: Some(runtime_handle),
                _driver: driver,
                shutdown_done_tx: Some(tx),
                resources: None,
                thread_handle: None,
            })));
            dispatcher = Some(fdf_dispatcher);
            shutdown_done_rx = Some(rx);
        } else {
            control = InnerControl::Sync(Box::new(SyncControl {
                terminate_cb: Some(terminate_routine),
                thread_handle: None,
            }));
            dispatcher = None;
            shutdown_done_rx = None;
        };

        let component = Self {
            url,
            argv,
            environ,
            ns,
            lifecycle_client,
            handle_infos,
            library,
            local_scope,
            control,
        };
        component
            .run(dso_name, dispatcher, shutdown_done_rx)
            .map_err(|err| StartError::Execute { err })
    }

    fn run(
        self,
        dso_name: &str,
        dispatcher: Option<fdf::Dispatcher>,
        shutdown_done_rx: Option<oneshot::Receiver<()>>,
    ) -> Result<LaunchInfo, zx::Status> {
        let Self {
            url,
            library,
            argv,
            environ,
            ns,
            lifecycle_client,
            handle_infos,
            local_scope,
            mut control,
        } = self;
        let is_async = matches!(control, InnerControl::Async(_));
        let hooks = Hooks::new_from_library(&library, is_async).map_err(|expected_symbol| {
            warn!(url:%; "Failed to start component: symbol `{}` missing from \
                              shared object", expected_symbol.to_string_lossy());
            zx::Status::NOT_FOUND
        })?;
        let c_dso_name = CString::new(dso_name).unwrap();

        let (mut handle, mut handle_info): (Vec<_>, Vec<_>) = handle_infos
            .into_iter()
            .map(|h| {
                let fprocess::HandleInfo { handle, id } = h;
                (handle.into_raw(), id)
            })
            .unzip();

        let ns = ns.flatten();
        let mut names_alloc = Vec::with_capacity(ns.len());
        for (i, entry) in ns.into_iter().enumerate() {
            let namespace::Entry { path, directory } = entry;
            names_alloc.push(CString::new(format!("{path}")).unwrap());
            handle.push(directory.into_channel().into_raw());
            let info = fuchsia_runtime::HandleInfo::new(
                fuchsia_runtime::HandleType::NamespaceDirectory,
                i as u16,
            )
            .as_raw();
            handle_info.push(info);
        }

        let exit_wait;
        match &mut control {
            InnerControl::Async(None) => unreachable!("missing async control"),
            InnerControl::Async(Some(control)) => {
                let dso_name = dso_name.to_string();
                let dispatcher =
                    dispatcher.expect("missing dispatcher for async component").release();
                let (exit_code_tx, exit_code_rx) = oneshot::channel();
                let shutdown_done_rx = shutdown_done_rx.expect("missing shutdown_done_rx");

                let mut names: Vec<_> = names_alloc.iter().map(|n| n.as_ptr()).collect();
                let argv_alloc: Vec<_> =
                    argv.into_iter().map(|s| CString::new(s).unwrap()).collect();
                let mut argv: Vec<_> = argv_alloc.iter().map(|s| s.as_ptr()).collect();
                argv.insert(0, c_dso_name.as_ptr());
                let envp_alloc: Vec<_> =
                    environ.into_iter().map(|s| CString::new(s).unwrap()).collect();
                let mut envp: Vec<_> = envp_alloc.iter().map(|s| s.as_ptr()).collect();
                envp.push(ptr::null_mut());

                let handle_arr = CArray::new(&mut handle);
                let handle_info_arr = CArray::new(&mut handle_info);
                let names_arr = CArray::new(&mut names);
                let argv_arr = CArray::new(&mut argv);
                let envp_arr = CArray::new(&mut envp);

                // Because the program is still running after dso_start_async returns, it may
                // (and probably will) later make references to the inputs. Therefore the
                // component's execution context holds onto them until the component is shutdown.
                control.resources = Some(AsyncProgramResources {
                    _handle: handle,
                    _handle_info: handle_info,
                    _names: names,
                    _names_alloc: names_alloc,
                    _argv: argv,
                    _argv_alloc: argv_alloc,
                    _envp: envp,
                    _envp_alloc: envp_alloc,
                });

                let h = thread::Builder::new().name(dso_name.clone().into()).spawn(move || {
                    // SAFETY: Inputs are not freed until the dispatcher is shutdown.
                    let code = unsafe {
                        hooks.dso_start_async(
                            handle_arr,
                            handle_info_arr,
                            names_arr,
                            argv_arr,
                            envp_arr,
                            dispatcher,
                        )
                    };
                    if code != 0 {
                        warn!(dso_name:%, code:%; "async component dso_start returned non-zero");
                        _ = exit_code_tx.send(code);
                    } else {
                        drop(exit_code_tx);
                    }
                }).expect("spawn async thread");

                exit_wait = async move {
                    let mut shutdown_wait = async move {
                        _ = shutdown_done_rx.await;
                    }
                    .boxed()
                    .fuse();
                    let mut exit_code_wait = exit_code_rx.boxed().fuse();
                    loop {
                        select! {
                            _ = shutdown_wait => {
                                return Ok(None);
                            }
                            res = exit_code_wait => {
                                match res {
                                    Ok(c) => return Ok(Some(c)),
                                    Err(_) => {
                                        // We end up here if dso_main_async returned 0. In that case
                                        // no exit code will be sent so there is no more exit code
                                        // to wait for.
                                        exit_code_wait = async move {
                                            std::future::pending::<()>().await;
                                            Ok(0)
                                        }.boxed().fuse();
                                    }
                                }
                            }
                        }
                    }
                }
                .boxed()
                .fuse();
                control.thread_handle = Some(h);
            }
            InnerControl::Sync(control) => {
                let (exit_tx, exit_rx) = oneshot::channel();
                let h = thread::Builder::new()
                    .name(dso_name.into())
                    .spawn(move || {
                        let mut names: Vec<_> = names_alloc.iter().map(|n| n.as_ptr()).collect();
                        let argv_alloc: Vec<_> =
                            argv.into_iter().map(|s| CString::new(s).unwrap()).collect();
                        let mut argv: Vec<_> = argv_alloc.iter().map(|s| s.as_ptr()).collect();
                        argv.insert(0, c_dso_name.as_ptr());

                        let envp_alloc: Vec<_> =
                            environ.into_iter().map(|s| CString::new(s).unwrap()).collect();
                        let mut envp: Vec<_> = envp_alloc.iter().map(|s| s.as_ptr()).collect();
                        envp.push(ptr::null_mut());

                        // SAFETY: Inputs are not freed until `dso_start` returns.
                        let code = unsafe {
                            hooks.dso_start(
                                &mut handle,
                                &mut handle_info,
                                &mut names,
                                &mut argv,
                                &mut envp,
                            )
                        };
                        _ = exit_tx.send(Some(code));
                    })
                    .expect("spawn sync thread");
                control.thread_handle = Some(h);
                exit_wait = exit_rx.boxed().fuse();
            }
        };

        let component_url = url.clone();
        let is_async = match &mut control {
            InnerControl::Async(_) => true,
            InnerControl::Sync(_) => false,
        };
        let lifecycle_client2 = lifecycle_client.clone();
        let exit_fut = async move {
            let mut exit_wait = exit_wait;
            let mut lifecycle_close_wait = if is_async {
                async move {
                    let mut event_stream = lifecycle_client2.take_event_stream();
                    while let Some(_) = event_stream.next().await {
                        // TODO(https://fxbug.dev/508351654): Handle lifecycle events like OnEscrow
                    }
                    // Lifecycle channel closed means component has exited.
                }
                .boxed()
            } else {
                std::future::pending::<()>().boxed()
            }
            .fuse();
            select! {
                _ = lifecycle_close_wait => {
                    info!(component_url:%, exit_code:% = 0, is_async:%; "Component terminated");
                    return StopInfo { termination_status: Ok(()), exit_code: Some(0) };
                }
                res = exit_wait => {
                    match res {
                        Ok(Some(exit_code)) => {
                            let exit_code: i64 = exit_code.into();
                            info!(component_url:%, exit_code:%, is_async:%; "Component terminated");
                            return StopInfo {
                                termination_status: Ok(()),
                                exit_code: Some(exit_code),
                            };
                        }
                        Ok(None) | Err(_) => {
                            // This case handles async dispatcher shutdown
                            assert!(is_async);
                            warn!(component_url:%, is_async:%; "Component terminated (killed)");
                            return StopInfo::from_error(
                                fcomponent::Error::InstanceDied,
                                None,
                            );
                        }
                    }
                }
            }
        };
        let controller = Controller {
            url,
            _library: library,
            _local_scope: local_scope,
            lifecycle_client,
            control,
        };
        Ok(LaunchInfo { controller, exit_fut: Box::pin(exit_fut) })
    }
}

#[derive(Debug)]
enum InnerControl {
    Sync(Box<SyncControl>),
    // `None` if shutdown has completed and there is nothing left to do
    Async(Option<Box<AsyncControl>>),
}

#[derive(Derivative)]
#[derivative(Debug)]
struct SyncControl {
    #[derivative(Debug = "ignore")]
    terminate_cb: Option<TerminateCallback>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

#[derive(Debug)]
struct AsyncControl {
    runtime_handle: Option<fdf_env::Driver<()>>,
    _driver: Arc<()>,
    shutdown_done_tx: Option<oneshot::Sender<()>>,
    resources: Option<AsyncProgramResources>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

// SAFETY: [`AsyncProgramResources`] is only freed once the async component's dispatcher is shutdown.
unsafe impl Send for AsyncProgramResources {}

#[derive(Debug)]
struct AsyncProgramResources {
    _handle: Vec<zx::sys::zx_handle_t>,
    _handle_info: Vec<u32>,
    _names: Vec<*const ::libc::c_char>,
    _names_alloc: Vec<CString>,
    _argv: Vec<*const ::libc::c_char>,
    _argv_alloc: Vec<CString>,
    _envp: Vec<*const ::libc::c_char>,
    _envp_alloc: Vec<CString>,
}

impl Drop for AsyncControl {
    fn drop(&mut self) {
        // We may end up here without maybe_shutdown_dispatcher if an error preempted the component
        // from starting.
        let Self { runtime_handle, _driver, shutdown_done_tx: _, resources, thread_handle: _ } =
            self;
        // Must be done before dropping `_driver`, otherwise it will panic.
        runtime_handle.take().map(|r| r.shutdown(|_| {}));
        drop(resources.take());
    }
}

async fn maybe_shutdown_dispatcher(ctl: Option<Box<AsyncControl>>) {
    let Some(mut ctl) = ctl else {
        return;
    };
    let (tx, rx) = oneshot::channel();
    ctl.runtime_handle.take().expect("shutdown called twice").shutdown(|_| {
        _ = tx.send(());
    });
    _ = rx.await;
    _ = ctl.shutdown_done_tx.take().expect("shutdown called twice").send(());
}

#[derive(Derivative)]
#[derivative(Debug)]
struct Controller {
    // For debugging
    url: String,
    // Must be kept alive while executable is running
    _library: Library,
    _local_scope: ExecutionScope,
    control: InnerControl,
    lifecycle_client: flifecycle::LifecycleProxy,
}

#[async_trait]
impl runner_component::Controllable for Controller {
    fn teardown<'a>(&mut self) -> BoxFuture<'a, ()> {
        match &mut self.control {
            InnerControl::Async(control) => maybe_shutdown_dispatcher(control.take()).boxed(),
            InnerControl::Sync(control) => {
                // If there was a thread it should have exited by now.
                if let Some(thread_handle) = control.thread_handle.take() {
                    thread_handle.join().unwrap();
                }
                return async move {}.boxed();
            }
        }
    }

    async fn kill(&mut self) {
        match &mut self.control {
            InnerControl::Async(control) => {
                maybe_shutdown_dispatcher(control.take()).await;
            }
            InnerControl::Sync(control) => {
                if let Some(cb) = control.terminate_cb.take() {
                    (cb)(&self.url);
                }
            }
        }
    }

    fn stop<'a>(&mut self) -> BoxFuture<'a, ()> {
        let lifecycle_client = self.lifecycle_client.clone();
        async move {
            _ = lifecycle_client.stop();
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::{self, ClientEnd};
    use fidl_fuchsia_data as fdata;
    use fidl_fuchsia_io as fio;
    use futures::poll;
    use futures::task::Poll;
    use test_case::test_case;

    #[derive(Debug)]
    struct ComponentInfo {
        controller: frunner::ComponentControllerProxy,
        termination_rx: Option<oneshot::Receiver<()>>,
        _scope: fasync::Scope,
        _outgoing: ClientEnd<fio::DirectoryMarker>,
        _runtime: ClientEnd<fio::DirectoryMarker>,
    }

    #[derive(Debug, Clone, Copy)]
    enum Syncness {
        Sync,
        Async,
    }

    fn terminate_cb() -> (TerminateCallback, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        let f = Box::new(move |_url: &str| {
            _ = tx.send(());
        });
        (f, rx)
    }

    async fn start_component(
        env: &fdf_env::Environment,
        binary: &str,
        syncness: Syncness,
    ) -> Result<ComponentInfo, StartError> {
        let scope = fasync::Scope::new();
        let (controller, controller_server) = endpoints::create_endpoints();
        let (outgoing, outgoing_server) = endpoints::create_endpoints();
        let (runtime, runtime_server) = endpoints::create_endpoints();
        let (pkg, pkg_server) = zx::Channel::create();
        let is_async = match syncness {
            Syncness::Sync => "false",
            Syncness::Async => "true",
        };
        fdio::open("/pkg", fio::PERM_READABLE | fio::PERM_EXECUTABLE, pkg_server).unwrap();
        let start_info = frunner::ComponentStartInfo {
            resolved_url: Some("fuchsia-pkg://fuchsia.com/something#meta/something.cm".into()),
            program: Some(fdata::Dictionary {
                entries: Some(vec![
                    fdata::DictionaryEntry {
                        key: "runner".into(),
                        value: Some(Box::new(fdata::DictionaryValue::Str("dso".into()))),
                    },
                    fdata::DictionaryEntry {
                        key: "binary".into(),
                        value: Some(Box::new(fdata::DictionaryValue::Str(binary.into()))),
                    },
                    fdata::DictionaryEntry {
                        key: "async".into(),
                        value: Some(Box::new(fdata::DictionaryValue::Str(is_async.into()))),
                    },
                    fdata::DictionaryEntry {
                        key: "scheduler_role".into(),
                        value: Some(Box::new(fdata::DictionaryValue::Str("test_role".into()))),
                    },
                ]),
                ..Default::default()
            }),
            component_instance: Some(zx::Event::create()),
            ns: Some(vec![frunner::ComponentNamespaceEntry {
                path: Some("/pkg".into()),
                directory: Some(ClientEnd::from(pkg)),
                ..Default::default()
            }]),
            outgoing_dir: Some(outgoing_server),
            runtime_dir: Some(runtime_server),
            ..Default::default()
        };
        let (termination_routine, termination_rx) = terminate_cb();
        start(start_info, controller_server, env, "test_role", &scope, termination_routine).await?;
        Ok(ComponentInfo {
            controller: controller.into_proxy(),
            termination_rx: Some(termination_rx),
            _scope: scope,
            _outgoing: outgoing,
            _runtime: runtime,
        })
    }

    async fn lib_so(name: &str) -> String {
        #[cfg(feature = "variant_coverage")]
        let path = async {
            // The path could be either lib/coverage or lib/coverage-rust, look for something
            // that begins with coverage
            let (client, server) = endpoints::create_proxy::<fio::DirectoryMarker>();
            fdio::open("/pkg/lib", fio::PERM_READABLE, server.into_channel()).unwrap();
            for entry in fuchsia_fs::directory::readdir(&client).await.unwrap() {
                let dir = entry.name;
                if dir.starts_with("coverage") {
                    return format!("lib/{dir}/{name}");
                }
            }
            panic!("no coverage dir found");
        }
        .await;
        #[cfg(feature = "variant_asan")]
        let path = format!("lib/asan-ubsan/{name}");
        #[cfg(feature = "variant_hwasan")]
        let path = format!("lib/hwasan-ubsan/{name}");
        #[cfg(not(any(
            feature = "variant_asan",
            feature = "variant_hwasan",
            feature = "variant_coverage"
        )))]
        let path = format!("lib/{name}");
        path
    }

    unsafe extern "C" {
        safe fn simple_sync_read_run_counter() -> u32;
        safe fn simple_async_read_run_counter() -> u32;
        safe fn hanging_sync_read_run_counter() -> u32;
        safe fn hanging_async_read_run_counter() -> u32;
        safe fn waiting_sync_read_run_counter() -> u32;
        safe fn waiting_async_read_run_counter() -> u32;
        safe fn rust_sync_read_run_counter() -> u32;
        safe fn rust_async_read_run_counter() -> u32;
    }

    #[derive(Debug)]
    enum Language {
        Cpp,
        Rust,
    }

    #[fuchsia::test]
    async fn start_binary_not_found() {
        let env = crate::init();
        let binary = lib_so("libdoes_not_exist.so").await;
        for syncness in [Syncness::Sync, Syncness::Async] {
            let res = start_component(&env, &binary, syncness).await;
            assert_matches!(
                &res,
                Err(StartError::OpenDsoFidl { path, err: _ }) if *path == binary
            );
            assert_eq!(
                zx::Status::from(&res.unwrap_err()).into_raw(),
                fcomponent::Error::ResourceNotFound.into_primitive() as i32
            );
        }
    }

    #[test_case(Language::Cpp)]
    #[test_case(Language::Rust)]
    #[fuchsia::test]
    async fn start_and_exit_sync(lang: Language) {
        let read_run_counter = || {
            (match lang {
                Language::Cpp => simple_sync_read_run_counter(),
                Language::Rust => rust_sync_read_run_counter(),
            }) as usize
        };

        const NUM_REPS: usize = 3;
        let env = crate::init();
        let name = match lang {
            Language::Cpp => "libsimple_sync.so",
            Language::Rust => "librust_sync.so",
        };
        let binary = lib_so(name).await;
        for _ in 0..NUM_REPS {
            let mut component = start_component(&env, &binary, Syncness::Sync).await.unwrap();
            let mut event_stream = component.controller.take_event_stream();
            assert_matches!(
                event_stream.next().await,
                Some(Ok(frunner::ComponentControllerEvent::OnStop {
                    payload: frunner::ComponentStopInfo {
                        termination_status: Some(0),
                        exit_code: Some(0),
                        ..
                    }
                }))
            );
            assert_matches!(event_stream.next().await, None);
            // Termination callback should not run.
            assert_matches!(component.termination_rx.take().unwrap().await, Err(oneshot::Canceled));
        }
        assert_eq!((read_run_counter)(), NUM_REPS);
    }

    #[test_case(Language::Cpp)]
    #[test_case(Language::Rust)]
    #[fuchsia::test]
    async fn start_and_exit_async(lang: Language) {
        let read_run_counter = || {
            (match lang {
                Language::Cpp => simple_async_read_run_counter(),
                Language::Rust => rust_async_read_run_counter(),
            }) as usize
        };

        const NUM_REPS: usize = 3;
        let env = crate::init();
        let name = match lang {
            Language::Cpp => "libsimple_async.so",
            Language::Rust => "librust_async.so",
        };
        let binary = lib_so(name).await;
        for _ in 0..NUM_REPS {
            let component = start_component(&env, &binary, Syncness::Async).await.unwrap();
            let mut event_stream = component.controller.take_event_stream();
            assert_matches!(
                event_stream.next().await,
                Some(Ok(frunner::ComponentControllerEvent::OnStop {
                    payload: frunner::ComponentStopInfo {
                        termination_status: Some(0),
                        exit_code: Some(0),
                        ..
                    }
                }))
            );
            assert_matches!(event_stream.next().await, None);
        }
        assert_eq!(read_run_counter(), NUM_REPS);
    }

    #[test_case(Language::Cpp)]
    #[test_case(Language::Rust)]
    #[fuchsia::test]
    async fn start_and_exit_sync_with_error(lang: Language) {
        const NUM_REPS: usize = 3;
        let env = crate::init();
        let name = match lang {
            Language::Cpp => "liberror_sync.so",
            Language::Rust => "librust_error_sync.so",
        };
        let binary = lib_so(name).await;
        for _ in 0..NUM_REPS {
            let mut component = start_component(&env, &binary, Syncness::Sync).await.unwrap();
            let mut event_stream = component.controller.take_event_stream();
            assert_matches!(
                event_stream.next().await,
                Some(Ok(frunner::ComponentControllerEvent::OnStop {
                    payload: frunner::ComponentStopInfo {
                        termination_status: Some(0),
                        // Returned by dso_main
                        exit_code: Some(1),
                        ..
                    }
                }))
            );
            assert_matches!(event_stream.next().await, None);
            // Termination callback should not run.
            assert_matches!(component.termination_rx.take().unwrap().await, Err(oneshot::Canceled));
        }
    }

    // This test is for C++ only because the rust DSO bindings in libfuchsia always generate a
    // _dso_main_async that returns 0. This is because its implementation of _dso_main_async
    // currently just spawns a thread to run dso_main_async() and exits; the reason for this that
    // the fuchsia-async executor expects to have its own thread.
    #[test_case(Language::Cpp)]
    #[fuchsia::test]
    async fn start_and_exit_async_with_error(lang: Language) {
        const NUM_REPS: usize = 3;
        let env = crate::init();
        let name = match lang {
            Language::Cpp => "liberror_async.so",
            Language::Rust => unreachable!(),
        };
        let binary = lib_so(name).await;
        for _ in 0..NUM_REPS {
            let component = start_component(&env, &binary, Syncness::Async).await.unwrap();
            let mut event_stream = component.controller.take_event_stream();
            assert_matches!(
                event_stream.next().await,
                Some(Ok(frunner::ComponentControllerEvent::OnStop {
                    payload: frunner::ComponentStopInfo {
                        termination_status: Some(0),
                        // Returned by dso_main_async
                        exit_code: Some(1),
                        ..
                    }
                }))
            );
            assert_matches!(event_stream.next().await, None);
        }
    }

    // TODO(https://fxbug.dev/492227113): This test needs libfdio thread local support to allow the
    // component to retrieve the lifecycle handle.
    #[ignore]
    #[fuchsia::test]
    async fn start_and_stop_sync() {
        fn read_run_counter() -> usize {
            waiting_sync_read_run_counter() as usize
        }

        const NUM_REPS: usize = 3;
        let env = crate::init();
        let binary = lib_so("libwaiting_sync.so").await;

        let mut components = vec![];
        for _ in 0..NUM_REPS {
            components.push(start_component(&env, &binary, Syncness::Sync).await.unwrap());
        }
        while read_run_counter() < NUM_REPS {
            fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await
        }
        assert_eq!(read_run_counter(), NUM_REPS);
        for i in 0..NUM_REPS {
            components[i].controller.stop().unwrap();
        }

        for i in 0..NUM_REPS {
            let mut event_stream = components[i].controller.take_event_stream();
            assert_matches!(
                event_stream.next().await,
                Some(Ok(frunner::ComponentControllerEvent::OnStop {
                    payload: frunner::ComponentStopInfo {
                        termination_status: Some(0),
                        exit_code: Some(0),
                        ..
                    }
                }))
            );
            assert_matches!(event_stream.next().await, None);
        }
    }

    #[fuchsia::test]
    async fn start_and_stop_async() {
        fn read_run_counter() -> usize {
            waiting_async_read_run_counter() as usize
        }

        const NUM_REPS: usize = 3;
        let env = crate::init();
        let binary = lib_so("libwaiting_async.so").await;

        let mut components = vec![];
        for _ in 0..NUM_REPS {
            components.push(start_component(&env, &binary, Syncness::Async).await.unwrap());
        }
        while read_run_counter() < NUM_REPS {
            fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await
        }
        assert_eq!(read_run_counter(), NUM_REPS);
        for i in 0..NUM_REPS {
            components[i].controller.stop().unwrap();
        }

        for i in 0..NUM_REPS {
            let mut event_stream = components[i].controller.take_event_stream();
            assert_matches!(
                event_stream.next().await,
                Some(Ok(frunner::ComponentControllerEvent::OnStop {
                    payload: frunner::ComponentStopInfo {
                        termination_status: Some(0),
                        exit_code: Some(0),
                        ..
                    }
                }))
            );
            assert_matches!(event_stream.next().await, None);
        }
    }

    #[fuchsia::test]
    async fn start_and_kill_sync() {
        fn read_run_counter() -> usize {
            hanging_sync_read_run_counter() as usize
        }

        const NUM_REPS: usize = 3;
        let env = crate::init();
        let binary = lib_so("libhanging_sync.so").await;

        let mut components = vec![];
        for _ in 0..NUM_REPS {
            components.push(start_component(&env, &binary, Syncness::Sync).await.unwrap());
        }
        while read_run_counter() < NUM_REPS {
            fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await
        }
        assert_eq!(read_run_counter(), NUM_REPS);
        for i in 0..NUM_REPS {
            components[i].controller.kill().unwrap();
        }
        for i in 0..NUM_REPS {
            // Termination callback should run. But the runner won't close the controller channel.
            // (In a real setting the channel will close because the termination routine exits the
            // process.)
            let mut event_stream = components[i].controller.take_event_stream();
            assert_matches!(poll!(event_stream.next()), Poll::Pending);
            assert_matches!(components[i].termination_rx.take().unwrap().await, Ok(()));
        }
    }

    #[fuchsia::test]
    async fn start_and_kill_async() {
        fn read_run_counter() -> usize {
            hanging_async_read_run_counter() as usize
        }

        const NUM_REPS: usize = 3;
        let env = crate::init();
        let binary = lib_so("libhanging_async.so").await;

        let mut components = vec![];
        for _ in 0..NUM_REPS {
            components.push(start_component(&env, &binary, Syncness::Async).await.unwrap());
        }
        while read_run_counter() < NUM_REPS {
            fasync::Timer::new(fasync::MonotonicDuration::from_millis(100)).await
        }
        assert_eq!(read_run_counter(), NUM_REPS);
        for i in 0..NUM_REPS {
            components[i].controller.kill().unwrap();
        }
        for i in 0..NUM_REPS {
            let mut event_stream = components[i].controller.take_event_stream();
            assert_matches!(
                event_stream.next().await,
                Some(Ok(frunner::ComponentControllerEvent::OnStop {
                    payload: frunner::ComponentStopInfo {
                        termination_status: Some(s),
                        exit_code: None,
                        ..
                    }
                }))
                if s == i32::try_from(fcomponent::Error::InstanceDied.into_primitive()).unwrap()
            );
            assert_matches!(event_stream.next().await, None);
        }
    }
}
