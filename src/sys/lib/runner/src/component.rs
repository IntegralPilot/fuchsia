// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use cm_types::NamespacePath;
use fidl::endpoints::ServerEnd;
use fidl::epitaph::ChannelEpitaphExt;
use fidl::prelude::*;
use fidl_fuchsia_component as fcomp;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fproc;
use fuchsia_async as fasync;
use fuchsia_runtime::{HandleInfo, HandleType, job_default};
use futures::future::{BoxFuture, Either};
use futures::prelude::*;
#[cfg(fuchsia_api_level_at_least = "HEAD")]
use futures::stream::BoxStream;
use log::*;
use namespace::Namespace;
use std::sync::LazyLock;
use thiserror::Error;

pub static PKG_PATH: LazyLock<NamespacePath> = LazyLock::new(|| "/pkg".parse().unwrap());

/// Object implementing this type can be killed by calling kill function.
#[async_trait]
pub trait Controllable {
    /// Should kill self and do cleanup.
    /// Should not return error or panic, should log error instead.
    async fn kill(&mut self);

    /// Stop the component. Once the component is stopped, the
    /// ComponentControllerControlHandle should be closed. If the component is
    /// not stopped quickly enough, kill will be called. The amount of time
    /// `stop` is allowed may vary based on a variety of factors.
    fn stop<'a>(&mut self) -> BoxFuture<'a, ()>;

    /// Perform any teardown tasks before closing the controller channel.
    fn teardown<'a>(&mut self) -> BoxFuture<'a, ()> {
        async {}.boxed()
    }

    /// Monitor any escrow requests from the component.
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    fn on_escrow<'a>(&self) -> BoxStream<'a, fcrunner::ComponentControllerOnEscrowRequest> {
        futures::stream::empty().boxed()
    }
}

/// Holds information about the component that allows the controller to
/// interact with and control the component.
pub struct Controller<C: Controllable> {
    /// stream via which the component manager will ask the controller to
    /// manipulate the component
    request_stream: fcrunner::ComponentControllerRequestStream,

    #[allow(dead_code)] // Only needed at HEAD
    control: fcrunner::ComponentControllerControlHandle,

    /// Controllable object which controls the underlying component.
    /// This would be None once the object is killed.
    controllable: Option<C>,

    /// Task that forwards the `on_escrow` event.
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    on_escrow_monitor: fasync::Task<()>,
}

/// Information about a component's termination (fuchsia.component.runner/ComponentStopInfo)
#[derive(Debug, Clone, PartialEq)]
pub struct StopInfo {
    pub termination_status: Result<(), zx::Status>,
    pub exit_code: Option<i64>,
}

impl StopInfo {
    pub fn from_status(s: zx::Status, c: Option<i64>) -> Self {
        Self { termination_status: zx::Status::ok(s.into_raw()), exit_code: c }
    }

    pub fn from_u32(s: u32, c: Option<i64>) -> Self {
        Self {
            termination_status: zx::Status::ok(i32::try_from(s).unwrap_or(i32::MAX)),
            exit_code: c,
        }
    }

    pub fn from_error(s: fcomp::Error, c: Option<i64>) -> Self {
        Self::from_u32(s.into_primitive().into(), c)
    }

    pub fn from_ok(c: Option<i64>) -> Self {
        Self { termination_status: Ok(()), exit_code: c }
    }
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
impl From<StopInfo> for fcrunner::ComponentStopInfo {
    fn from(info: StopInfo) -> Self {
        Self {
            termination_status: Some(zx::Status::result_into_raw(info.termination_status)),
            exit_code: info.exit_code,
            ..Default::default()
        }
    }
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
impl From<fcrunner::ComponentStopInfo> for StopInfo {
    fn from(value: fcrunner::ComponentStopInfo) -> Self {
        Self {
            termination_status: zx::Status::ok(value.termination_status.unwrap_or(0)),
            exit_code: value.exit_code,
        }
    }
}

impl<C: Controllable + 'static> Controller<C> {
    /// Creates new instance
    pub fn new(
        controllable: C,
        requests: fcrunner::ComponentControllerRequestStream,
        control: fcrunner::ComponentControllerControlHandle,
    ) -> Controller<C> {
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        {
            let on_escrow = controllable.on_escrow();
            let on_escrow_monitor =
                fasync::Task::spawn(Self::monitor_events(on_escrow, requests.control_handle()));
            Controller {
                controllable: Some(controllable),
                request_stream: requests,
                control,
                on_escrow_monitor,
            }
        }
        #[cfg(fuchsia_api_level_less_than = "HEAD")]
        Controller { controllable: Some(controllable), request_stream: requests, control }
    }

