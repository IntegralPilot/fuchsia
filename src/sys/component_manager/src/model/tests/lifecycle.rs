// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtin_environment::BuiltinEnvironment;
use crate::model::actions::{
    ActionsManager, ShutdownAction, ShutdownType, StartAction, StopAction,
};
use crate::model::component::instance::InstanceState;
use crate::model::component::{ComponentInstance, IncomingCapabilities, StartReason};
use crate::model::model::Model;
use crate::model::start::Start;
use crate::model::testing::mocks::*;
use crate::model::testing::out_dir::OutDir;
use crate::model::testing::routing_test_helpers::RoutingTestBuilder;
use crate::model::testing::test_helpers::*;
use crate::model::testing::test_hook::{Lifecycle, TestHook};
use ::routing::policy::PolicyError;
use assert_matches::assert_matches;
use async_trait::async_trait;
use cm_config::AllowlistEntryBuilder;
use cm_rust::{ComponentDecl, RegistrationSource, RunnerRegistration};
use cm_rust_testing::*;
use errors::{ActionErrorKind, ModelError, StartActionError};
use fidl::endpoints::{ProtocolMarker, ServerEnd, create_endpoints};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_hardware_power_statecontrol as fstatecontrol;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_sync as fsync;
use futures::channel::mpsc;
use futures::future::pending;
use futures::join;
use futures::lock::Mutex;
use futures::prelude::*;
use hooks::{Event, EventType, Hook, HooksRegistration};
use moniker::{ChildName, Moniker};
use std::collections::HashSet;
use std::sync::{Arc, Weak};
use test_case::test_case;
use vfs::ToObjectRequest;
use vfs::directory::entry::OpenRequest;
use vfs::execution_scope::ExecutionScope;
use zx::AsHandleRef;

