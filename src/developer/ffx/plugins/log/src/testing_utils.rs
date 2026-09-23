// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_data::{BuilderArgs, LogsData, LogsDataBuilder, Severity, Timestamp};
use fdomain_client::fidl::DiscoverableProtocolMarker as _;
use ffx_config::EnvironmentContext;
use fho::{FhoEnvironment, TryFromEnv};
use fidl::endpoints::{DiscoverableProtocolMarker as _, Responder as _};
use fidl_fuchsia_developer_remotecontrol::{
    ConnectCapabilityError, IdentifyHostError, IdentifyHostResponse, RemoteControlMarker,
    RemoteControlRequest,
};
use fidl_fuchsia_diagnostics::{
    LogInterestSelector, LogSettingsMarker, LogSettingsRequest, LogSettingsRequestStream,
    StreamMode,
};
use fidl_fuchsia_diagnostics_host::{
    ArchiveAccessorMarker, ArchiveAccessorRequest, ArchiveAccessorRequestStream,
};
use fidl_fuchsia_sys2 as fsys;
use fuchsia_async as fasync;
use futures::channel::{mpsc, oneshot};
use futures::{AsyncWriteExt as _, Stream, StreamExt, TryStreamExt};
use log_command_fdomain::parse_utc_time;
use moniker::Moniker;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use target_behavior::ConnectionBehavior;
use target_connector::Connector;
use target_holders::RemoteControlProxyHolder;

const NODENAME: &str = "Rust";

/// Test configuration
pub struct TestEnvironmentConfig {
    pub messages: Vec<LogsData>,
    pub boot_timestamp: u64,
    pub boot_id: Option<u64>,
    pub instances: Vec<Moniker>,
    pub send_connected_event: bool,
    pub show_initial_timestamp: bool,
    pub fail_device_connection: bool,
    pub hang_device_connection: bool,
}

pub fn test_log_with_severity(timestamp: i64, severity: Severity) -> LogsData {
    LogsDataBuilder::new(BuilderArgs {
        component_url: Some("ffx".into()),
        moniker: "host/ffx".try_into().unwrap(),
        severity,
        timestamp: Timestamp::from_nanos(timestamp),
    })
    .set_pid(1)
    .set_tid(2)
    .set_message("Hello world!")
    .build()
}

pub fn test_log(timestamp: i64) -> LogsData {
    LogsDataBuilder::new(BuilderArgs {
        component_url: Some("ffx".into()),
        moniker: "host/ffx".try_into().unwrap(),
        severity: Severity::Info,
        timestamp: Timestamp::from_nanos(timestamp),
    })
    .set_pid(1)
    .set_tid(2)
    .set_message("Hello world!")
    .build()
}

pub fn test_log_with_file(timestamp: i64) -> LogsData {
    LogsDataBuilder::new(BuilderArgs {
        component_url: Some("ffx".into()),
        moniker: "host/ffx".try_into().unwrap(),
        severity: Severity::Info,
        timestamp: Timestamp::from_nanos(timestamp),
    })
    .set_file("test_filename.cc")
    .set_line(42)
    .add_tag("test tag")
    .set_pid(1)
    .set_tid(2)
    .set_message("Hello world!")
    .build()
}

pub fn test_log_with_tag(timestamp: i64) -> LogsData {
    LogsDataBuilder::new(BuilderArgs {
        component_url: Some("ffx".into()),
        moniker: "host/ffx".try_into().unwrap(),
        severity: Severity::Info,
        timestamp: Timestamp::from_nanos(timestamp),
    })
    .add_tag("test tag")
    .set_pid(1)
    .set_tid(2)
    .set_message("Hello world!")
    .build()
}

pub fn naive_utc_nanos(utc_time: &str) -> i64 {
    parse_utc_time(utc_time).unwrap().time.naive_utc().and_utc().timestamp_nanos_opt().unwrap()
}

