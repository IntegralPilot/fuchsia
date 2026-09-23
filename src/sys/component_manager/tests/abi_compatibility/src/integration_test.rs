use anyhow::Error;
use assert_matches::assert_matches;
use cm_rust::push_box;
use fuchsia_component::server as fserver;
use fuchsia_component_test::*;
use futures::channel::mpsc;
use futures::{FutureExt, SinkExt, StreamExt, TryStreamExt};
use std::fmt;
use version_history::AbiRevision;
use {
    fidl_fuchsia_component_decl as fdecl, fidl_fuchsia_component_resolution as fresolution,
    fidl_fuchsia_mem as fmem, fidl_fuchsia_sys2 as fsys, fuchsia_async as fasync,
};

#[derive(Clone, Copy, Debug)]
enum TargetAbi {
    Unsupported,
    Supported,
    Absent,
}

impl fmt::Display for TargetAbi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id = match self {
            TargetAbi::Unsupported => "unsupported",
            TargetAbi::Supported => "supported",
            TargetAbi::Absent => "absent",
        };
        f.write_str(id)
    }
}

/// A component resolver serving the `fuchsia.component.resolution.Resolver` protocol.
#[derive(Clone, Copy)]
struct ComponentResolver {
    // Used to set `abi_revision` for components returned by this resolver.
    target_abi: TargetAbi,
}

impl ComponentResolver {
    pub fn new(target_abi: TargetAbi) -> Self {
        Self { target_abi }
    }
    // the scheme is the name of the target abi sent by this resolver
    pub fn scheme(&self) -> String {
        self.target_abi.to_string()
    }
    pub fn name(&self) -> String {
        format!("{}_abi_resolver", self.target_abi)
    }
    pub fn environment(&self) -> String {
        format!("{}_abi_env", self.target_abi)
    }
    pub fn abi_revision(&self) -> Option<AbiRevision> {
        match self.target_abi {
            // Assumes the platform does not support a u64::MAX ABI value.
            TargetAbi::Unsupported => Some(u64::MAX.into()),
            TargetAbi::Supported => Some(
                version_history_data::HISTORY
                    .get_example_supported_version_for_tests()
                    .abi_revision,
            ),
            TargetAbi::Absent => None,
        }
    }
    // Return a mock component specific to this resolver. Implemented as a function because
    // fresolution::Component doesn't implement Clone.
    fn mock_component(&self) -> fresolution::Component {
        fresolution::Component {
            url: Some("test".to_string()),
            decl: Some(fmem::Data::Bytes(
                fidl::persist(&fdecl::Component::default().clone()).unwrap(),
            )),
            abi_revision: self.abi_revision().map(Into::into),
            ..Default::default()
        }
    }
    pub async fn serve(
        self,
        handles: LocalComponentHandles,
        test_channel: mpsc::Sender<fresolution::Component>,
    ) -> Result<(), Error> {
        let mut fs = fserver::ServiceFs::new();
        let mut tasks = vec![];
        fs.dir("svc").add_fidl_service(move |mut stream: fresolution::ResolverRequestStream| {
            let mut test_channel = test_channel.clone();
            tasks.push(fasync::Task::local(async move {
                while let Some(req) = stream.try_next().await.expect("failed to serve resolver") {
                    match req {
                        fresolution::ResolverRequest::Resolve { component_url: _, responder } => {
                            responder.send(Ok(self.mock_component())).unwrap();
                            test_channel
                                .send(self.mock_component())
                                .await
                                .expect("failed to send results");
                        }
                        fresolution::ResolverRequest::ResolveWithContext {
                            component_url: _,
                            context: _,
                            responder,
                        } => {
                            responder.send(Ok(self.mock_component())).unwrap();
                            test_channel
                                .send(self.mock_component())
                                .await
                                .expect("failed to send results");
                        }
                        fresolution::ResolverRequest::_UnknownMethod { .. } => {
                            panic!("unknown resolver request");
                        }
                    }
                }
            }));
        });
        fs.serve_connection(handles.outgoing_dir)?;
        fs.collect::<()>().await;
        Ok(())
    }
}