async fn new_model(
    components: Vec<(&'static str, ComponentDecl)>,
) -> (Arc<Model>, Arc<Mutex<BuiltinEnvironment>>, Arc<MockRunner>) {
    new_model_with(components, vec![]).await
}

async fn new_model_with(
    components: Vec<(&'static str, ComponentDecl)>,
    additional_hooks: Vec<HooksRegistration>,
) -> (Arc<Model>, Arc<Mutex<BuiltinEnvironment>>, Arc<MockRunner>) {
    let TestModelResult { model, builtin_environment, mock_runner, .. } =
        TestEnvironmentBuilder::new()
            .set_components(components)
            .set_hooks(additional_hooks)
            .build()
            .await;
    (model, builtin_environment, mock_runner)
}

#[fuchsia::test]
async fn bind_root() {
    let (model, _builtin_environment, mock_runner) =
        new_model(vec![("root", component_decl_with_test_runner())]).await;
    let res = model.root().start_instance(&Moniker::root(), &StartReason::Root).await;
    assert!(res.is_ok());
    mock_runner.wait_for_url("test:///root").await;
    let actual_children = get_live_children(&model.root()).await;
    assert!(actual_children.is_empty());
}

#[fuchsia::test]
async fn bind_non_existent_root_child() {
    let (model, _builtin_environment, _mock_runner) =
        new_model(vec![("root", component_decl_with_test_runner())]).await;
    let m: Moniker = ["no-such-instance"].try_into().unwrap();
    let res = model.root().start_instance(&m, &StartReason::Root).await;
    let expected_res: Result<Arc<ComponentInstance>, ModelError> =
        Err(ModelError::instance_not_found(["no-such-instance"].try_into().unwrap()));
    assert_eq!(format!("{:?}", res), format!("{:?}", expected_res));
}

// Blocks the Start action for the "system" component
pub struct StartBlocker {
    rx: Mutex<mpsc::Receiver<()>>,
}

impl StartBlocker {
    pub fn new() -> (Arc<Self>, mpsc::Sender<()>) {
        let (tx, rx) = mpsc::channel::<()>(0);
        let blocker = Arc::new(Self { rx: Mutex::new(rx) });
        (blocker, tx)
    }
}

#[async_trait]
impl Hook for StartBlocker {
    async fn on(self: Arc<Self>, event: &Event) -> Result<(), ModelError> {
        let moniker = event
            .target_moniker
            .unwrap_instance_moniker_or(ModelError::UnexpectedComponentManagerMoniker)
            .unwrap();
        let expected_moniker: Moniker = ["system"].try_into().unwrap();
        if moniker == &expected_moniker {
            let mut rx = self.rx.lock().await;
            rx.next().await.unwrap();
        }
        Ok(())
    }
}

#[fuchsia::test]
async fn bind_concurrent() {
    // Test binding twice concurrently to the same component. The component should only be
    // started once.
    let (blocker, mut unblocker) = StartBlocker::new();
    let (model, _builtin_environment, mock_runner) = new_model_with(
        vec![
            ("root", ComponentDeclBuilder::new().child_default("system").build()),
            ("system", component_decl_with_test_runner()),
        ],
        vec![HooksRegistration::new(
            "start_blocker",
            vec![EventType::Started],
            Arc::downgrade(&blocker) as Weak<dyn Hook>,
        )],
    )
    .await;

    // Start the root component.
    model.start().await;

    // Attempt to start the "system" component
    let system_component = model.root().find(&["system"].try_into().unwrap()).await.unwrap();
    let first_start = system_component
        .actions()
        .register_no_wait(StartAction::new(
            StartReason::Debug,
            None,
            IncomingCapabilities::default(),
        ))
        .await;

    // While the first start is paused, simulate a second start by explicitly scheduling a second
    // Start action. This should just be deduplicated to the first start by the action system.
    let second_start = system_component
        .actions()
        .register_no_wait(StartAction::new(
            StartReason::Debug,
            None,
            IncomingCapabilities::default(),
        ))
        .await;

    // Unblock the start hook, then check the result of both starts.
    unblocker.try_send(()).unwrap();

    // The first and second start results must both be successful.
    first_start.await.expect("first start failed");
    second_start.await.expect("second start failed");

    // Verify that the component was started only once.
    mock_runner.wait_for_urls(&["test:///system"]).await;
}

#[fuchsia::test]
async fn bind_parent_then_child() {
    let hook = Arc::new(TestHook::new());
    let (model, _builtin_environment, mock_runner) = new_model_with(
        vec![
            (
                "root",
                ComponentDeclBuilder::new().child_default("system").child_default("echo").build(),
            ),
            ("system", component_decl_with_test_runner()),
            ("echo", component_decl_with_test_runner()),
        ],
        hook.hooks(),
    )
    .await;
    let root = model.root();

    // Start the system.
    let m: Moniker = ["system"].try_into().unwrap();
    assert!(root.start_instance(&m, &StartReason::Root).await.is_ok());
    mock_runner.wait_for_urls(&["test:///system"]).await;

    // Validate children. system is resolved, but not echo.
    let actual_children = get_live_children(&*root).await;
    let mut expected_children: HashSet<ChildName> = HashSet::new();
    expected_children.insert("system".try_into().unwrap());
    expected_children.insert("echo".try_into().unwrap());
    assert_eq!(actual_children, expected_children);

    let system_component = get_live_child(&*root, "system").await;
    let echo_component = get_live_child(&*root, "echo").await;
    let actual_children = get_live_children(&*system_component).await;
    assert!(actual_children.is_empty());
    assert_matches!(*echo_component.lock_state().await, InstanceState::Unresolved(_));
    // Start echo.
    let m: Moniker = ["echo"].try_into().unwrap();
    assert!(root.start_instance(&m, &StartReason::Root).await.is_ok());
    mock_runner.wait_for_urls(&["test:///system", "test:///echo"]).await;

    // Validate children. Now echo is resolved.
    let echo_component = get_live_child(&*root, "echo").await;
    let actual_children = get_live_children(&*echo_component).await;
    assert!(actual_children.is_empty());

    // Verify that the component topology matches expectations.
    assert_eq!("(echo,system)", hook.print());
}

#[fuchsia::test]
async fn bind_child_doesnt_bind_parent() {
    let hook = Arc::new(TestHook::new());
    let (model, _builtin_environment, mock_runner) = new_model_with(
        vec![
            ("root", ComponentDeclBuilder::new().child_default("system").build()),
            (
                "system",
                ComponentDeclBuilder::new()
                    .child_default("logger")
                    .child_default("netstack")
                    .build(),
            ),
            ("logger", component_decl_with_test_runner()),
            ("netstack", component_decl_with_test_runner()),
        ],
        hook.hooks(),
    )
    .await;
    let root = model.root();

    // Start logger (before ever starting system).
    let m: Moniker = ["system", "logger"].try_into().unwrap();
    assert!(root.start_instance(&m, &StartReason::Root).await.is_ok());
    mock_runner.wait_for_urls(&["test:///logger"]).await;

    // Start netstack.
    let m: Moniker = ["system", "netstack"].try_into().unwrap();
    assert!(root.start_instance(&m, &StartReason::Root).await.is_ok());
    mock_runner.wait_for_urls(&["test:///logger", "test:///netstack"]).await;

    // Finally, start the system.
    let m: Moniker = ["system"].try_into().unwrap();
    assert!(root.start_instance(&m, &StartReason::Root).await.is_ok());
    mock_runner.wait_for_urls(&["test:///system", "test:///logger", "test:///netstack"]).await;

    // validate the component topology.
    assert_eq!("(system(logger,netstack))", hook.print());
}

#[fuchsia::test]
async fn bind_child_non_existent() {
    let (model, _builtin_environment, mock_runner) = new_model(vec![
        ("root", ComponentDeclBuilder::new().child_default("system").build()),
        ("system", component_decl_with_test_runner()),
    ])
    .await;
    let root = model.root();

    // Start the system.
    let m: Moniker = ["system"].try_into().unwrap();
    assert!(root.start_instance(&m, &StartReason::Root).await.is_ok());
    mock_runner.wait_for_urls(&["test:///system"]).await;

    // Can't start the logger. It does not exist.
    let m: Moniker = ["system", "logger"].try_into().unwrap();
    let res = root.start_instance(&m, &StartReason::Root).await;
    let expected_res: Result<(), ModelError> = Err(ModelError::instance_not_found(m));
    assert_eq!(format!("{:?}", res), format!("{:?}", expected_res));
    mock_runner.wait_for_urls(&["test:///system"]).await;
}

/// Create a hierarchy of children:
///
///   a
///  / \
/// b   c
///      \
///       d
///        \
///         e
///
/// `b`, `c`, and `d` are started eagerly. `a` and `e` are lazy.
#[fuchsia::test]
async fn bind_eager_children() {
    let hook = Arc::new(TestHook::new());
    let (model, _builtin_environment, mock_runner) = new_model_with(
        vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            (
                "a",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("b").eager())
                    .child(ChildBuilder::new().name("c").eager())
                    .build(),
            ),
            ("b", component_decl_with_test_runner()),
            ("c", ComponentDeclBuilder::new().child(ChildBuilder::new().name("d").eager()).build()),
            ("d", ComponentDeclBuilder::new().child_default("e").build()),
            ("e", component_decl_with_test_runner()),
        ],
        hook.hooks(),
    )
    .await;

    // Start the top component, and check that it and the eager components were started.
    {
        let m = Moniker::parse_str("/a").unwrap();
        let res = model.root().start_instance(&m, &StartReason::Root).await;
        assert!(res.is_ok());
        mock_runner.wait_for_urls(&["test:///a", "test:///b", "test:///c", "test:///d"]).await;
    }
    // Verify that the topology of started components matches expectations.
    assert_eq!("(a(b,c(d)))", hook.print());
}