    async fn serve_controller(&mut self) -> Result<(), ()> {
        while let Ok(Some(request)) = self.request_stream.try_next().await {
            match request {
                fcrunner::ComponentControllerRequest::Stop { control_handle: _c } => {
                    // Since stop takes some period of time to complete, call
                    // it in a separate context so we can respond to other
                    // requests.
                    let stop_func = self.stop();
                    fasync::Task::spawn(stop_func).detach();
                }
                fcrunner::ComponentControllerRequest::Kill { control_handle: _c } => {
                    self.kill().await;
                    return Ok(());
                }
                fcrunner::ComponentControllerRequest::_UnknownMethod { .. } => (),
            }
        }
        // The channel closed
        Err(())
    }

    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    async fn monitor_events(
        mut on_escrow: impl Stream<Item = fcrunner::ComponentControllerOnEscrowRequest> + Unpin + Send,
        control_handle: fcrunner::ComponentControllerControlHandle,
    ) {
        while let Some(event) = on_escrow.next().await {
            control_handle
                .send_on_escrow(event)
                .unwrap_or_else(|err| error!(err:%; "failed to send OnEscrow event"));
        }
    }

    /// Serve the request stream held by this Controller. `exit_fut` should
    /// complete if the component exits and should return a value which is
    /// either 0 (ZX_OK) or one of the fuchsia.component/Error constants
    /// defined as valid in the fuchsia.component.runner/ComponentController
    /// documentation. This function will return after `exit_fut` completes
    /// or the request stream closes. In either case the request stream is
    /// closed once this function returns since the stream itself, which owns
    /// the channel, is dropped.
    pub async fn serve(mut self, exit_fut: impl Future<Output = StopInfo> + Unpin) {
        let stop_info = {
            // Pin the server_controller future so we can use it with select
            let request_server = self.serve_controller();
            futures::pin_mut!(request_server);

            // Get the result of waiting for `exit_fut` to complete while also
            // polling the request server.
            match future::select(exit_fut, request_server).await {
                Either::Left((return_code, _controller_server)) => return_code,
                Either::Right((serve_result, pending_close)) => match serve_result {
                    Ok(()) => pending_close.await,
                    Err(_) => {
                        // Return directly because the controller channel
                        // closed so there's no point in an epitaph.
                        return;
                    }
                },
            }
        };

        // Before closing the controller channel, perform teardown tasks if the runner configured
        // them. This will only run if the component was not killed (otherwise `controllable` is
        // `None`).
        if let Some(mut controllable) = self.controllable.take() {
            controllable.teardown().await;
        }

        // Drain any escrow events.
        // TODO(https://fxbug.dev/326626515): Drain the escrow requests until no long readable
        // instead of waiting for an unbounded amount of time if `on_escrow` never completes.
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        {
            self.on_escrow_monitor.await;
            _ = self.control.send_on_stop(stop_info.clone().into());
        }
        let _ = stop_info; // avoid unused error at stable API level
        self.request_stream.control_handle().shutdown();
    }

    /// Kill the job and shutdown control handle supplied to this function.
    async fn kill(&mut self) {
        if let Some(mut controllable) = self.controllable.take() {
            controllable.kill().await;
        }
    }

    /// If we have a Controllable, ask it to stop the component.
    fn stop<'a>(&mut self) -> BoxFuture<'a, ()> {
        if self.controllable.is_some() {
            self.controllable.as_mut().unwrap().stop()
        } else {
            async {}.boxed()
        }
    }
}

/// An error encountered trying to launch a component.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum LaunchError {
    #[error("invalid binary path {}", _0)]
    InvalidBinaryPath(String),

    #[error("/pkg missing in the namespace")]
    MissingPkg,

    #[error("error loading executable: {:?}", _0)]
    LoadingExecutable(String),

    #[error("cannot convert proxy to channel")]
    DirectoryToChannel,

    #[error("cannot create channels: {}", _0)]
    ChannelCreation(zx_status::Status),

    #[error("error loading 'lib' in /pkg: {:?}", _0)]
    LibLoadError(String),

    #[error("cannot create job: {}", _0)]
    JobCreation(zx_status::Status),

    #[error("cannot duplicate job: {}", _0)]
    DuplicateJob(zx_status::Status),

    #[error("cannot add args to launcher: {:?}", _0)]
    AddArgs(String),

    #[error("cannot add args to launcher: {:?}", _0)]
    AddHandles(String),

    #[error("cannot add args to launcher: {:?}", _0)]
    AddNames(String),

    #[error("cannot add env to launcher: {:?}", _0)]
    AddEnvirons(String),

    #[error("cannot set options for launcher: {:?}", _0)]
    SetOptions(String),
}