// Add a component resolver to the realm that will return resolved a component containing an
// abi_revision value associated with an Resolver. This function is called for each type of
// resolver that returns a certain ABI revision value.
//
// For example, a ComponentResolver with a TargetAbi::Absent will return an
// fresolution::Component { abi_revision: None, ..} when responding to resolve requests.
//
// A copy of the fresolution::Component is sent over test_channel_tx for the test to verify.
async fn add_component_resolver(
    builder: &mut RealmBuilder,
    component_resolver: ComponentResolver,
    test_channel: mpsc::Sender<fresolution::Component>,
) {
    // Add the resolver component to the realm
    let child = builder
        .add_local_child(
            component_resolver.name(),
            move |h| component_resolver.clone().serve(h, test_channel.clone()).boxed(),
            ChildOptions::new(),
        )
        .await
        .unwrap();
    // Add resolver decl
    let mut child_decl = builder.get_component_decl(&child).await.unwrap();
    push_box(
        &mut child_decl.capabilities,
        cm_rust::CapabilityDecl::Resolver(cm_rust::ResolverDecl {
            name: component_resolver.name().parse().unwrap(),
            source_path: Some("/svc/fuchsia.component.resolution.Resolver".parse().unwrap()),
        }),
    );
    push_box(
        &mut child_decl.exposes,
        cm_rust::ExposeDecl::Resolver(cm_rust::ExposeResolverDecl {
            source: cm_rust::ExposeSource::Self_,
            source_name: component_resolver.name().parse().unwrap(),
            source_dictionary: Default::default(),
            target: cm_rust::ExposeTarget::Parent,
            target_name: component_resolver.name().parse().unwrap(),
        }),
    );
    builder.replace_component_decl(&child, child_decl).await.unwrap();
    // Add resolver to the test realm
    let mut realm_decl = builder.get_realm_decl().await.unwrap();
    push_box(
        &mut realm_decl.environments,
        cm_rust::EnvironmentDecl {
            name: component_resolver.environment().parse().unwrap(),
            extends: fdecl::EnvironmentExtends::Realm,
            runners: Box::from([]),
            resolvers: Box::from([cm_rust::ResolverRegistration {
                resolver: component_resolver.name().parse().unwrap(),
                source: cm_rust::RegistrationSource::Child(component_resolver.name()),
                // The component resolver is associated with the scheme indicating the abi_revision it returns.
                // (e.g component url "absent:xxx" is resolved by scheme "absent", served by
                // ComponentResolver { TargetAbi::Absent }
                scheme: component_resolver.clone().scheme(),
            }]),
            debug_capabilities: Box::from([]),
            stop_timeout_ms: None,
        },
    );
    builder.replace_realm_decl(realm_decl).await.unwrap();
}

// Construct a realm with three component resolvers that return distinct abi revisions for components.
// A test channel is passed to each resolver to send a copy of its FIDL response for testing.
// `child_name_prefix` will be prepended to the names of all of the components.
async fn create_realm(
    child_name_prefix: &str,
    test_channel_tx: mpsc::Sender<fresolution::Component>,
) -> RealmInstance {
    let mut builder = RealmBuilder::new().await.unwrap();

    let cr_absent = ComponentResolver::new(TargetAbi::Absent);
    let cr_supported = ComponentResolver::new(TargetAbi::Supported);
    let cr_unsupported = ComponentResolver::new(TargetAbi::Unsupported);

    add_component_resolver(&mut builder, cr_absent.clone(), test_channel_tx.clone()).await;
    add_component_resolver(&mut builder, cr_supported.clone(), test_channel_tx.clone()).await;
    add_component_resolver(&mut builder, cr_unsupported.clone(), test_channel_tx).await;

    builder
        .add_child(
            format!("{}absent_abi_component", child_name_prefix),
            "absent://absent_abi_component",
            ChildOptions::new().environment(cr_absent.environment()),
        )
        .await
        .unwrap();
    builder
        .add_child(
            format!("{}unsupported_abi_component", child_name_prefix),
            "unsupported://unsupported_abi_component",
            ChildOptions::new().environment(cr_unsupported.environment()),
        )
        .await
        .unwrap();
    builder
        .add_child(
            format!("{}supported_abi_component", child_name_prefix),
            "supported://supported_abi_component",
            ChildOptions::new().environment(cr_supported.environment()),
        )
        .await
        .unwrap();

    let cm_builder = builder
        .with_nested_component_manager("#meta/abi_compat_component_manager.cm")
        .await
        .unwrap();

    let instance = cm_builder.build().await.unwrap();
    instance.start_component_tree().await.unwrap();
    instance
}