/// `b` is an eager child of `a` that uses a runner provided by `a`. In the process of binding
/// to `a`, `b` will be eagerly started, which requires re-binding to `a`. This should work
/// without causing reentrance issues.
#[fuchsia::test]
async fn bind_eager_children_reentrant() {
    let hook = Arc::new(TestHook::new());
    let (model, _builtin_environment, mock_runner) = new_model_with(
        vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            (
                "a",
                ComponentDeclBuilder::new()
                    .child(
                        ChildBuilder::new()
                            .name("b")
                            .url("test:///b")
                            .startup(fdecl::StartupMode::Eager)
                            .environment("env"),
                    )
                    .runner_default("foo")
                    .environment(EnvironmentBuilder::new().name("env").runner(RunnerRegistration {
                        source_name: "foo".parse().unwrap(),
                        source: RegistrationSource::Self_,
                        target_name: "foo".parse().unwrap(),
                    }))
                    .build(),
            ),
            ("b", ComponentDeclBuilder::new_empty_component().program_runner("foo").build()),
        ],
        hook.hooks(),
    )
    .await;

    // Set up the runner.
    let (runner_service, mut receiver) =
        create_service_directory_entry::<fcrunner::ComponentRunnerMarker>();
    let mut out_dir = OutDir::new();
    out_dir.add_entry(
        format!("/svc/{}", fcrunner::ComponentRunnerMarker::DEBUG_NAME).parse().unwrap(),
        runner_service,
    );
    mock_runner.add_host_fn("test:///a", out_dir.host_fn());

    // Start the top component, and check that it and the eager components were started.
    {
        let (f, bind_handle) = async move {
            let m = Moniker::parse_str("/a").unwrap();
            model.root().start_instance(&m, &StartReason::Root).await
        }
        .remote_handle();
        fasync::Task::spawn(f).detach();
        // `b` uses the runner offered by `a`.
        assert_eq!(
            wait_for_runner_request(&mut receiver).await.resolved_url,
            Some("test:///b".to_string())
        );
        bind_handle.await.expect("start `a` failed");
        // `root` and `a` use the test runner.
        mock_runner.wait_for_urls(&["test:///a"]).await;
    }
    // Verify that the topology of started components matches expectations.
    assert_eq!("(a(b))", hook.print());
}