impl Default for TestEnvironmentConfig {
    fn default() -> Self {
        Self {
            messages: vec![test_log(0)],
            boot_timestamp: 1,
            instances: Vec::new(),
            send_connected_event: false,
            boot_id: Some(1),
            show_initial_timestamp: false,
            fail_device_connection: false,
            hang_device_connection: false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum TestEvent {
    Connected(StreamMode),
    SetInterest(Vec<LogInterestSelector>),
    LogSettingsClosed,
}

pub struct TestEnvironment {
    fho_env: FhoEnvironment,
    state: Rc<State>,
    event_rcv: Option<mpsc::UnboundedReceiver<TestEvent>>,
    disconnect_snd: oneshot::Sender<()>,
}

impl TestEnvironment {
    pub async fn new(config: TestEnvironmentConfig) -> Self {
        let (event_snd, event_rcv) = mpsc::unbounded();
        let (disconnect_snd, disconnect_rcv) = oneshot::channel();

        let (open_snd, mut open_rcv) = mpsc::unbounded();
        let client = fdomain_local::local_client(move || {
            let (client_end, dir_stream) =
                fidl::endpoints::create_request_stream::<fidl_fuchsia_io::DirectoryMarker>();
            open_snd.unbounded_send(dir_stream).unwrap();
            Ok(client_end)
        });

        let state = Rc::new(State::new(config, event_snd, disconnect_rcv));

        let state_clone = state.clone();
        fasync::Task::local(async move {
            while let Some(mut dir_stream) = open_rcv.next().await {
                let state = state_clone.clone();
                fasync::Task::local(async move {
                    while let Ok(Some(req)) = dir_stream.try_next().await {
                        if let fidl_fuchsia_io::DirectoryRequest::Open { path, object, .. } = req {
                            let path = path.strip_prefix("./").unwrap_or(&path);
                            let path = path.strip_prefix("svc/").unwrap_or(path);
                            if path == RemoteControlMarker::PROTOCOL_NAME
                                || path
                                    == fdomain_fuchsia_developer_remotecontrol::RemoteControlMarker::PROTOCOL_NAME
                            {
                                let server_end =
                                    fidl::endpoints::ServerEnd::<RemoteControlMarker>::new(object);
                                let state = state.clone();
                                fasync::Task::local(async move {
                                    let mut stream = server_end.into_stream();
                                    while let Ok(Some(req)) = stream.try_next().await {
                                        handle_remote_control_fidl(req, state.clone()).await;
                                    }
                                })
                                .detach();
                            }
                        }
                    }
                })
                .detach();
            }
        })
        .detach();

        let fho_env = FhoEnvironment::new_with_args(
            &ffx_config::EnvironmentContext::no_context(
                ffx_config::environment::ExecutableKind::Test,
                Default::default(),
                None,
                true,
            )
            .unwrap(),
            &["some", "test"],
        );
        let target_env = target_behavior::target_interface(&fho_env);
        let conn = ffx_target::Connection::from_fdomain_client(client.clone());
        let resolution = ffx_target::Resolution::mock(|| unreachable!());
        resolution.set_connection_for_test(Some(conn)).await;
        let behavior = ConnectionBehavior::fake_direct_connector(resolution);
        target_env.set_behavior_for_test(behavior);

        Self { fho_env, state, event_rcv: Some(event_rcv), disconnect_snd }
    }

    pub fn take_event_stream(&mut self) -> Option<impl Stream<Item = TestEvent> + use<>> {
        self.event_rcv.take()
    }

    pub async fn rcs_connector(&self) -> Connector<RemoteControlProxyHolder> {
        Connector::try_from_env(&self.fho_env).await.expect("Could not make test connector")
    }

    pub fn reboot_target(&mut self, new_boot_id: Option<u64>) {
        self.state.mutable.borrow_mut().boot_id = new_boot_id;
        self.disconnect_target();
    }

    pub fn set_boot_timestamp(&mut self, new_boot_timestamp: u64) {
        self.state.mutable.borrow_mut().boot_timestamp = new_boot_timestamp;
    }

    pub fn set_fail_device_connection(&mut self, fail: bool) {
        self.state.mutable.borrow_mut().fail_device_connection = fail;
    }

    pub fn disconnect_target(&mut self) {
        let mut mutable_state = self.state.mutable.borrow_mut();
        // This must have already been taken and is been awaited on.
        assert!(mutable_state.disconnect_rcv.is_none());
        let (snd, rcv) = oneshot::channel();
        let disconnect_snd = std::mem::replace(&mut self.disconnect_snd, snd);
        let _ = disconnect_snd.send(());
        mutable_state.disconnect_rcv = Some(rcv);
    }

    pub fn environment_context(&self) -> EnvironmentContext {
        self.fho_env.environment_context().clone()
    }
}

struct State {
    messages: Vec<LogsData>,
    instances: Vec<Moniker>,
    send_connected_event: bool,
    event_snd: mpsc::UnboundedSender<TestEvent>,
    mutable: RefCell<MutableState>,
}

impl State {
    fn new(
        config: TestEnvironmentConfig,
        snd: mpsc::UnboundedSender<TestEvent>,
        disconnect_rcv: oneshot::Receiver<()>,
    ) -> Self {
        Self {
            messages: config.messages,
            instances: config.instances,
            send_connected_event: config.send_connected_event,
            event_snd: snd,
            mutable: RefCell::new(MutableState {
                boot_timestamp: config.boot_timestamp,
                boot_id: config.boot_id,
                fail_device_connection: config.fail_device_connection,
                hang_device_connection: config.hang_device_connection,
                disconnect_rcv: Some(disconnect_rcv),
            }),
        }
    }
}

struct MutableState {
    boot_timestamp: u64,
    boot_id: Option<u64>,
    fail_device_connection: bool,
    hang_device_connection: bool,
    disconnect_rcv: Option<oneshot::Receiver<()>>,
}

async fn handle_remote_control_fidl(req: RemoteControlRequest, state: Rc<State>) {
    match req {
        RemoteControlRequest::IdentifyHost { responder } => {
            let hang_device_connection = state.mutable.borrow().hang_device_connection;
            let fail_device_connection = state.mutable.borrow().fail_device_connection;
            if hang_device_connection {
                // Hang indefinitely!
                responder.drop_without_shutdown();
            } else if fail_device_connection {
                let _ = responder.send(Err(IdentifyHostError::ProxyConnectionFailed));
            } else {
                let _ = responder.send(Ok(&IdentifyHostResponse {
                    nodename: Some(NODENAME.into()),
                    boot_timestamp_nanos: Some(state.mutable.borrow().boot_timestamp),
                    boot_id: state.mutable.borrow().boot_id,
                    ..Default::default()
                }));
            }
        }
        RemoteControlRequest::ConnectCapability {
            moniker: _,
            capability_set: _,
            capability_name,
            server_channel,
            responder,
        } => match capability_name.strip_prefix("svc/").unwrap_or(&capability_name) {
            "fuchsia.sys2.RealmQuery.root" | fsys::RealmQueryMarker::PROTOCOL_NAME => {
                let instances = state
                    .instances
                    .iter()
                    .map(|m| fsys::Instance {
                        moniker: Some(m.to_string()),
                        url: Some("fuchsia-pkg://test".into()),
                        ..Default::default()
                    })
                    .collect();
                let server_end =
                    fidl::endpoints::ServerEnd::<fsys::RealmQueryMarker>::new(server_channel);
                fasync::Task::local(async move {
                    handle_realm_query(instances, server_end).await;
                })
                .detach();
                let _ = responder.send(Ok(()));
            }
            ArchiveAccessorMarker::PROTOCOL_NAME => {
                let stream =
                    fidl::endpoints::ServerEnd::<ArchiveAccessorMarker>::new(server_channel)
                        .into_stream();
                let state = state.clone();
                fasync::Task::local(async move {
                    handle_archive_accessor(stream, state).await;
                })
                .detach();
                let _ = responder.send(Ok(()));
            }
            LogSettingsMarker::PROTOCOL_NAME => {
                let stream = fidl::endpoints::ServerEnd::<LogSettingsMarker>::new(server_channel)
                    .into_stream();
                let state = state.clone();
                fasync::Task::local(async move {
                    handle_log_settings(stream, state).await;
                })
                .detach();
                let _ = responder.send(Ok(()));
            }
            _ => {
                let _ = responder.send(Err(ConnectCapabilityError::NoMatchingCapabilities));
            }
        },
        _ => {}
    }
}

async fn handle_realm_query(
    instances: Vec<fsys::Instance>,
    server_end: fidl::endpoints::ServerEnd<fsys::RealmQueryMarker>,
) {
    let mut stream = server_end.into_stream();
    let mut instance_map = HashMap::new();
    for instance in instances {
        let moniker = Moniker::parse_str(instance.moniker.as_ref().unwrap()).unwrap();
        let previous = instance_map.insert(moniker.to_string(), instance);
        assert!(previous.is_none());
    }

    while let Some(Ok(request)) = stream.next().await {
        match request {
            fsys::RealmQueryRequest::GetInstance { moniker, responder } => {
                let moniker = Moniker::parse_str(&moniker).unwrap().to_string();
                if let Some(instance) = instance_map.get(&moniker) {
                    let _ = responder.send(Ok(instance));
                } else {
                    let _ = responder.send(Err(fsys::GetInstanceError::InstanceNotFound));
                }
            }
            fsys::RealmQueryRequest::GetAllInstances { responder } => {
                let instances = instance_map.values().cloned().collect();
                let iterator = serve_instance_iterator(instances);
                let _ = responder.send(Ok(iterator));
            }
            _ => panic!("Unexpected RealmQuery request"),
        }
    }
}

fn serve_instance_iterator(
    instances: Vec<fsys::Instance>,
) -> fidl::endpoints::ClientEnd<fsys::InstanceIteratorMarker> {
    let (client, mut stream) =
        fidl::endpoints::create_request_stream::<fsys::InstanceIteratorMarker>();
    fasync::Task::local(async move {
        let Some(Ok(fsys::InstanceIteratorRequest::Next { responder })) = stream.next().await
        else {
            return;
        };
        let _ = responder.send(&instances);
        let Some(Ok(fsys::InstanceIteratorRequest::Next { responder })) = stream.next().await
        else {
            return;
        };
        let _ = responder.send(&[]);
    })
    .detach();
    client
}

async fn handle_archive_accessor(mut stream: ArchiveAccessorRequestStream, state: Rc<State>) {
    while let Some(Ok(ArchiveAccessorRequest::StreamDiagnostics {
        parameters,
        stream,
        responder,
    })) = stream.next().await
    {
        if state.send_connected_event {
            let _ = state
                .event_snd
                .unbounded_send(TestEvent::Connected(parameters.stream_mode.unwrap()));
        }
        let _ = responder.send();
        let mut socket = fasync::Socket::from_socket(stream);
        let _ = socket.write_all(serde_json::to_string(&state.messages).unwrap().as_bytes()).await;

        match parameters.stream_mode.unwrap() {
            StreamMode::Snapshot => {}
            StreamMode::SnapshotThenSubscribe | StreamMode::Subscribe => {
                let rcv = state.mutable.borrow_mut().disconnect_rcv.take();
                if let Some(rcv) = rcv {
                    let _ = rcv.await;
                }
            }
        }
    }
}

async fn handle_log_settings(mut stream: LogSettingsRequestStream, state: Rc<State>) {
    while let Some(Ok(request)) = stream.next().await {
        match request {
            LogSettingsRequest::SetComponentInterest { payload, responder } => {
                let _ = state
                    .event_snd
                    .unbounded_send(TestEvent::SetInterest(payload.selectors.unwrap_or_default()));
                responder.send().unwrap();
            }
        }
    }
    let _ = state.event_snd.unbounded_send(TestEvent::LogSettingsClosed);
}
