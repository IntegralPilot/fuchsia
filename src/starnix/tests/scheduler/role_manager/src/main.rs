// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::{EventStream, ExitStatus, Stopped};
use component_events::matcher::EventMatcher;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_component::{CreateChildArgs, RealmProxy};
use fidl_fuchsia_component_decl::{Child, CollectionRef, StartupMode};
use fidl_fuchsia_process as fprocess;
use fidl_fuchsia_scheduler::{
    RoleManagerMarker, RoleManagerRequest, RoleManagerRequestStream, RoleManagerSetRoleResponder,
    RoleManagerSetRoleResponse, RoleTarget,
};
use fuchsia_async::Task;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmBuilderParams, Ref, Route,
};
use fuchsia_runtime::{HandleInfo, HandleType};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use log::info;
use serde::Deserialize;
use std::collections::BTreeMap;

#[fuchsia::main]
async fn main() {
    let mut events = EventStream::open().await.unwrap();

    info!("reading package profile config");
    let profiles_config = std::fs::read_to_string("/pkg/config/profiles/starnix.profiles").unwrap();
    let mut profiles_config: ProfilesConfig = serde_json5::from_str(&profiles_config).unwrap();
    profiles_config
        .profiles
        .insert("first_custom_role".to_string(), ProfileConfig::Flexible { priority: 16 });
    profiles_config
        .profiles
        .insert("second_custom_role".to_string(), ProfileConfig::Flexible { priority: 16 });
    profiles_config
        .profiles
        .insert("cpuset_custom_role".to_string(), ProfileConfig::Flexible { priority: 16 });

    let (fake_manager, mut requests) = FakeRoleManager::new(profiles_config);

    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new().from_relative_url("#meta/test_realm.cm"),
    )
    .await
    .unwrap();

    let role_manager_ref = builder
        .add_local_child(
            "fake_role_manager",
            move |handles| Box::pin(fake_manager.clone().serve(handles)),
            ChildOptions::new(),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name(RoleManagerMarker::PROTOCOL_NAME))
                .from(&role_manager_ref)
                .to(Ref::child("kernel")),
        )
        .await
        .unwrap();

    info!("building realm and starting eager container");
    let realm = builder.build().await.unwrap();

    // initial kernel thread probes for profile provider (zircon 16)
    requests.with_next(|_, role| assert_eq!(role, "fuchsia.starnix.fair.16")).await;

    // init process' initial thread gets default (nice 0, zircon 16) when it starts
    requests.with_next(|_, role| assert_eq!(role, "fuchsia.starnix.fair.16")).await;

    // The puppet binary expects to receive line-delimited commands over its stdin before proceeding
    // with the different steps of the test. This synchronization limits the concurrency in the
    // test environment to produce more predictable and debuggable failures.
    let (stdin_recv, stdin_send) = zx::Socket::create_stream();

    info!("kernel and container init have requested thread roles, starting puppet");
    let test_realm: RealmProxy = realm.root.connect_to_protocol_at_exposed_dir().unwrap();
    test_realm
        .create_child(
            &CollectionRef { name: "puppets".to_string() },
            &Child {
                name: Some("puppet".to_string()),
                url: Some("#meta/puppet.cm".to_string()),
                startup: Some(StartupMode::Lazy),
                ..Default::default()
            },
            CreateChildArgs {
                numbered_handles: Some(vec![fprocess::HandleInfo {
                    id: HandleInfo::new(HandleType::FileDescriptor, 0).as_raw(),
                    handle: stdin_recv.into(),
                }]),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    let wait_for_puppet_exit = Task::spawn(async move {
        let puppet_stopped = EventMatcher::ok()
            .moniker_regex("realm_builder:.+/puppets:puppet")
            .wait::<Stopped>(&mut events)
            .await
            .unwrap();
        assert_eq!(
            puppet_stopped.result().unwrap().status,
            ExitStatus::Clean,
            "puppet must exit cleanly"
        );
    });

    // * puppet main process
    //   * thread one gets default (nice 0, zircon 16) when it starts, waits for control message
    //     * this thread requests an update (nice 10, zircon 11)
    //   * thread two in this process starts, inherits (nice 10, zircon 11)
    //     * this thread requests an update (nice 12, zircon 10)
    info!("waiting for initial puppet thread's creation");
    let puppet_thread_one_koid = requests
        .with_next(|koid, role| {
            assert_eq!(role, "fuchsia.starnix.fair.16");
            koid
        })
        .await;

    stdin_send.write(b"10\n").unwrap();
    info!("waiting for initial puppet thread's update");
    requests
        .with_next(|koid, role| {
            assert_eq!(koid, puppet_thread_one_koid);
            assert_eq!(role, "fuchsia.starnix.fair.11");
        })
        .await;

    stdin_send.write(b"thread\n").unwrap();
    info!("waiting for second puppet thread's creation");
    let puppet_thread_two_koid = requests
        .with_next(|koid, role| {
            assert_ne!(koid, puppet_thread_one_koid, "request must come from a different thread");
            assert_eq!(role, "fuchsia.starnix.fair.11");
            koid
        })
        .await;

    stdin_send.write(b"12\n").unwrap();
    info!("waiting for second puppet thread's update");
    requests
        .with_next(|koid, role| {
            assert_eq!(koid, puppet_thread_two_koid);
            assert_eq!(role, "fuchsia.starnix.fair.10");
        })
        .await;

    // * puppet child process
    //   * initial thread inherits main process' value (nice 10, zircon 11)
    //     * this thread requests an update (nice 14, zircon 9)
    //   * second thread inherit's main thread's value (nice 14, zircon 9)
    //     * this thread requests an update (nice 16, zircon 8)
    stdin_send.write(b"fork\n").unwrap();
    info!("waiting for child process initial thread creation");
    let puppet_child_thread_one_koid = requests
        .with_next(|koid, role| {
            assert_eq!(role, "fuchsia.starnix.fair.11");
            koid
        })
        .await;

    stdin_send.write(b"14\n").unwrap();
    info!("waiting for child process' initial thread update");
    requests
        .with_next(|koid, role| {
            assert_eq!(koid, puppet_child_thread_one_koid);
            assert_eq!(role, "fuchsia.starnix.fair.9");
        })
        .await;

    stdin_send.write(b"thread\n").unwrap();
    info!("waiting for child process' second thread creation");
    let puppet_child_thread_two_koid = requests
        .with_next(|koid, role| {
            assert_eq!(role, "fuchsia.starnix.fair.9");
            koid
        })
        .await;

    stdin_send.write(b"16\n").unwrap();
    info!("waiting for child process' second thread update");
    requests
        .with_next(|koid, role| {
            assert_eq!(koid, puppet_child_thread_two_koid);
            assert_eq!(role, "fuchsia.starnix.fair.8");
        })
        .await;

    stdin_send.write(b"cpuset\n").unwrap();
    info!("waiting for cpuset role update");
    requests
        .with_next(|koid, role| {
            assert_eq!(koid, puppet_thread_one_koid);
            assert_eq!(role, "cpuset_custom_role");
        })
        .await;

    stdin_send.write(b"thread-fifo\n").unwrap();
    info!("waiting for child process' first FIFO thread update");
    requests
        .with_next(|koid, role| {
            assert_eq!(koid, puppet_thread_one_koid);
            assert_eq!(role, "first_custom_role");
        })
        .await;

    info!("waiting for child process' second FIFO thread update");
    requests
        .with_next(|koid, role| {
            assert_eq!(koid, puppet_thread_one_koid);
            assert_eq!(role, "second_custom_role");
        })
        .await;

    info!("waiting for puppet to exit");
    wait_for_puppet_exit.await;
    realm.destroy().await.unwrap();
}

#[derive(Clone, Debug, Deserialize)]
struct ProfilesConfig {
    profiles: BTreeMap<String, ProfileConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum ProfileConfig {
    Flexible {
        #[allow(unused)]
        priority: u8,
    },
    Constant {
        #[allow(unused)]
        capacity: String,
        #[allow(unused)]
        deadline: Option<String>,
        #[allow(unused)]
        period: String,
    },
}

#[derive(Clone)]
struct FakeRoleManager {
    config: ProfilesConfig,
    sender: UnboundedSender<SetRole>,
}

/// Profiles which are used by the kernel but are not in starnix.profiles because they should be
/// defined by product integrators.
const IGNORED_PROFILES: &[&str] = &["fuchsia.starnix.kthread.input_relay"];

impl FakeRoleManager {
    fn new(config: ProfilesConfig) -> (Self, FakeProfileRequests) {
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        (Self { config, sender }, FakeProfileRequests { receiver })
    }

    async fn serve(self, handles: LocalComponentHandles) -> Result<(), anyhow::Error> {
        let mut fs = fuchsia_component::server::ServiceFs::new();
        fs.dir("svc").add_fidl_service(|client: RoleManagerRequestStream| client);
        fs.serve_connection(handles.outgoing_dir).unwrap();

        while let Some(mut client) = fs.next().await {
            while let Some(request) = client.next().await {
                match request.unwrap() {
                    RoleManagerRequest::SetRole { payload, responder } => {
                        let role_name = payload.role.unwrap().role;
                        if IGNORED_PROFILES.contains(&role_name.as_str()) {
                            info!(role_name:%; "ignoring role request");
                            continue;
                        }
                        let thread = match payload.target.unwrap() {
                            RoleTarget::Thread(t) => t,
                            other => panic!("unexpected request {other:?} for role {role_name}"),
                        };
                        assert!(
                            self.config.profiles.contains_key(&role_name),
                            "requested={role_name} allowed={:#?}",
                            self.config.profiles,
                        );

                        self.sender
                            .unbounded_send(SetRole {
                                thread: thread,
                                role: role_name,
                                responder: responder,
                            })
                            .unwrap();
                    }
                    RoleManagerRequest::GetProfileForRole { payload: _, responder } => {
                        responder.send(Err(zx::Status::NOT_SUPPORTED.into_raw())).unwrap();
                    }
                    other => {
                        panic!("unexpected RoleManager request from starnix kernel {other:?}")
                    }
                }
            }
        }

        Ok(())
    }
}

struct FakeProfileRequests {
    receiver: UnboundedReceiver<SetRole>,
}

impl FakeProfileRequests {
    async fn with_next<R>(&mut self, op: impl FnOnce(zx::Koid, &str) -> R) -> R {
        let next = self.receiver.next().await.unwrap();
        let ret = op(next.thread.koid().unwrap(), &next.role);
        let response = RoleManagerSetRoleResponse { ..Default::default() };
        next.responder.send(Ok(response)).unwrap();
        ret
    }
}

#[derive(Debug)]
struct SetRole {
    thread: zx::Thread,
    role: String,
    responder: RoleManagerSetRoleResponder,
}