#[fuchsia::test]
async fn bind_no_execute() {
    // Create a non-executable component with an eagerly-started child.
    let (model, _builtin_environment, mock_runner) = new_model(vec![
        ("root", ComponentDeclBuilder::new().child_default("a").build()),
        (
            "a",
            ComponentDeclBuilder::new_empty_component()
                .child(ChildBuilder::new().name("b").eager())
                .build(),
        ),
        ("b", component_decl_with_test_runner()),
    ])
    .await;

    // Start the parent component. The child should be started. However, the parent component
    // is non-executable so it is not run.
    let m: Moniker = ["a"].try_into().unwrap();
    assert!(model.root().start_instance(&m, &StartReason::Root).await.is_ok());
    mock_runner.wait_for_urls(&["test:///b"]).await;
}

#[fuchsia::test]
async fn bind_action_sequence() {
    // Test that binding registers the expected actions in the expected sequence
    // (Discover -> Resolve -> Start).

    // Set up the tree.
    let (model, builtin_environment, _mock_runner) = new_model(vec![
        ("root", ComponentDeclBuilder::new().child_default("system").build()),
        ("system", component_decl_with_test_runner()),
    ])
    .await;
    let event_stream = new_event_stream(
        &*builtin_environment.lock().await,
        vec![EventType::Resolved, EventType::Started],
    )
    .await;

    // Child of root should start out discovered but not resolved yet.
    let m = Moniker::parse_str("/system").unwrap();
    model.start().await;
    let events = get_n_events(&event_stream, 2).await;
    assert_event_type_and_moniker(&events[0], fcomponent::EventType::Resolved, Moniker::root());
    assert_event_type_and_moniker(&events[1], fcomponent::EventType::Started, Moniker::root());

    // Start child and check that it gets resolved, with a Resolve event and action.
    model.root().start_instance(&m, &StartReason::Root).await.unwrap();
    let events = get_n_events(&event_stream, 2).await;
    assert_event_type_and_moniker(&events[0], fcomponent::EventType::Resolved, &m);
    // Check that the child is started, with a Start event and action.
    assert_event_type_and_moniker(&events[1], fcomponent::EventType::Started, &m);
}

#[fuchsia::test]
async fn reboot_on_terminate_disallowed() {
    let components = vec![
        (
            "root",
            ComponentDeclBuilder::new()
                .child(ChildBuilder::new().name("system").on_terminate(fdecl::OnTerminate::Reboot))
                .build(),
        ),
        ("system", ComponentDeclBuilder::new().build()),
    ];
    let test = RoutingTestBuilder::new("root", components)
        .set_reboot_on_terminate_policy(vec![AllowlistEntryBuilder::new().exact("other").build()])
        .build()
        .await;

    let res = test
        .model
        .root()
        .start_instance(&["system"].try_into().unwrap(), &StartReason::Debug)
        .await;
    let expected_moniker = Moniker::try_from(["system"]).unwrap();
    assert_matches!(res, Err(ModelError::ActionError { err }) => {
        assert_matches!(err.kind(), ActionErrorKind::StartError {
            err: StartActionError::RebootOnTerminateForbidden {
                err: PolicyError::ChildPolicyDisallowed {
                    policy,
                    moniker: m2
                },
                moniker: m1
            }
        } if policy == "reboot_on_terminate" && m1 == &expected_moniker && m2 == &expected_moniker);
    });
}