#[fuchsia::test]
async fn resolve_regular_components() {
    // A channel used to verify component resolver fidl responses.
    let (test_channel_tx, mut test_channel_rx) = mpsc::channel(1);
    let instance = create_realm("", test_channel_tx).await;
    // get a handle to lifecycle controller to start components
    let lifecycle_controller: fsys::LifecycleControllerProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    // get a handle to realmquery to get component info
    let realm_query: fsys::RealmQueryProxy = instance
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("failed to connect to RealmQuery");

    // Test resolution of a component with an absent abi revision
    {
        let child_moniker = "./absent_abi_component";
        // Attempt to resolve the component. Expect the component resolver to have returned a
        // resolved component with an absent ABI, so resolution fails.
        let resolve_res = lifecycle_controller.resolve_instance(child_moniker).await.unwrap();
        assert_eq!(resolve_res, Err(fsys::ResolveError::Internal));
        // verify the copy of the component resolver result that was sent to component manager
        let resolver_response = test_channel_rx.next().await;
        assert_matches!(resolver_response, Some(fresolution::Component { abi_revision: None, .. }));
        let instance = realm_query.get_instance(child_moniker).await.unwrap().unwrap();
        assert!(instance.resolved_info.is_none());
    }
    // Test resolution of a component with an unsupported abi revision
    {
        let child_moniker = "./unsupported_abi_component";
        let resolve_res = lifecycle_controller.resolve_instance(child_moniker).await.unwrap();
        assert_eq!(resolve_res, Err(fsys::ResolveError::Internal));
        let resolver_response = test_channel_rx.next().await;
        assert_matches!(
            resolver_response,
            Some(fresolution::Component { abi_revision: Some(u64::MAX), .. })
        );
        let instance = realm_query.get_instance(child_moniker).await.unwrap().unwrap();
        assert!(instance.resolved_info.is_none());
    }
    // Test resolution of a component with a supported abi revision
    {
        let child_moniker = "./supported_abi_component";
        let resolve_res = lifecycle_controller.resolve_instance(child_moniker).await.unwrap();
        assert_eq!(resolve_res, Ok(()));
        let resolver_response =
            test_channel_rx.next().await.expect("resolver failed to return an ABI component");
        assert_eq!(
            resolver_response.abi_revision.unwrap(),
            version_history_data::HISTORY
                .get_example_supported_version_for_tests()
                .abi_revision
                .as_u64()
        );
        let instance = realm_query.get_instance(child_moniker).await.unwrap().unwrap();
        assert!(instance.resolved_info.is_some());
    }
}

#[fuchsia::test]
async fn resolve_allowlisted_components() {
    // A channel used to verify component resolver fidl responses.
    let (test_channel_tx, mut test_channel_rx) = mpsc::channel(1);

    // Add `exempt_` to the beginning of the component names. These specific
    // component names are allowlisted in abi_compat_cm_config.json5.
    let instance = create_realm("exempt_", test_channel_tx).await;
    // get a handle to lifecycle controller to start components
    let lifecycle_controller: fsys::LifecycleControllerProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    // get a handle to realmquery to get component info
    let realm_query: fsys::RealmQueryProxy = instance
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("failed to connect to RealmQuery");

    // Test resolution of a component with an absent abi revision
    {
        let child_moniker = "./exempt_absent_abi_component";
        // Attempt to resolve the component. Expect the component resolver to
        // have returned a resolved component with an absent ABI, and resolution
        // works because the child is on the allowlist.
        let resolve_res = lifecycle_controller.resolve_instance(child_moniker).await.unwrap();
        assert_eq!(resolve_res, Ok(()));
        // verify the copy of the component resolver result that was sent to component manager
        let resolver_response = test_channel_rx.next().await;
        assert_matches!(resolver_response, Some(fresolution::Component { abi_revision: None, .. }));
        let instance = realm_query.get_instance(child_moniker).await.unwrap().unwrap();
        assert!(instance.resolved_info.is_some());
    }
    // Test resolution of a component with an unsupported abi revision
    {
        let child_moniker = "./exempt_unsupported_abi_component";
        let resolve_res = lifecycle_controller.resolve_instance(child_moniker).await.unwrap();
        assert_eq!(resolve_res, Ok(()));
        let resolver_response = test_channel_rx.next().await;
        assert_matches!(
            resolver_response,
            Some(fresolution::Component { abi_revision: Some(u64::MAX), .. })
        );
        let instance = realm_query.get_instance(child_moniker).await.unwrap().unwrap();
        assert!(instance.resolved_info.is_some());
    }
    // Test resolution of a component with a supported abi revision
    {
        let child_moniker = "./exempt_supported_abi_component";
        let resolve_res = lifecycle_controller.resolve_instance(child_moniker).await.unwrap();
        assert_eq!(resolve_res, Ok(()));
        let resolver_response =
            test_channel_rx.next().await.expect("resolver failed to return an ABI component");
        assert_eq!(
            resolver_response.abi_revision.unwrap(),
            version_history_data::HISTORY
                .get_example_supported_version_for_tests()
                .abi_revision
                .as_u64()
        );
        let instance = realm_query.get_instance(child_moniker).await.unwrap().unwrap();
        assert!(instance.resolved_info.is_some());
    }
}