/// Arguments to `configure_launcher` function.
pub struct LauncherConfigArgs<'a> {
    /// relative binary path to /pkg in `ns`.
    pub bin_path: &'a str,

    /// Name of the binary to add to `LaunchInfo`. This will be truncated to
    /// `zx::sys::ZX_MAX_NAME_LEN` bytes.
    pub name: &'a str,

    /// The options used to create the process.
    pub options: zx::ProcessOptions,

    /// Arguments to binary. Binary path will be automatically
    /// prepended so that should not be passed as first argument.
    pub args: Option<Vec<String>>,

    /// Namespace for binary process to be launched.
    pub ns: Namespace,

    /// Job in which process is launched. If None, a child job would be created in default one.
    pub job: Option<zx::Job>,

    /// Extra handle infos to add. This function all ready adds handles for default job and svc
    /// loader.
    pub handle_infos: Option<Vec<fproc::HandleInfo>>,

    /// Extra names to add to namespace. by default only names from `ns` are added.
    pub name_infos: Option<Vec<fproc::NameInfo>>,

    /// Process environment to add to launcher.
    pub environs: Option<Vec<String>>,

    /// proxy for `fuchsia.proc.Launcher`.
    pub launcher: &'a fproc::LauncherProxy,

    /// Custom loader proxy. If None, /pkg/lib would be used to load libraries.
    pub loader_proxy_chan: Option<zx::Channel>,

    /// VMO containing mapping to executable binary. If None, it would be loaded from /pkg.
    pub executable_vmo: Option<zx::Vmo>,
}

/// Configures launcher to launch process using passed params and creates launch info.
/// This starts a library loader service, that will live as long as the handle for it given to the
/// launcher is alive.
pub async fn configure_launcher(
    config_args: LauncherConfigArgs<'_>,
) -> Result<fproc::LaunchInfo, LaunchError> {
    // Locate the '/pkg' directory proxy previously added to the new component's namespace.
    let pkg_dir = config_args.ns.get(&PKG_PATH).ok_or(LaunchError::MissingPkg)?;

    // library_loader provides a helper function that we use to load the main executable from the
    // package directory as a VMO in the same way that dynamic libraries are loaded. Doing this
    // first allows launching to fail quickly and clearly in case the main executable can't be
    // loaded with ZX_RIGHT_EXECUTE from the package directory.
    let executable_vmo = match config_args.executable_vmo {
        Some(v) => v,
        None => library_loader::load_vmo(pkg_dir, &config_args.bin_path)
            .await
            .map_err(|e| LaunchError::LoadingExecutable(e.to_string()))?,
    };

    let ll_client_chan = match config_args.loader_proxy_chan {
        None => {
            // The loader service should only be able to load files from `/pkg/lib`. Giving it a
            // larger scope is potentially a security vulnerability, as it could make it trivial for
            // parts of applications to get handles to things the application author didn't intend.
            let lib_proxy = fuchsia_component::directory::open_directory_async(
                pkg_dir,
                "lib",
                fio::RX_STAR_DIR,
            )
            .map_err(|e| LaunchError::LibLoadError(e.to_string()))?;
            let (ll_client_chan, ll_service_chan) = zx::Channel::create();
            library_loader::start(lib_proxy.into(), ll_service_chan);
            ll_client_chan
        }
        Some(chan) => chan,
    };

    // Get the provided job to create the new process in, if one was provided, or else create a new
    // child job of this process's (this process that this code is running in) own 'default job'.
    let job = config_args
        .job
        .unwrap_or(job_default().create_child_job().map_err(LaunchError::JobCreation)?);

    // Build the command line args for the new process and send them to the launcher.
    let bin_arg = PKG_PATH
        .to_path_buf()
        .join(&config_args.bin_path)
        .to_str()
        .ok_or_else(|| LaunchError::InvalidBinaryPath(config_args.bin_path.to_string()))?
        .as_bytes()
        .to_vec();
    let mut all_args = vec![bin_arg];
    if let Some(args) = config_args.args {
        all_args.extend(args.into_iter().map(|s| s.into_bytes()));
    }
    config_args.launcher.add_args(&all_args).map_err(|e| LaunchError::AddArgs(e.to_string()))?;

    // Get any initial handles to provide to the new process, if any were provided by the caller.
    // Add handles for the new process's default job (by convention, this is the same job that the
    // new process is launched in) and the fuchsia.ldsvc.Loader service created above, then send to
    // the launcher.
    let job_dup =
        job.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(LaunchError::DuplicateJob)?;
    let mut handle_infos = config_args.handle_infos.unwrap_or(vec![]);
    handle_infos.append(&mut vec![
        fproc::HandleInfo {
            handle: ll_client_chan.into_handle(),
            id: HandleInfo::new(HandleType::LdsvcLoader, 0).as_raw(),
        },
        fproc::HandleInfo {
            handle: job_dup.into_handle(),
            id: HandleInfo::new(HandleType::DefaultJob, 0).as_raw(),
        },
    ]);
    config_args
        .launcher
        .add_handles(handle_infos)
        .map_err(|e| LaunchError::AddHandles(e.to_string()))?;

    if !config_args.options.is_empty() {
        config_args
            .launcher
            .set_options(config_args.options.bits())
            .map_err(|e| LaunchError::SetOptions(e.to_string()))?;
    }

    // Send environment variables for the new process, if any, to the launcher.
    let environs: Vec<_> = config_args.environs.unwrap_or(vec![]);
    if environs.len() > 0 {
        let environs_bytes: Vec<_> = environs.into_iter().map(|s| s.into_bytes()).collect();
        config_args
            .launcher
            .add_environs(&environs_bytes)
            .map_err(|e| LaunchError::AddEnvirons(e.to_string()))?;
    }

    // Combine any manually provided namespace entries with the provided Namespace, and
    // then send the new process's namespace to the launcher.
    let mut name_infos = config_args.name_infos.unwrap_or(vec![]);
    let ns: Vec<_> = config_args.ns.into();
    name_infos.extend(ns.into_iter());
    config_args.launcher.add_names(name_infos).map_err(|e| LaunchError::AddNames(e.to_string()))?;

    let name = truncate_str(config_args.name, zx::sys::ZX_MAX_NAME_LEN).to_owned();

    Ok(fproc::LaunchInfo { executable: executable_vmo, job, name })
}