const REBOOT_PROTOCOL: &str = fstatecontrol::AdminMarker::DEBUG_NAME;

#[fuchsia::test]
async fn on_terminate_stop_triggers_reboot() {
    // Create a topology with a reboot-on-terminate component and a fake reboot protocol
    let reboot_protocol_path = format!("/svc/{}", REBOOT_PROTOCOL);
    let components = vec![
        (
            "root",
            ComponentDeclBuilder::new()
                .child(ChildBuilder::new().name("system").on_terminate(fdecl::OnTerminate::Reboot))
                .protocol_default(REBOOT_PROTOCOL)
                .expose(
                    ExposeBuilder::protocol()
                        .name(REBOOT_PROTOCOL)
                        .source(cm_rust::ExposeSource::Self_),
                )
                .build(),
        ),
        ("system", ComponentDeclBuilder::new().build()),
    ];
    let (reboot_service, mut receiver) =
        create_service_directory_entry::<fstatecontrol::AdminMarker>();
    let test = RoutingTestBuilder::new("root", components)
        .set_reboot_on_terminate_policy(vec![AllowlistEntryBuilder::new().exact("system").build()])
        .add_outgoing_path("root", reboot_protocol_path.parse().unwrap(), reboot_service)
        .build()
        .await;
    let root = test.model.root();

    // First stop of the critical component exits cleanly and so does not trigger a reboot.
    test.mock_runner.add_controller_response(
        "test:///system",
        Box::new(|| ControllerActionResponse {
            close_channel: true,
            delay: None,
            termination_status: Some(Ok(())),
            exit_code: Some(0),
        }),
    );
    root.start_instance(&["system"].try_into().unwrap(), &StartReason::Debug).await.unwrap();
    let component = root.find_and_maybe_resolve(&["system"].try_into().unwrap()).await.unwrap();
    ActionsManager::register(component.clone(), StopAction::new(false)).await.unwrap();
    assert!(!test.model.top_instance().has_reboot_task());

    // Second stop of the critical component does not exit cleanly and so does trigger a reboot.
    test.mock_runner.add_controller_response(
        "test:///system",
        Box::new(|| ControllerActionResponse {
            close_channel: true,
            delay: None,
            termination_status: Some(Ok(())),
            exit_code: Some(1),
        }),
    );
    root.start_instance(&["system"].try_into().unwrap(), &StartReason::Debug).await.unwrap();
    let component = root.find_and_maybe_resolve(&["system"].try_into().unwrap()).await.unwrap();
    let stop = async move {
        ActionsManager::register(component.clone(), StopAction::new(false)).await.unwrap();
    };
    let recv_reboot = async move {
        let options = match receiver.next().await.unwrap() {
            fstatecontrol::AdminRequest::Shutdown {
                options:
                    options @ fstatecontrol::ShutdownOptions {
                        action: Some(_), reasons: Some(_), ..
                    },
                ..
            } => options,
            _ => panic!("unexpected request"),
        };
        assert_matches!(&options.action, Some(fstatecontrol::ShutdownAction::Reboot));
        assert_matches!(
            options.reasons.as_ref().unwrap().as_slice(),
            [fstatecontrol::ShutdownReason::CriticalComponentFailure]
        );
    };
    join!(stop, recv_reboot);
    assert!(test.model.top_instance().has_reboot_task());
}

#[fuchsia::test]
async fn on_terminate_exit_triggers_reboot() {
    // Create a topology with a reboot component and a fake reboot protocol
    let reboot_protocol_path = format!("/svc/{}", REBOOT_PROTOCOL);
    let components = vec![
        (
            "root",
            ComponentDeclBuilder::new()
                .child(ChildBuilder::new().name("system").on_terminate(fdecl::OnTerminate::Reboot))
                .protocol_default(REBOOT_PROTOCOL)
                .expose(
                    ExposeBuilder::protocol()
                        .name(REBOOT_PROTOCOL)
                        .source(cm_rust::ExposeSource::Self_),
                )
                .build(),
        ),
        ("system", ComponentDeclBuilder::new().build()),
    ];
    let (reboot_service, mut receiver) =
        create_service_directory_entry::<fstatecontrol::AdminMarker>();
    let test = RoutingTestBuilder::new("root", components)
        .set_reboot_on_terminate_policy(vec![AllowlistEntryBuilder::new().exact("system").build()])
        .add_outgoing_path("root", reboot_protocol_path.parse().unwrap(), reboot_service)
        .build()
        .await;
    let root = test.model.root();

    // Start the critical component and cause it to 'exit' by making the runner close its end
    // of the controller channel. This should cause the Admin protocol to receive a reboot request.
    test.start_instance_and_wait_start(&["system"].try_into().unwrap()).await.unwrap();
    let component = root.find_and_maybe_resolve(&["system"].try_into().unwrap()).await.unwrap();
    let info = ComponentInfo::new(component.clone()).await;
    test.mock_runner.wait_for_url("test:///system").await;
    test.mock_runner.abort_controller(&info.channel_id);
    let options = match receiver.next().await.unwrap() {
        fstatecontrol::AdminRequest::Shutdown {
            options:
                options @ fstatecontrol::ShutdownOptions { action: Some(_), reasons: Some(_), .. },
            ..
        } => options,
        _ => panic!("unexpected request"),
    };
    assert_matches!(&options.action, Some(fstatecontrol::ShutdownAction::Reboot));
    assert_matches!(
        options.reasons.as_ref().unwrap().as_slice(),
        [fstatecontrol::ShutdownReason::CriticalComponentFailure]
    );

    assert!(test.model.top_instance().has_reboot_task());
}

#[fuchsia::test]
async fn reboot_shutdown_does_not_trigger_reboot() {
    // Create a topology with a reboot-on-terminate component and a fake reboot protocol
    let reboot_protocol_path = format!("/svc/{}", REBOOT_PROTOCOL);
    let components = vec![
        (
            "root",
            ComponentDeclBuilder::new()
                .child(ChildBuilder::new().name("system").on_terminate(fdecl::OnTerminate::Reboot))
                .protocol_default(REBOOT_PROTOCOL)
                .expose(
                    ExposeBuilder::protocol()
                        .name(REBOOT_PROTOCOL)
                        .source(cm_rust::ExposeSource::Self_),
                )
                .build(),
        ),
        ("system", ComponentDeclBuilder::new().build()),
    ];
    let (reboot_service, _receiver) =
        create_service_directory_entry::<fstatecontrol::AdminMarker>();
    let test = RoutingTestBuilder::new("root", components)
        .set_reboot_on_terminate_policy(vec![AllowlistEntryBuilder::new().exact("system").build()])
        .add_outgoing_path("root", reboot_protocol_path.parse().unwrap(), reboot_service)
        .build()
        .await;
    let root = test.model.root();

    // Start the critical component and make it stop. This should cause the Admin protocol to
    // receive a reboot request.
    root.start_instance(&["system"].try_into().unwrap(), &StartReason::Debug).await.unwrap();
    let component = root.find_and_maybe_resolve(&["system"].try_into().unwrap()).await.unwrap();
    ActionsManager::register(component.clone(), ShutdownAction::new(ShutdownType::Instance))
        .await
        .unwrap();
    assert!(!test.model.top_instance().has_reboot_task());
}

#[fuchsia::test]
#[should_panic(expected = "Component with on_terminate=REBOOT terminated, but triggering \
                          reboot failed. Crashing component_manager instead: \
                          StateControl Admin FIDL:\n\tA FIDL client's channel to the protocol \
                          fuchsia.hardware.power.statecontrol.Admin was closed: NOT_FOUND")]