/// Truncates `s` to be at most `max_len` bytes.
fn truncate_str(s: &str, max_len: usize) -> &str {
    let index = s.floor_char_boundary(max_len);
    &s[..index]
}

static CONNECT_ERROR_HELP: &'static str = "To learn more, see \
https://fuchsia.dev/go/components/connect-errors";

/// Sets an epitaph on `ComponentController` `server_end` for a runner failure and the outgoing
/// directory, and logs it.
pub fn report_start_error(
    err: zx::Status,
    err_str: String,
    resolved_url: &str,
    controller_server_end: ServerEnd<fcrunner::ComponentControllerMarker>,
) {
    let _ = controller_server_end.into_channel().close_with_epitaph(err);
    warn!("Failed to start component `{}`: {}\n{}", resolved_url, err_str, CONNECT_ERROR_HELP);
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Error;
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use fidl::endpoints::{ClientEnd, create_endpoints, create_proxy};
    use fidl_fuchsia_component_runner::{self as fcrunner, ComponentControllerProxy};
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_process as fproc;
    use fuchsia_async as fasync;
    use fuchsia_runtime::{HandleInfo, HandleType};
    use futures::future::BoxFuture;
    use futures::poll;
    use namespace::{Namespace, NamespaceError};
    use std::pin::Pin;
    use std::task::Poll;

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("", 0), "");
        assert_eq!(truncate_str("", 1), "");

        assert_eq!(truncate_str("été", 0), "");
        assert_eq!(truncate_str("été", 1), "");
        assert_eq!(truncate_str("été", 2), "é");
        assert_eq!(truncate_str("été", 3), "ét");
        assert_eq!(truncate_str("été", 4), "ét");
        assert_eq!(truncate_str("été", 5), "été");
        assert_eq!(truncate_str("été", 6), "été");
    }

    struct FakeComponent<K, J>
    where
        K: FnOnce() + std::marker::Send,
        J: FnOnce() + std::marker::Send,
    {
        pub onkill: Option<K>,

        pub onstop: Option<J>,

        pub onteardown: Option<BoxFuture<'static, ()>>,
    }

    #[async_trait]
    impl<K: 'static, J: 'static> Controllable for FakeComponent<K, J>
    where
        K: FnOnce() + std::marker::Send,
        J: FnOnce() + std::marker::Send,
    {
        async fn kill(&mut self) {
            let func = self.onkill.take().unwrap();
            func();
        }

        fn stop<'a>(&mut self) -> BoxFuture<'a, ()> {
            let func = self.onstop.take().unwrap();
            async move { func() }.boxed()
        }

        fn teardown<'a>(&mut self) -> BoxFuture<'a, ()> {
            self.onteardown.take().unwrap()
        }
    }

    #[fuchsia::test]
    async fn test_kill_component() -> Result<(), Error> {
        let (sender, recv) = futures::channel::oneshot::channel::<()>();
        let (term_tx, term_rx) = futures::channel::oneshot::channel::<StopInfo>();
        let stop_info = StopInfo::from_ok(Some(42));
        let fake_component = FakeComponent {
            onkill: Some(move || {
                sender.send(()).unwrap();
                // After acknowledging that we received kill, send the status
                // value so `serve` completes.
                let _ = term_tx.send(stop_info.clone());
            }),
            onstop: Some(|| {}),
            onteardown: Some(async {}.boxed()),
        };

        let (controller, client_proxy) = create_controller_and_proxy(fake_component)?;

        client_proxy.kill().expect("FIDL error returned from kill request to controller");

        let term_receiver = Box::pin(async move { term_rx.await.unwrap() });
        // this should return after kill call
        controller.serve(term_receiver).await;

        // this means kill was called
        recv.await?;

        // Check the event on the controller channel, this should match what
        // is sent by `term_tx`
        let mut event_stream = client_proxy.take_event_stream();
        assert_matches!(
            event_stream.try_next().await,
            Ok(Some(fcrunner::ComponentControllerEvent::OnStop {
                payload: fcrunner::ComponentStopInfo {
                    termination_status: Some(0),
                    exit_code: Some(42),
                    ..
                }
            }))
        );
        assert_matches!(event_stream.try_next().await, Ok(None));

        Ok(())
    }

    #[fuchsia::test]
    async fn test_stop_component() -> Result<(), Error> {
        let (sender, recv) = futures::channel::oneshot::channel::<()>();
        let (teardown_signal_tx, teardown_signal_rx) = futures::channel::oneshot::channel::<()>();
        let (teardown_fence_tx, teardown_fence_rx) = futures::channel::oneshot::channel::<()>();
        let (term_tx, term_rx) = futures::channel::oneshot::channel::<StopInfo>();
        let stop_info = StopInfo::from_ok(Some(42));

        let fake_component = FakeComponent {
            onstop: Some(move || {
                sender.send(()).unwrap();
                let _ = term_tx.send(stop_info.clone());
            }),
            onkill: Some(move || {}),
            onteardown: Some(
                async move {
                    teardown_signal_tx.send(()).unwrap();
                    teardown_fence_rx.await.unwrap();
                }
                .boxed(),
            ),
        };

        let (controller, client_proxy) = create_controller_and_proxy(fake_component)?;

        client_proxy.stop().expect("FIDL error returned from kill request to controller");

        let term_receiver = Box::pin(async move { term_rx.await.unwrap() });

        // This should return once the channel is closed, that is after stop and teardown
        let controller_serve = fasync::Task::spawn(controller.serve(term_receiver));

        // This means stop was called
        recv.await?;

        // Teardown should be called
        teardown_signal_rx.await?;

        // Teardown is blocked. Verify there's no event on the channel yet, then unblock it.
        let mut client_stream = client_proxy.take_event_stream();
        let mut client_stream_fut = client_stream.try_next();
        assert_matches!(poll!(Pin::new(&mut client_stream_fut)), Poll::Pending);
        teardown_fence_tx.send(()).unwrap();
        controller_serve.await;

        // Check the event on the controller channel, this should match what
        // is sent by `term_tx`
        assert_matches!(
            client_stream_fut.await,
            Ok(Some(fcrunner::ComponentControllerEvent::OnStop {
                payload: fcrunner::ComponentStopInfo {
                    termination_status: Some(0),
                    exit_code: Some(42),
                    ..
                }
            }))
        );
        assert_matches!(client_stream.try_next().await, Ok(None));

        Ok(())
    }

    #[fuchsia::test]
    fn test_stop_then_kill() -> Result<(), Error> {
        let mut exec = fasync::TestExecutor::new();
        let (sender, mut recv) = futures::channel::oneshot::channel::<()>();
        let (term_tx, term_rx) = futures::channel::oneshot::channel::<StopInfo>();
        let stop_info = StopInfo::from_ok(Some(42));

        // This component will only 'exit' after kill is called.
        let fake_component = FakeComponent {
            onstop: Some(move || {
                sender.send(()).unwrap();
            }),
            onkill: Some(move || {
                let _ = term_tx.send(stop_info.clone());
            }),
            onteardown: Some(async {}.boxed()),
        };

        let (controller, client_proxy) = create_controller_and_proxy(fake_component)?;
        // Send a stop request, note that the controller isn't even running
        // yet, but the request will be waiting in the channel when it does.
        client_proxy.stop().expect("FIDL error returned from stop request to controller");

        // Set up the controller to run.
        let term_receiver = Box::pin(async move { term_rx.await.unwrap() });
        let mut controller_fut = Box::pin(controller.serve(term_receiver));

        // Run the serve loop until it is stalled, it shouldn't return because
        // stop doesn't automatically call exit.
        match exec.run_until_stalled(&mut controller_fut) {
            Poll::Pending => {}
            x => panic!("Serve future should have been pending but was not {:?}", x),
        }

        // Check that stop was called
        assert_eq!(exec.run_until_stalled(&mut recv), Poll::Ready(Ok(())));

        // Kill the component which should call the `onkill` we passed in.
        // This should cause the termination future to complete, which should then
        // cause the controller future to complete.
        client_proxy.kill().expect("FIDL error returned from kill request to controller");
        match exec.run_until_stalled(&mut controller_fut) {
            Poll::Ready(()) => {}
            x => panic!("Unexpected controller poll state {:?}", x),
        }

        // Check the controller channel closed with an event that matches
        // what was sent in the `onkill` closure.
        let mut event_stream = client_proxy.take_event_stream();
        let mut next_fut = event_stream.try_next();
        assert_matches!(
            exec.run_until_stalled(&mut next_fut),
            Poll::Ready(Ok(Some(fcrunner::ComponentControllerEvent::OnStop {
                payload: fcrunner::ComponentStopInfo {
                    termination_status: Some(0),
                    exit_code: Some(42),
                    ..
                }
            })))
        );

        let mut next_fut = event_stream.try_next();
        assert_matches!(exec.run_until_stalled(&mut next_fut), Poll::Ready(Ok(None)));
        Ok(())
    }

    fn create_controller_and_proxy<K: 'static, J: 'static>(
        fake_component: FakeComponent<K, J>,
    ) -> Result<(Controller<FakeComponent<K, J>>, ComponentControllerProxy), Error>
    where
        K: FnOnce() + std::marker::Send,
        J: FnOnce() + std::marker::Send,
    {
        let (client_endpoint, server_endpoint) =
            create_endpoints::<fcrunner::ComponentControllerMarker>();

        // Get a proxy to the ComponentController channel.
        let (controller_stream, control) = server_endpoint.into_stream_and_control_handle();
        Ok((
            Controller::new(fake_component, controller_stream, control),
            client_endpoint.into_proxy(),
        ))
    }

    mod launch_info {
        use fidl::endpoints::Proxy;

        use super::*;
        use anyhow::format_err;
        use futures::channel::oneshot;

        fn setup_empty_namespace() -> Result<Namespace, NamespaceError> {
            setup_namespace(false, vec![])
        }

        fn setup_namespace(
            include_pkg: bool,
            // All the handles created for this will have server end closed.
            // Clients cannot send messages on those handles in ns.
            extra_paths: Vec<&str>,
        ) -> Result<Namespace, NamespaceError> {
            let mut ns = Vec::<fcrunner::ComponentNamespaceEntry>::new();
            if include_pkg {
                let pkg_path = "/pkg".to_string();
                let pkg_chan = fuchsia_fs::directory::open_in_namespace(
                    "/pkg",
                    fio::PERM_READABLE | fio::PERM_EXECUTABLE,
                )
                .unwrap()
                .into_channel()
                .unwrap()
                .into_zx_channel();
                let pkg_handle = ClientEnd::new(pkg_chan);

                ns.push(fcrunner::ComponentNamespaceEntry {
                    path: Some(pkg_path),
                    directory: Some(pkg_handle),
                    ..Default::default()
                });
            }

            for path in extra_paths {
                let (client, _server) = create_endpoints::<fio::DirectoryMarker>();
                ns.push(fcrunner::ComponentNamespaceEntry {
                    path: Some(path.to_string()),
                    directory: Some(client),
                    ..Default::default()
                });
            }
            Namespace::try_from(ns)
        }

        #[derive(Default)]
        struct FakeLauncherServiceResults {
            names: Vec<String>,
            handles: Vec<u32>,
            args: Vec<String>,
            options: zx::ProcessOptions,
        }

        fn start_launcher()
        -> Result<(fproc::LauncherProxy, oneshot::Receiver<FakeLauncherServiceResults>), Error>
        {
            let (launcher_proxy, server_end) = create_proxy::<fproc::LauncherMarker>();
            let (sender, receiver) = oneshot::channel();
            fasync::Task::local(async move {
                let stream = server_end.into_stream();
                run_launcher_service(stream, sender)
                    .await
                    .expect("error running fake launcher service");
            })
            .detach();
            Ok((launcher_proxy, receiver))
        }

        async fn run_launcher_service(
            mut stream: fproc::LauncherRequestStream,
            sender: oneshot::Sender<FakeLauncherServiceResults>,
        ) -> Result<(), Error> {
            let mut res = FakeLauncherServiceResults::default();
            while let Some(event) = stream.try_next().await? {
                match event {
                    fproc::LauncherRequest::AddArgs { args, .. } => {
                        res.args.extend(
                            args.into_iter()
                                .map(|a| {
                                    std::str::from_utf8(&a)
                                        .expect("cannot convert bytes to utf8 string")
                                        .to_owned()
                                })
                                .collect::<Vec<String>>(),
                        );
                    }
                    fproc::LauncherRequest::AddEnvirons { .. } => {}
                    fproc::LauncherRequest::AddNames { names, .. } => {
                        res.names
                            .extend(names.into_iter().map(|m| m.path).collect::<Vec<String>>());
                    }
                    fproc::LauncherRequest::AddHandles { handles, .. } => {
                        res.handles.extend(handles.into_iter().map(|m| m.id).collect::<Vec<u32>>());
                    }
                    fproc::LauncherRequest::SetOptions { options, .. } => {
                        res.options = zx::ProcessOptions::from_bits_retain(options);
                    }
                    fproc::LauncherRequest::CreateWithoutStarting { .. } => {}
                    fproc::LauncherRequest::Launch { .. } => {}
                }
            }
            sender.send(res).map_err(|_e| format_err!("can't send result"))?;
            Ok(())
        }

        #[fuchsia::test]
        async fn missing_pkg() -> Result<(), Error> {
            let (launcher_proxy, _server_end) = create_proxy::<fproc::LauncherMarker>();
            let ns = setup_empty_namespace()?;

            assert_eq!(
                configure_launcher(LauncherConfigArgs {
                    bin_path: "bin/path",
                    name: "name",
                    args: None,
                    options: zx::ProcessOptions::empty(),
                    ns: ns,
                    job: None,
                    handle_infos: None,
                    name_infos: None,
                    environs: None,
                    launcher: &launcher_proxy,
                    loader_proxy_chan: None,
                    executable_vmo: None
                })
                .await,
                Err(LaunchError::MissingPkg),
            );

            drop(_server_end);
            Ok(())
        }

        #[fuchsia::test]
        async fn invalid_executable() -> Result<(), Error> {
            let (launcher_proxy, _server_end) = create_proxy::<fproc::LauncherMarker>();
            let ns = setup_namespace(true, vec![])?;

            match configure_launcher(LauncherConfigArgs {
                bin_path: "test/path",
                name: "name",
                args: None,
                options: zx::ProcessOptions::empty(),
                ns: ns,
                job: None,
                handle_infos: None,
                name_infos: None,
                environs: None,
                launcher: &launcher_proxy,
                loader_proxy_chan: None,
                executable_vmo: None,
            })
            .await
            .expect_err("should error out")
            {
                LaunchError::LoadingExecutable(_) => {}
                e => panic!("Expected LoadingExecutable error, got {:?}", e),
            }
            Ok(())
        }

        #[fuchsia::test]
        async fn invalid_pkg() -> Result<(), Error> {
            let (launcher_proxy, _server_end) = create_proxy::<fproc::LauncherMarker>();
            let ns = setup_namespace(false, vec!["/pkg"])?;

            match configure_launcher(LauncherConfigArgs {
                bin_path: "bin/path",
                name: "name",
                args: None,
                options: zx::ProcessOptions::empty(),
                ns: ns,
                job: None,
                handle_infos: None,
                name_infos: None,
                environs: None,
                launcher: &launcher_proxy,
                loader_proxy_chan: None,
                executable_vmo: None,
            })
            .await
            .expect_err("should error out")
            {
                LaunchError::LoadingExecutable(_) => {}
                e => panic!("Expected LoadingExecutable error, got {:?}", e),
            }
            Ok(())
        }

        #[fuchsia::test]
        async fn default_args() -> Result<(), Error> {
            let (launcher_proxy, recv) = start_launcher()?;

            let ns = setup_namespace(true, vec![])?;

            let _launch_info = configure_launcher(LauncherConfigArgs {
                bin_path: "bin/runner_lib_test",
                name: "name",
                args: None,
                options: zx::ProcessOptions::empty(),
                ns: ns,
                job: None,
                handle_infos: None,
                name_infos: None,
                environs: None,
                launcher: &launcher_proxy,
                loader_proxy_chan: None,
                executable_vmo: None,
            })
            .await?;

            drop(launcher_proxy);

            let ls = recv.await?;

            assert_eq!(ls.args, vec!("/pkg/bin/runner_lib_test".to_owned()));

            Ok(())
        }

        #[fuchsia::test]
        async fn custom_executable_vmo() -> Result<(), Error> {
            let (launcher_proxy, _recv) = start_launcher()?;

            let ns = setup_namespace(true, vec![])?;
            let vmo = zx::Vmo::create(100)?;
            vmo.write(b"my_data", 0)?;
            let launch_info = configure_launcher(LauncherConfigArgs {
                bin_path: "bin/runner_lib_test",
                name: "name",
                args: None,
                options: zx::ProcessOptions::empty(),
                ns: ns,
                job: None,
                handle_infos: None,
                name_infos: None,
                environs: None,
                launcher: &launcher_proxy,
                loader_proxy_chan: None,
                executable_vmo: Some(vmo),
            })
            .await?;

            let mut bytes: [u8; 10] = [0; 10];
            launch_info.executable.read(&mut bytes, 0)?;
            let expected = b"my_data";
            assert_eq!(bytes[0..expected.len()], expected[..]);
            Ok(())
        }

        #[fuchsia::test]
        async fn extra_args() -> Result<(), Error> {
            let (launcher_proxy, recv) = start_launcher()?;

            let ns = setup_namespace(true, vec![])?;

            let args = vec!["args1".to_owned(), "arg2".to_owned()];

            let _launch_info = configure_launcher(LauncherConfigArgs {
                bin_path: "bin/runner_lib_test",
                name: "name",
                args: Some(args.clone()),
                options: zx::ProcessOptions::empty(),
                ns: ns,
                job: None,
                handle_infos: None,
                name_infos: None,
                environs: None,
                launcher: &launcher_proxy,
                loader_proxy_chan: None,
                executable_vmo: None,
            })
            .await?;

            drop(launcher_proxy);

            let ls = recv.await?;

            let mut expected = vec!["/pkg/bin/runner_lib_test".to_owned()];
            expected.extend(args);
            assert_eq!(ls.args, expected);

            Ok(())
        }

        #[fuchsia::test]
        async fn namespace_added() -> Result<(), Error> {
            let (launcher_proxy, recv) = start_launcher()?;

            let ns = setup_namespace(true, vec!["/some_path1", "/some_path2"])?;

            let _launch_info = configure_launcher(LauncherConfigArgs {
                bin_path: "bin/runner_lib_test",
                name: "name",
                args: None,
                options: zx::ProcessOptions::empty(),
                ns: ns,
                job: None,
                handle_infos: None,
                name_infos: None,
                environs: None,
                launcher: &launcher_proxy,
                loader_proxy_chan: None,
                executable_vmo: None,
            })
            .await?;

            drop(launcher_proxy);

            let ls = recv.await?;

            let mut names = ls.names;
            names.sort();
            assert_eq!(
                names,
                vec!("/pkg", "/some_path1", "/some_path2")
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>()
            );

            Ok(())
        }

        #[fuchsia::test]
        async fn extra_namespace_entries() -> Result<(), Error> {
            let (launcher_proxy, recv) = start_launcher()?;

            let ns = setup_namespace(true, vec!["/some_path1", "/some_path2"])?;

            let mut names = vec![];

            let extra_paths = vec!["/extra1", "/extra2"];

            for path in &extra_paths {
                let (client, _server) = create_endpoints::<fio::DirectoryMarker>();

                names.push(fproc::NameInfo { path: path.to_string(), directory: client });
            }

            let _launch_info = configure_launcher(LauncherConfigArgs {
                bin_path: "bin/runner_lib_test",
                name: "name",
                args: None,
                options: zx::ProcessOptions::empty(),
                ns: ns,
                job: None,
                handle_infos: None,
                name_infos: Some(names),
                environs: None,
                launcher: &launcher_proxy,
                loader_proxy_chan: None,
                executable_vmo: None,
            })
            .await?;

            drop(launcher_proxy);

            let ls = recv.await?;

            let mut paths = vec!["/pkg", "/some_path1", "/some_path2"];
            paths.extend(extra_paths.into_iter());
            paths.sort();

            let mut ls_names = ls.names;
            ls_names.sort();

            assert_eq!(ls_names, paths.into_iter().map(|s| s.to_string()).collect::<Vec<String>>());

            Ok(())
        }

        #[fuchsia::test]
        async fn handles_added() -> Result<(), Error> {
            let (launcher_proxy, recv) = start_launcher()?;

            let ns = setup_namespace(true, vec![])?;

            let _launch_info = configure_launcher(LauncherConfigArgs {
                bin_path: "bin/runner_lib_test",
                name: "name",
                args: None,
                options: zx::ProcessOptions::empty(),
                ns: ns,
                job: None,
                handle_infos: None,
                name_infos: None,
                environs: None,
                launcher: &launcher_proxy,
                loader_proxy_chan: None,
                executable_vmo: None,
            })
            .await?;

            drop(launcher_proxy);

            let ls = recv.await?;

            assert_eq!(
                ls.handles,
                vec!(
                    HandleInfo::new(HandleType::LdsvcLoader, 0).as_raw(),
                    HandleInfo::new(HandleType::DefaultJob, 0).as_raw()
                )
            );

            Ok(())
        }

        #[fuchsia::test]
        async fn handles_added_with_custom_loader_chan() -> Result<(), Error> {
            let (launcher_proxy, recv) = start_launcher()?;

            let (c1, _c2) = zx::Channel::create();

            let ns = setup_namespace(true, vec![])?;

            let _launch_info = configure_launcher(LauncherConfigArgs {
                bin_path: "bin/runner_lib_test",
                name: "name",
                args: None,
                options: zx::ProcessOptions::empty(),
                ns: ns,
                job: None,
                handle_infos: None,
                name_infos: None,
                environs: None,
                launcher: &launcher_proxy,
                loader_proxy_chan: Some(c1),
                executable_vmo: None,
            })
            .await?;

            drop(launcher_proxy);

            let ls = recv.await?;

            assert_eq!(
                ls.handles,
                vec!(
                    HandleInfo::new(HandleType::LdsvcLoader, 0).as_raw(),
                    HandleInfo::new(HandleType::DefaultJob, 0).as_raw()
                )
            );

            Ok(())
        }

        #[fuchsia::test]
        async fn extra_handles() -> Result<(), Error> {
            let (launcher_proxy, recv) = start_launcher()?;

            let ns = setup_namespace(true, vec![])?;

            let mut handle_infos = vec![];
            for fd in 0..3 {
                let (client, _server) = create_endpoints::<fio::DirectoryMarker>();
                handle_infos.push(fproc::HandleInfo {
                    handle: client.into_channel().into_handle(),
                    id: fd,
                });
            }

            let _launch_info = configure_launcher(LauncherConfigArgs {
                bin_path: "bin/runner_lib_test",
                name: "name",
                args: None,
                options: zx::ProcessOptions::empty(),
                ns: ns,
                job: None,
                handle_infos: Some(handle_infos),
                name_infos: None,
                environs: None,
                launcher: &launcher_proxy,
                loader_proxy_chan: None,
                executable_vmo: None,
            })
            .await?;

            drop(launcher_proxy);

            let ls = recv.await?;

            assert_eq!(
                ls.handles,
                vec!(
                    0,
                    1,
                    2,
                    HandleInfo::new(HandleType::LdsvcLoader, 0).as_raw(),
                    HandleInfo::new(HandleType::DefaultJob, 0).as_raw(),
                )
            );

            Ok(())
        }
    }
}