async fn on_terminate_with_missing_reboot_protocol_panics() {
    // Create a topology with a reboot-on-terminate component but no reboot protocol routed to root.
    let components = vec![
        (
            "root",
            ComponentDeclBuilder::new()
                .child(ChildBuilder::new().name("system").on_terminate(fdecl::OnTerminate::Reboot))
                .build(),
        ),
        ("system", ComponentDeclBuilder::new().build()),
    ];
    let test = RoutingTestBuilder::new("root", components)
        .set_reboot_on_terminate_policy(vec![AllowlistEntryBuilder::new().exact("system").build()])
        .build()
        .await;
    let root = test.model.root();

    // Start the critical component and cause it to 'exit' by making the runner close its end of
    // the controller channel. component_manager should attempt to send a reboot request, which
    // should fail because the reboot protocol isn't exposed to it -- expect component_manager to
    // respond by crashing.
    test.start_instance_and_wait_start(&["system"].try_into().unwrap()).await.unwrap();
    let component = root.find_and_maybe_resolve(&["system"].try_into().unwrap()).await.unwrap();
    let info = ComponentInfo::new(component.clone()).await;
    test.mock_runner.wait_for_url("test:///system").await;
    test.mock_runner.abort_controller(&info.channel_id);
    let () = pending().await;
}

#[fuchsia::test]
#[should_panic(expected = "Component with on_terminate=REBOOT terminated, but triggering \
                          reboot failed. Crashing component_manager instead: \
                          StateControl Admin: INTERNAL")]
async fn on_terminate_with_failed_reboot_panics() {
    // Create a topology with a reboot-on-terminate component and a fake reboot protocol
    const REBOOT_PROTOCOL: &str = fstatecontrol::AdminMarker::DEBUG_NAME;
    let reboot_protocol_path = format!("/svc/{}", REBOOT_PROTOCOL);
    let components = vec![
        (
            "root",
            ComponentDeclBuilder::new()
                .child(
                    ChildBuilder::new()
                        .name("system")
                        .on_terminate(fdecl::OnTerminate::Reboot)
                        .build(),
                )
                .protocol_default(REBOOT_PROTOCOL)
                .expose(
                    ExposeBuilder::protocol()
                        .name(REBOOT_PROTOCOL)
                        .source(cm_rust::ExposeSource::Self_),
                )
                .build(),
        ),
        ("system", ComponentDeclBuilder::new().build()),
    ];
    let (reboot_service, mut receiver) =
        create_service_directory_entry::<fstatecontrol::AdminMarker>();
    let test = RoutingTestBuilder::new("root", components)
        .set_reboot_on_terminate_policy(vec![AllowlistEntryBuilder::new().exact("system").build()])
        .add_outgoing_path("root", reboot_protocol_path.parse().unwrap(), reboot_service)
        .build()
        .await;
    let root = test.model.root();

    // Start the critical component and cause it to 'exit' by making the runner close its end
    // of the controller channel. Admin protocol should receive a reboot request -- make it fail
    // and expect component_manager to respond by crashing.
    test.start_instance_and_wait_start(&["system"].try_into().unwrap()).await.unwrap();
    let component = root.find_and_maybe_resolve(&["system"].try_into().unwrap()).await.unwrap();
    let info = ComponentInfo::new(component.clone()).await;
    test.mock_runner.wait_for_url("test:///system").await;
    test.mock_runner.abort_controller(&info.channel_id);
    match receiver.next().await.unwrap() {
        fstatecontrol::AdminRequest::Shutdown { responder, .. } => {
            responder.send(Err(zx::sys::ZX_ERR_INTERNAL)).unwrap();
        }
        _ => panic!("unexpected request"),
    };
    let () = pending().await;
}

/// If a component escrows its outgoing directory and stops, it should be started again,
/// and it should get back the queued open requests.
#[fuchsia::test(allow_stalls = false)]
async fn open_then_stop_with_escrow() {
    let (out_dir_tx, mut out_dir_rx) = mpsc::channel(1);
    let out_dir_tx = fsync::Mutex::new(out_dir_tx);

    // Create and start a component.
    let components = vec![("root", ComponentDeclBuilder::new().build())];
    let url = "test:///root";
    let test = ActionsTest::new(components[0].0, components, None).await;
    test.runner.add_host_fn(
        url,
        Box::new(move |server_end: ServerEnd<fio::DirectoryMarker>| {
            out_dir_tx.lock().try_send(server_end).unwrap();
        }),
    );
    let root = test.model.root();
    root.ensure_started(&StartReason::Debug).await.unwrap();
    test.runner.wait_for_url(url).await;

    // Queue an open request.
    let (client_end, server_end) = create_endpoints::<fio::DirectoryMarker>();
    let execution_scope = ExecutionScope::new();
    let mut object_request = fio::Flags::empty().to_object_request(server_end);
    root.open_outgoing(OpenRequest::new(
        execution_scope.clone(),
        fio::Flags::empty(),
        "echo".try_into().unwrap(),
        &mut object_request,
    ))
    .await
    .unwrap();

    // Get a hold of the outgoing directory server endpoint.
    let outgoing_server_end = out_dir_rx.next().await.unwrap();

    // Escrow the outgoing directory, then have the program stop itself.
    let info = ComponentInfo::new(root.clone()).await;
    test.runner.send_on_escrow(
        &info.channel_id,
        fcrunner::ComponentControllerOnEscrowRequest {
            outgoing_dir: Some(outgoing_server_end),
            ..Default::default()
        },
    );
    test.runner.reset_wait_for_url(url);
    test.runner.abort_controller(&info.channel_id);

    // We should observe the program getting started again.
    test.runner.wait_for_url(url).await;
    _ = fasync::TestExecutor::poll_until_stalled(future::pending::<()>()).await;
    let events: Vec<_> = test
        .test_hook
        .lifecycle()
        .into_iter()
        .filter(|event| match event {
            Lifecycle::Start(_) | Lifecycle::Stop(_) => true,
            _ => false,
        })
        .collect();
    assert_eq!(
        events,
        vec![
            Lifecycle::Start([].try_into().unwrap()),
            Lifecycle::Stop([].try_into().unwrap()),
            Lifecycle::Start([].try_into().unwrap()),
        ]
    );

    // And we should get back the same outgoing directory, with that earlier request in it.
    let outgoing_server_end = out_dir_rx.next().await.unwrap();
    let mut out_dir = OutDir::new();
    let (request_tx, mut request_rx) = mpsc::channel(1);
    let request_tx = fsync::Mutex::new(request_tx);
    out_dir.add_entry(
        "/echo".parse().unwrap(),
        vfs::service::endpoint(move |_scope, server_end| {
            request_tx.lock().try_send(server_end).unwrap();
        }),
    );
    out_dir.host_fn()(outgoing_server_end);
    let server_end = request_rx.next().await.unwrap();
    assert_eq!(
        client_end.as_handle_ref().basic_info().unwrap().related_koid,
        server_end.as_handle_ref().basic_info().unwrap().koid
    );
}

#[test_case(0)]
#[test_case(-1000)]
#[test_case(123456)]
#[fuchsia::test]
async fn stop_with_exit_code(expected_code: i64) {
    // Build test realm.
    let (model, builtin_environment, mock_runner) =
        new_model(vec![("root", component_decl_with_test_runner())]).await;
    let event_stream = new_event_stream(
        &*builtin_environment.lock().await,
        vec![EventType::Started, EventType::Stopped],
    )
    .await;

    model.start().await;
    let events = get_n_events(&event_stream, 1).await;
    assert_event_type_and_moniker(&events[0], fcomponent::EventType::Started, Moniker::root());
    let url = "test:///root";
    mock_runner.wait_for_url(url).await;

    // Stop the root component with an exit code.
    let root = model.root();
    let info = ComponentInfo::new(root.clone()).await;
    mock_runner.send_on_stop_info(
        &info.channel_id,
        fcrunner::ComponentStopInfo { exit_code: Some(expected_code), ..Default::default() },
    );
    root.stop().await.unwrap();

    // Assert that the event stream contains the exit code.
    let events = get_n_events(&event_stream, 1).await;
    assert_event_type_and_moniker(&events[0], fcomponent::EventType::Stopped, Moniker::root());
    assert_matches!(
        events[0].payload,
        Some(fcomponent::EventPayload::Stopped(fcomponent::StoppedPayload {
            status: Some(status), exit_code: Some(exit_code), ..
        }))
        if zx::Status::ok(status).is_ok() && exit_code == expected_code
    );
}
