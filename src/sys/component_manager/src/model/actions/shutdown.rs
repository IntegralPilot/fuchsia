// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::actions::{Action, ActionKey, ActionsManager};
use crate::model::component::instance::{InstanceState, ResolvedInstanceState};
use crate::model::component::{ComponentInstance, WeakComponentInstance};
use async_trait::async_trait;
use cm_types::LongName;
use errors::{ActionError, ShutdownActionError};
use futures::prelude::*;
use log::*;
use moniker::ChildName;
use std::collections::HashMap;
use std::sync::Arc;

use cm_graph::DependencyNode;
use directed_graph::DirectedGraph;
use fuchsia_async as fasync;

/// Shuts down all component instances in this component (stops them and guarantees they will never
/// be started again).
pub struct ShutdownAction {
    shutdown_type: ShutdownType,
}

/// Indicates the type of shutdown being performed.
#[derive(Clone, Copy, PartialEq)]
pub enum ShutdownType {
    /// An individual component instance was shut down. For example, this is used when
    /// a component instance is destroyed.
    Instance,

    /// The entire system under this component_manager was shutdown on behalf of
    /// a call to SystemController/Shutdown.
    System,
}

impl ShutdownAction {
    pub fn new(shutdown_type: ShutdownType) -> Self {
        Self { shutdown_type }
    }
}

#[async_trait]
impl Action for ShutdownAction {
    async fn handle(self, component: Arc<ComponentInstance>) -> Result<(), ActionError> {
        do_shutdown(&component, self.shutdown_type).await
    }
    fn key(&self) -> ActionKey {
        ActionKey::Shutdown
    }
}

/// Used to track information during the shutdown process.
struct ShutdownComponentInfo {
    dependency_node: DependencyNode,
    component: Arc<ComponentInstance>,
}

async fn shutdown_component(
    target: ShutdownComponentInfo,
    shutdown_type: ShutdownType,
) -> Result<DependencyNode, ActionError> {
    match target.dependency_node {
        DependencyNode::Self_ => {
            // TODO: Put `self` in a "shutting down" state so that if it creates
            // new instances after this point, they are created in a shut down
            // state.
            //
            // NOTE: we cannot register a `StopAction { shutdown: true }` action because
            // that would be overridden by any concurrent `StopAction { shutdown: false }`.
            // More over, for reasons detailed in
            // https://fxrev.dev/I8ccfa1deed368f2ccb77cde0d713f3af221f7450, an in-progress
            // Shutdown action will block Stop actions, so registering the latter will deadlock.
            target.component.stop_instance_internal(true).await?;
        }
        DependencyNode::Child(..) => {
            ActionsManager::register(target.component, ShutdownAction::new(shutdown_type)).await?;
        }
        _ => {
            // This is just an intermediate node that exists to track dependencies on storage
            // and dictionary capabilities from Self, which aren't associated with the running
            // program. Nothing to do.
        }
    }

    Ok(target.dependency_node.clone())
}

/// Contains all the information necessary to shut down a single component
/// instance. The shutdown process is recursive: shutting down a component requires
/// shutting down all of its children first.
struct ShutdownJob {
    /// The type of shutdown being performed. For debug purposes.
    shutdown_type: ShutdownType,
    /// The names of the static children of the component being shut down.
    static_child_names: Vec<LongName>,
    /// A reference-counted pointer to the component instance being shut down.
    instance: Arc<ComponentInstance>,
    /// A map of the live children of the `instance`.
    children: HashMap<ChildName, WeakComponentInstance>,
    /// The component's capability graph, which determines the shutdown order.
    dependencies: Arc<DirectedGraph<DependencyNode>>,
}

/// ShutdownJob encapsulates the logic and state require to shutdown a component.
impl ShutdownJob {
    /// Creates a new ShutdownJob by examining the Component's declaration and
    /// runtime state to build up the necessary data structures to stop
    /// components in the component in dependency order.
    pub async fn new(
        instance: &Arc<ComponentInstance>,
        state: &ResolvedInstanceState,
        shutdown_type: ShutdownType,
    ) -> ShutdownJob {
        let dependencies = state.resolved_component.dependencies.clone();
        let static_child_names = state
            .children
            .iter()
            .filter(|k| k.0.collection().is_none())
            .filter_map(|(v, _)| {
                LongName::new(&v)
                    .inspect_err(|e| log::warn!("child name not convertible to long name: {e}"))
                    .ok()
            })
            .collect();
        let children = state
            .children
            .iter()
            .map(|(k, v)| (k.clone(), WeakComponentInstance::new(v)))
            .collect();
        let new_job = ShutdownJob {
            shutdown_type,
            static_child_names,
            instance: instance.clone(),
            children,
            dependencies,
        };
        return new_job;
    }

    /// Perform shutdown of the Component that was used to create this ShutdownJob.
    pub async fn execute(mut self) -> Result<(), ActionError> {
        let dynamic_children: Box<[_]> = self
            .children
            .keys()
            .filter_map(|key| key.collection().map(|coll| (key.name().as_str(), coll.as_str())))
            .collect();
        process_deps(
            &self.static_child_names,
            &dynamic_children,
            Arc::make_mut(&mut self.dependencies),
        );

        let sorted_map = self
            .dependencies
            .topological_sort()
            .map_err(|_| ActionError::from(ShutdownActionError::CyclesDetected {}))?;

        for dependency_node in sorted_map.into_iter() {
            let component = match dependency_node {
                DependencyNode::Child(name, coll) => {
                    let moniker = &ChildName::try_new(name, coll.as_ref())
                        .map_err(|err| ShutdownActionError::InvalidChildName { err })?;

                    let child_instance_res = self.children.get(moniker);
                    let Some(child_instance) = child_instance_res else {
                        continue;
                    };
                    let upgraded_instance_result = child_instance.upgrade();
                    match upgraded_instance_result {
                        Ok(upgraded_instance) => upgraded_instance,
                        Err(_) => continue,
                    }
                }
                _ => self.instance.clone(),
            };
            let target: ShutdownComponentInfo =
                ShutdownComponentInfo { dependency_node: dependency_node.clone(), component };
            shutdown_component(target, self.shutdown_type).await?;
        }
        Ok(())
    }
}

pub async fn do_shutdown(
    component: &Arc<ComponentInstance>,
    shutdown_type: ShutdownType,
) -> Result<(), ActionError> {
    const WATCHDOG_INTERVAL: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(15);

    // Keep logs short to preserve as much as possible in the crash report
    // NS: Shutdown of {moniker} was no-op
    // RS: Beginning shutdown of resolved component {moniker}
    // US: Beginning shutdown of unresolved component {moniker}
    // PS: Pending shutdown of component {moniker} has taken more than WATCHDOG_TIMEOUT_SECS
    // FS: Finished shutdown of {moniker}
    // ES: Errored shutdown of {moniker}
    let moniker = component.moniker.clone();
    let _watchdog_task = fasync::Task::spawn(async move {
        let mut interval = fasync::Interval::new(WATCHDOG_INTERVAL);
        while let Some(_) = interval.next().await {
            info!("=PS {}", moniker);
        }
    });
    {
        let state = component.lock_state().await;
        match *state {
            InstanceState::Resolved(ref s) | InstanceState::Started(ref s, _) => {
                if matches!(shutdown_type, ShutdownType::System) {
                    info!("=RS {}", component.moniker);
                }
                let shutdown_job = ShutdownJob::new(component, s, shutdown_type).await;
                drop(state);
                Box::pin(shutdown_job.execute()).await.map_err(|err| {
                    warn!("=ES {}", component.moniker);
                    err
                })?;
                if matches!(shutdown_type, ShutdownType::System) {
                    info!("=FS {}", component.moniker);
                }
                return Ok(());
            }
            InstanceState::Shutdown(_, _) => {
                if matches!(shutdown_type, ShutdownType::System) {
                    info!("=NS {}", component.moniker);
                }
                return Ok(());
            }
            InstanceState::Unresolved(_) | InstanceState::Destroyed => {}
        }
    }

    // Control flow arrives here if the component isn't resolved.
    // TODO: Put this component in a "shutting down" state so that if it creates new instances
    // after this point, they are created in a shut down state.
    if let ShutdownType::System = shutdown_type {
        info!("=US {}", component.moniker);
    }

    match component.stop_instance_internal(true).await {
        Ok(()) if shutdown_type == ShutdownType::System => info!("=FS {}", component.moniker),
        Ok(()) => (),
        Err(e) if shutdown_type == ShutdownType::System => {
            info!("=ES {}", component.moniker);
            return Err(e.into());
        }
        Err(e) => return Err(e.into()),
    }

    Ok(())
}

/// For a given component `decl`, identify capability dependencies between the
/// component itself and its children. A directed graph of these dependencies is
/// returned.
fn process_deps(
    static_child_names: &Vec<LongName>,
    dynamic_children: &[(&str, &str)],
    strong_dependencies: &mut DirectedGraph<DependencyNode>,
) {
    let self_dep_closure = strong_dependencies.get_closure(&DependencyNode::Self_);

    let max_edges = static_child_names.len() + dynamic_children.len();

    let mut edges_to_add: Vec<(DependencyNode, DependencyNode)> = Vec::with_capacity(max_edges);
    for child_name in static_child_names {
        let dependency_node = DependencyNode::Child(child_name.clone().into(), None);
        if !self_dep_closure.contains(&&dependency_node) {
            edges_to_add.push((DependencyNode::Self_, dependency_node));
        }
    }

    for (child_name, collection) in dynamic_children {
        let dependency_node =
            DependencyNode::Child((*child_name).into(), Some((*collection).into()));
        if !self_dep_closure.contains(&dependency_node) {
            edges_to_add.push((DependencyNode::Self_, dependency_node));
        }
    }
    drop(self_dep_closure);

    for (a, b) in edges_to_add {
        strong_dependencies.add_edge(a, b);
    }
    strong_dependencies.add_node(DependencyNode::Self_);
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::actions::StopAction;
    use crate::model::actions::test_utils::MockAction;
    use crate::model::component::StartReason;
    use crate::model::testing::out_dir::OutDir;
    use crate::model::testing::test_helpers::{
        ActionsTest, ComponentInfo, component_decl_with_test_runner, execution_is_shut_down,
        has_child,
    };
    use crate::model::testing::test_hook::Lifecycle;
    use async_utils::PollExt;
    use cm_rust::offer::{OfferSource, OfferTarget};
    use cm_rust::{
        ChildRef, DependencyType, ExposeSource, ExposeTarget, RegistrationSource,
        StorageDirectorySource, UseSource,
    };
    use cm_rust_testing::*;
    use cm_types::AllowedOffers;
    use errors::StopActionError;
    use fidl::endpoints::RequestStream;
    use fidl_fuchsia_component as fcomponent;
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_component_runner as fcrunner;
    use fuchsia_async as fasync;
    use maplit::btreeset;
    use moniker::Moniker;
    use std::collections::BTreeSet;
    use test_case::test_case;

    fn process_deps_test(
        decl: &fdecl::Component,
        dynamic_children: &[(&str, &str)],
        dynamic_offers: &[(DependencyNode, DependencyNode)],
    ) -> DirectedGraph<DependencyNode> {
        let mut dependencies = dynamic_offers.iter().cloned().collect::<Box<[_]>>().into();
        cm_graph::generate_dependency_graph(&mut dependencies, decl, dynamic_children);
        let static_child_names = decl
            .children
            .as_ref()
            .map(|children| {
                children
                    .iter()
                    .filter_map(|child| child.name.clone())
                    .filter_map(|name| LongName::new(name).ok())
                    .collect()
            })
            .unwrap_or_default();
        process_deps(&static_child_names, dynamic_children, &mut dependencies);
        dependencies
    }

    #[fuchsia::test]
    fn test_service_from_self() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("serviceSelf")
                    .source(OfferSource::Self_)
                    .target_static_child("childA"),
            )
            .child_default("childA")
            .build()
            .into();

        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> =
            vec![DependencyNode::Child("childA".into(), None), DependencyNode::Self_];

        assert_eq!(ans, sorted_map)
    }

    #[test_case(DependencyType::Weak)]
    fn test_weak_service_from_self(weak_dep: DependencyType) {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("serviceSelf")
                    .source(OfferSource::Self_)
                    .target_static_child("childA")
                    .dependency(weak_dep),
            )
            .child_default("childA")
            .build()
            .into();

        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> =
            vec![DependencyNode::Child("childA".into(), None), DependencyNode::Self_];

        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_service_from_child() {
        let fidl_decl = ComponentDeclBuilder::new()
            .expose(
                ExposeBuilder::protocol()
                    .name("serviceFromChild")
                    .source_static_child("childA")
                    .target(ExposeTarget::Parent),
            )
            .child_default("childA")
            .build()
            .into();

        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> =
            vec![DependencyNode::Child("childA".into(), None), DependencyNode::Self_];

        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_single_dependency() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("serviceParent")
                    .source(OfferSource::Self_)
                    .target_static_child("childA"),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childBOffer")
                    .target_name("serviceSibling")
                    .source_static_child("childB")
                    .target_static_child("childA"),
            )
            .child_default("childA")
            .child_default("childB")
            .build()
            .into();

        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Self_,
        ];

        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_dictionary_dependency() {
        let fidl_decl = ComponentDeclBuilder::new()
            .dictionary_default("dict")
            .protocol_default("serviceA")
            .offer(
                OfferBuilder::protocol()
                    .name("serviceA")
                    .source(OfferSource::Self_)
                    .target(OfferTarget::Capability("dict".parse().unwrap())),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("serviceB")
                    .source_static_child("childA")
                    .target(OfferTarget::Capability("dict".parse().unwrap())),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("serviceB")
                    .source(OfferSource::Self_)
                    .from_dictionary("dict")
                    .target_static_child("childB"),
            )
            .child_default("childA")
            .child_default("childB")
            .build()
            .into();

        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Capability("dict".into()),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];

        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_environment_with_runner_from_parent() {
        let fidl_decl = ComponentDeclBuilder::new()
            .environment(EnvironmentBuilder::new().name("env").runner(
                cm_rust::RunnerRegistration {
                    source: RegistrationSource::Parent,
                    source_name: "foo".parse().unwrap(),
                    target_name: "foo".parse().unwrap(),
                },
            ))
            .child_default("childA")
            .child(ChildBuilder::new().name("childB").environment("env"))
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Self_,
            DependencyNode::Environment("env".into()),
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_environment_with_runner_from_self() {
        let fidl_decl = ComponentDeclBuilder::new()
            .environment(EnvironmentBuilder::new().name("env").runner(
                cm_rust::RunnerRegistration {
                    source: RegistrationSource::Self_,
                    source_name: "foo".parse().unwrap(),
                    target_name: "foo".parse().unwrap(),
                },
            ))
            .child_default("childA")
            .child(ChildBuilder::new().name("childB").environment("env"))
            .build()
            .into();

        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Environment("env".into()),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_environment_with_runner_from_child() {
        let fidl_decl = ComponentDeclBuilder::new()
            .environment(EnvironmentBuilder::new().name("env").runner(
                cm_rust::RunnerRegistration {
                    source: RegistrationSource::Child("childA".to_string()),
                    source_name: "foo".parse().unwrap(),
                    target_name: "foo".parse().unwrap(),
                },
            ))
            .child_default("childA")
            .child(ChildBuilder::new().name("childB").environment("env"))
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Environment("env".into()),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_environment_with_runner_from_child_to_collection() {
        let fidl_decl = ComponentDeclBuilder::new()
            .environment(EnvironmentBuilder::new().name("env").runner(
                cm_rust::RunnerRegistration {
                    source: RegistrationSource::Child("childA".to_string()),
                    source_name: "foo".parse().unwrap(),
                    target_name: "foo".parse().unwrap(),
                },
            ))
            .collection(CollectionBuilder::new().name("coll").environment("env"))
            .child_default("childA")
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(
            &fidl_decl,
            &vec![("dyn1", "coll"), ("dyn2", "coll")],
            &dynamic_offers,
        )
        .topological_sort()
        .unwrap()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("dyn1".into(), Some("coll".into())),
            DependencyNode::Child("dyn2".into(), Some("coll".into())),
            DependencyNode::Collection("coll".into()),
            DependencyNode::Environment("env".into()),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_chained_environments() {
        let fidl_decl = ComponentDeclBuilder::new()
            .environment(EnvironmentBuilder::new().name("env").runner(
                cm_rust::RunnerRegistration {
                    source: RegistrationSource::Child("childA".to_string()),
                    source_name: "foo".parse().unwrap(),
                    target_name: "foo".parse().unwrap(),
                },
            ))
            .environment(EnvironmentBuilder::new().name("env2").runner(
                cm_rust::RunnerRegistration {
                    source: RegistrationSource::Child("childB".to_string()),
                    source_name: "bar".parse().unwrap(),
                    target_name: "bar".parse().unwrap(),
                },
            ))
            .child_default("childA")
            .child(ChildBuilder::new().name("childB").environment("env"))
            .child(ChildBuilder::new().name("childC").environment("env2"))
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childC".into(), None),
            DependencyNode::Environment("env2".into()),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Environment("env".into()),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_environment_and_offer() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("childBOffer")
                    .target_name("serviceSibling")
                    .source_static_child("childB")
                    .target_static_child("childC"),
            )
            .environment(EnvironmentBuilder::new().name("env").runner(
                cm_rust::RunnerRegistration {
                    source: RegistrationSource::Child("childA".into()),
                    source_name: "foo".parse().unwrap(),
                    target_name: "foo".parse().unwrap(),
                },
            ))
            .child_default("childA")
            .child(ChildBuilder::new().name("childB").environment("env"))
            .child_default("childC")
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childC".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Environment("env".into()),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_environment_with_resolver_from_parent() {
        let fidl_decl = ComponentDeclBuilder::new()
            .environment(EnvironmentBuilder::new().name("resolver_env").resolver(
                cm_rust::ResolverRegistration {
                    source: RegistrationSource::Parent,
                    resolver: "foo".parse().unwrap(),
                    scheme: "httweeeeees".into(),
                },
            ))
            .child_default("childA")
            .child(ChildBuilder::new().name("childB").environment("resolver_env"))
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Self_,
            DependencyNode::Environment("resolver_env".into()),
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_environment_with_resolver_from_child() {
        let fidl_decl = ComponentDeclBuilder::new()
            .environment(EnvironmentBuilder::new().name("resolver_env").resolver(
                cm_rust::ResolverRegistration {
                    source: RegistrationSource::Child("childA".to_string()),
                    resolver: "foo".parse().unwrap(),
                    scheme: "httweeeeees".into(),
                },
            ))
            .child_default("childA")
            .child(ChildBuilder::new().name("childB").environment("resolver_env"))
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Environment("resolver_env".into()),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    // add test where B depends on A via environment and C depends on B via environment

    #[fuchsia::test]
    fn test_environment_with_chain_of_resolvers() {
        let fidl_decl = ComponentDeclBuilder::new()
            .environment(EnvironmentBuilder::new().name("env1").resolver(
                cm_rust::ResolverRegistration {
                    source: RegistrationSource::Child("childA".to_string()),
                    resolver: "foo".parse().unwrap(),
                    scheme: "httweeeeees".into(),
                },
            ))
            .environment(EnvironmentBuilder::new().name("env2").resolver(
                cm_rust::ResolverRegistration {
                    source: RegistrationSource::Child("childB".to_string()),
                    resolver: "bar".parse().unwrap(),
                    scheme: "httweeeeee".into(),
                },
            ))
            .child_default("childA")
            .child(ChildBuilder::new().name("childB").environment("env1"))
            .child(ChildBuilder::new().name("childC").environment("env2"))
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childC".into(), None),
            DependencyNode::Environment("env2".into()),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Environment("env1".into()),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_environment_with_resolver_and_runner_from_child() {
        let fidl_decl = ComponentDeclBuilder::new()
            .environment(
                EnvironmentBuilder::new()
                    .name("multi_env")
                    .resolver(cm_rust::ResolverRegistration {
                        source: RegistrationSource::Child("childA".to_string()),
                        resolver: "foo".parse().unwrap(),
                        scheme: "httweeeeees".into(),
                    })
                    .runner(cm_rust::RunnerRegistration {
                        source: RegistrationSource::Child("childB".to_string()),
                        source_name: "bar".parse().unwrap(),
                        target_name: "bar".parse().unwrap(),
                    }),
            )
            .child_default("childA")
            .child_default("childB")
            .child(ChildBuilder::new().name("childC").environment("multi_env"))
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childC".into(), None),
            DependencyNode::Environment("multi_env".into()),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_environment_with_collection_resolver_from_child() {
        let fidl_decl = ComponentDeclBuilder::new()
            .environment(EnvironmentBuilder::new().name("resolver_env").resolver(
                cm_rust::ResolverRegistration {
                    source: RegistrationSource::Child("childA".to_string()),
                    resolver: "foo".parse().unwrap(),
                    scheme: "httweeeeees".into(),
                },
            ))
            .child_default("childA")
            .collection(CollectionBuilder::new().name("coll").environment("resolver_env"))
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(
            &fidl_decl,
            &vec![("dyn1", "coll"), ("dyn2", "coll")],
            &dynamic_offers,
        )
        .topological_sort()
        .unwrap()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("dyn1".into(), Some("coll".into())),
            DependencyNode::Child("dyn2".into(), Some("coll".into())),
            DependencyNode::Collection("coll".into()),
            DependencyNode::Environment("resolver_env".into()),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_dynamic_offers_within_collection() {
        let fidl_decl = ComponentDeclBuilder::new()
            .child_default("childA")
            .collection_default("coll")
            .offer(
                OfferBuilder::directory()
                    .name("some_dir")
                    .source(OfferSource::Child(ChildRef {
                        name: "childA".parse().unwrap(),
                        collection: None,
                    }))
                    .target(OfferTarget::Collection("coll".parse().unwrap())),
            )
            .build()
            .into();
        let offer_1 = fdecl::Offer::Protocol(fdecl::OfferProtocol {
            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: "dyn1".into(),
                collection: Some("coll".parse().unwrap()),
            })),
            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: "dyn2".into(),
                collection: Some("coll".parse().unwrap()),
            })),
            target_name: Some("test.protocol".to_string()),
            dependency_type: Some(fdecl::DependencyType::Strong),
            ..Default::default()
        });

        let offer_2 = fdecl::Offer::Protocol(fdecl::OfferProtocol {
            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: "dyn1".into(),
                collection: Some("coll".parse().unwrap()),
            })),
            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: "dyn3".into(),
                collection: Some("coll".parse().unwrap()),
            })),
            target_name: Some("test.protocol".to_string()),
            dependency_type: Some(fdecl::DependencyType::Strong),
            ..Default::default()
        });

        let dynamic_offers = vec![make_dep(offer_1), make_dep(offer_2)];
        let sorted_map = process_deps_test(
            &fidl_decl,
            &vec![("dyn1", "coll"), ("dyn2", "coll"), ("dyn3", "coll"), ("dyn4", "coll")],
            &dynamic_offers,
        )
        .topological_sort()
        .unwrap()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("dyn2".into(), Some("coll".into())),
            DependencyNode::Child("dyn3".into(), Some("coll".into())),
            DependencyNode::Child("dyn1".into(), Some("coll".into())),
            DependencyNode::Child("dyn4".into(), Some("coll".into())),
            DependencyNode::Collection("coll".into()),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_dynamic_offers_between_collections() {
        let fidl_decl = ComponentDeclBuilder::new()
            .collection_default("coll1")
            .collection_default("coll2")
            .build()
            .into();
        let offer_1 = fdecl::Offer::Protocol(fdecl::OfferProtocol {
            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: "dyn1".into(),
                collection: Some("coll1".parse().unwrap()),
            })),
            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: "dyn1".into(),
                collection: Some("coll2".parse().unwrap()),
            })),
            target_name: Some("test.protocol".to_string()),
            dependency_type: Some(fdecl::DependencyType::Strong),
            ..Default::default()
        });

        let offer_2 = fdecl::Offer::Protocol(fdecl::OfferProtocol {
            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: "dyn2".into(),
                collection: Some("coll2".parse().unwrap()),
            })),
            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: "dyn1".into(),
                collection: Some("coll1".parse().unwrap()),
            })),
            target_name: Some("test.protocol".to_string()),
            dependency_type: Some(fdecl::DependencyType::Strong),
            ..Default::default()
        });

        let dynamic_offers = vec![make_dep(offer_1), make_dep(offer_2)];
        let sorted_map = process_deps_test(
            &fidl_decl,
            &vec![("dyn1", "coll1"), ("dyn2", "coll1"), ("dyn1", "coll2"), ("dyn2", "coll2")],
            &dynamic_offers,
        )
        .topological_sort()
        .unwrap()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("dyn1".into(), Some("coll2".into())),
            DependencyNode::Child("dyn1".into(), Some("coll1".into())),
            DependencyNode::Child("dyn2".into(), Some("coll1".into())),
            DependencyNode::Child("dyn2".into(), Some("coll2".into())),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_dynamic_offer_from_parent() {
        let fidl_decl = ComponentDeclBuilder::new().collection_default("coll").build().into();
        // parent does not generate a dependency node
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(
            &fidl_decl,
            &vec![("dyn1", "coll"), ("dyn2", "coll")],
            &dynamic_offers,
        )
        .topological_sort()
        .unwrap()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("dyn1".into(), Some("coll".into())),
            DependencyNode::Child("dyn2".into(), Some("coll".into())),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_dynamic_offer_from_self() {
        let fidl_decl = ComponentDeclBuilder::new().collection_default("coll").build().into();
        // An offer from Self -> Child does not generate a dependency
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(
            &fidl_decl,
            &vec![("dyn1", "coll"), ("dyn2", "coll")],
            &dynamic_offers,
        )
        .topological_sort()
        .unwrap()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("dyn1".into(), Some("coll".into())),
            DependencyNode::Child("dyn2".into(), Some("coll".into())),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_dynamic_offer_from_static_child() {
        let fidl_decl = ComponentDeclBuilder::new()
            .child_default("childA")
            .child_default("childB")
            .collection_default("coll")
            .build()
            .into();
        let offer_1 = fdecl::Offer::Protocol(fdecl::OfferProtocol {
            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: "childA".into(),
                collection: None,
            })),
            target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: "dyn1".into(),
                collection: Some("coll".parse().unwrap()),
            })),
            target_name: Some("test.protocol".to_string()),
            dependency_type: Some(fdecl::DependencyType::Strong),
            ..Default::default()
        });
        let dynamic_offers = vec![make_dep(offer_1)];
        let sorted_map = process_deps_test(
            &fidl_decl,
            &vec![("dyn1", "coll"), ("dyn2", "coll")],
            &dynamic_offers,
        )
        .topological_sort()
        .unwrap()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("dyn1".into(), Some("coll".into())),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Child("dyn2".into(), Some("coll".into())),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[test_case(DependencyType::Weak)]
    fn test_single_weak_dependency(weak_dep: DependencyType) {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("serviceSelf")
                    .source(OfferSource::Self_)
                    .target_static_child("childA")
                    .dependency(weak_dep.clone()),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childBOffer")
                    .target_name("serviceSibling")
                    .source_static_child("childB")
                    .target_static_child("childA")
                    .dependency(weak_dep.clone()),
            )
            .child_default("childA")
            .child_default("childB")
            .build()
            .into();

        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_multiple_dependencies_same_source() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("serviceSelf")
                    .source(OfferSource::Self_)
                    .target_static_child("childA"),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childBOffer")
                    .target_name("serviceSibling")
                    .source_static_child("childB")
                    .target_static_child("childA"),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childBOtherOffer")
                    .target_name("serviceOtherSibling")
                    .source_static_child("childB")
                    .target_static_child("childA"),
            )
            .child_default("childA")
            .child_default("childB")
            .build()
            .into();

        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_multiple_dependents_same_source() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("childBOffer")
                    .target_name("serviceSibling")
                    .source_static_child("childB")
                    .target_static_child("childA"),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childBToC")
                    .target_name("serviceSibling")
                    .source_static_child("childB")
                    .target_static_child("childC"),
            )
            .child_default("childA")
            .child_default("childB")
            .child_default("childC")
            .build()
            .into();

        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Child("childC".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[test_case(DependencyType::Weak)]
    fn test_multiple_dependencies(weak_dep: DependencyType) {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("childBOffer")
                    .target_name("serviceSibling")
                    .source_static_child("childA")
                    .target_static_child("childC"),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childBToC")
                    .target_name("serviceSibling")
                    .source_static_child("childB")
                    .target_static_child("childC"),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childCToA")
                    .target_name("serviceSibling")
                    .source_static_child("childC")
                    .target_static_child("childA")
                    .dependency(weak_dep),
            )
            .child_default("childA")
            .child_default("childB")
            .child_default("childC")
            .build()
            .into();

        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childC".into(), None),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_component_is_source_and_target() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("childBOffer")
                    .target_name("serviceSibling")
                    .source_static_child("childA")
                    .target_static_child("childB"),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childBToC")
                    .target_name("serviceSibling")
                    .source_static_child("childB")
                    .target_static_child("childC"),
            )
            .child_default("childA")
            .child_default("childB")
            .child_default("childC")
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childC".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    /// Tests a graph that looks like the below, tildes indicate a
    /// capability route. Route point toward the target of the capability
    /// offer. The manifest constructed is for 'P'.
    ///       P
    ///    ___|___
    ///  /  / | \  \
    /// e<~c<~a~>b~>d
    ///     \      /
    ///      *>~~>*
    #[fuchsia::test]
    fn test_complex_routing() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("childAService")
                    .source_static_child("childA")
                    .target_static_child("childB"),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childAService")
                    .source_static_child("childA")
                    .target_static_child("childC"),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childBService")
                    .source_static_child("childB")
                    .target_static_child("childD"),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childAService")
                    .source_static_child("childC")
                    .target_static_child("childD"),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("childAService")
                    .source_static_child("childC")
                    .target_static_child("childE"),
            )
            .child_default("childA")
            .child_default("childB")
            .child_default("childC")
            .child_default("childD")
            .child_default("childE")
            .build()
            .into();
        let dynamic_offers = vec![];
        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childD".into(), None),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Child("childE".into(), None),
            DependencyNode::Child("childC".into(), None),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_service_from_collection() {
        let fidl_decl = ComponentDeclBuilder::new()
            .collection_default("coll")
            .child_default("static_child")
            .offer(
                OfferBuilder::service()
                    .name("service_capability")
                    .source(OfferSource::Collection("coll".parse().unwrap()))
                    .target_static_child("static_child"),
            )
            .build()
            .into();
        let dynamic_offers = vec![];

        let sorted_map =
            process_deps_test(&fidl_decl, &vec![("dynamic_child", "coll")], &dynamic_offers)
                .topological_sort()
                .unwrap()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("static_child".into(), None),
            DependencyNode::Collection("coll".into()),
            DependencyNode::Child("dynamic_child".into(), Some("coll".into())),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_use_service_from_collection_with_multiple_instances() {
        let fidl_decl = ComponentDeclBuilder::new()
            .collection_default("coll")
            .use_(
                UseBuilder::service()
                    .name("service_capability")
                    .source(UseSource::Collection("coll".parse().unwrap())),
            )
            .build()
            .into();
        let dynamic_offers = vec![];

        let sorted_map = process_deps_test(
            &fidl_decl,
            &vec![("dynamic_child1", "coll"), ("dynamic_child2", "coll")],
            &dynamic_offers,
        )
        .topological_sort()
        .unwrap()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Self_,
            DependencyNode::Child("dynamic_child1".into(), Some("coll".into())),
            DependencyNode::Child("dynamic_child2".into(), Some("coll".into())),
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_service_from_collection_with_multiple_instances() {
        let fidl_decl = ComponentDeclBuilder::new()
            .collection_default("coll")
            .child_default("static_child")
            .offer(
                OfferBuilder::service()
                    .name("service_capability")
                    .source(OfferSource::Collection("coll".parse().unwrap()))
                    .target_static_child("static_child"),
            )
            .build()
            .into();
        let dynamic_offers = vec![];

        let sorted_map = process_deps_test(
            &fidl_decl,
            &vec![("dynamic_child1", "coll"), ("dynamic_child2", "coll")],
            &dynamic_offers,
        )
        .topological_sort()
        .unwrap()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("static_child".into(), None),
            DependencyNode::Collection("coll".into()),
            DependencyNode::Child("dynamic_child1".into(), Some("coll".into())),
            DependencyNode::Child("dynamic_child2".into(), Some("coll".into())),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_service_dependency_between_collections() {
        let fidl_decl = ComponentDeclBuilder::new()
            .collection_default("coll1")
            .collection_default("coll2")
            .offer(
                OfferBuilder::service()
                    .name("fuchsia.service.FakeService")
                    .source(OfferSource::Collection("coll1".parse().unwrap()))
                    .target(OfferTarget::Collection("coll2".parse().unwrap())),
            )
            .build()
            .into();
        let dynamic_offers = vec![];

        let sorted_map = process_deps_test(
            &fidl_decl,
            &vec![
                ("target_child1", "coll2"),
                ("source_child2", "coll1"),
                ("target_child2", "coll2"),
                ("source_child1", "coll1"),
            ],
            &dynamic_offers,
        )
        .topological_sort()
        .unwrap()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("target_child1".into(), Some("coll2".into())),
            DependencyNode::Child("target_child2".into(), Some("coll2".into())),
            DependencyNode::Collection("coll2".into()),
            DependencyNode::Collection("coll1".into()),
            DependencyNode::Child("source_child1".into(), Some("coll1".into())),
            DependencyNode::Child("source_child2".into(), Some("coll1".into())),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_use_from_child() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("serviceSelf")
                    .source(OfferSource::Self_)
                    .target_static_child("childA")
                    .dependency(DependencyType::Weak),
            )
            .child_default("childA")
            .use_(UseBuilder::protocol().name("test.protocol").source_static_child("childA"))
            .build()
            .into();
        let dynamic_offers = vec![];

        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> =
            vec![DependencyNode::Self_, DependencyNode::Child("childA".into(), None)];
        assert_eq!(ans, sorted_map);
    }

    #[fuchsia::test]
    fn test_use_from_dictionary() {
        let fidl_decl = ComponentDeclBuilder::new()
            .dictionary_default("dict")
            .offer(
                OfferBuilder::protocol()
                    .name("weakService")
                    .source(OfferSource::Self_)
                    .target_static_child("childA")
                    .dependency(DependencyType::Weak),
            )
            .offer(
                OfferBuilder::protocol()
                    .name("serviceA")
                    .source_static_child("childA")
                    .target(OfferTarget::Capability("dict".parse().unwrap())),
            )
            .child_default("childA")
            .use_(
                UseBuilder::protocol()
                    .name("serviceA")
                    .source(UseSource::Self_)
                    .from_dictionary("dict"),
            )
            .build()
            .into();
        let dynamic_offers = vec![];

        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Capability("dict".into()),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
            DependencyNode::Capability("serviceA".into()),
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_use_runner_from_child() {
        let fidl_decl = ComponentDeclBuilder::new_empty_component()
            .child_default("childA")
            .use_(UseBuilder::runner().name("test.runner").source_static_child("childA"))
            .build()
            .into();
        let dynamic_offers = vec![];

        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> =
            vec![DependencyNode::Self_, DependencyNode::Child("childA".into(), None)];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_use_from_some_children() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("serviceSelf")
                    .source(OfferSource::Self_)
                    .target_static_child("childA")
                    .dependency(DependencyType::Weak),
            )
            .child_default("childA")
            .child_default("childB")
            .use_(UseBuilder::protocol().name("test.protocol").source_static_child("childA"))
            .build()
            .into();
        let dynamic_offers = vec![];

        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Self_,
            DependencyNode::Child("childA".into(), None),
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_use_from_child_offer_storage() {
        let fidl_decl = ComponentDeclBuilder::new()
            .capability(
                CapabilityBuilder::storage()
                    .name("cdata")
                    .source(StorageDirectorySource::Child("childB".into()))
                    .backing_dir("directory"),
            )
            .capability(
                CapabilityBuilder::storage()
                    .name("pdata")
                    .source(StorageDirectorySource::Parent)
                    .backing_dir("directory"),
            )
            .offer(
                OfferBuilder::storage()
                    .name("cdata")
                    .source(OfferSource::Self_)
                    .target_static_child("childA"),
            )
            .offer(
                OfferBuilder::storage()
                    .name("pdata")
                    .source(OfferSource::Self_)
                    .target_static_child("childA"),
            )
            .child_default("childA")
            .child_default("childB")
            .use_(UseBuilder::protocol().name("test.protocol").source_static_child("childA"))
            .build()
            .into();
        let dynamic_offers = vec![];

        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Self_,
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Capability("cdata".into()),
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Capability("pdata".into()),
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_use_from_child_weak() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("serviceSelf")
                    .source(OfferSource::Self_)
                    .target_static_child("childA"),
            )
            .child_default("childA")
            .use_(
                UseBuilder::protocol()
                    .name("test.protocol")
                    .source_static_child("childA")
                    .dependency(DependencyType::Weak),
            )
            .build()
            .into();

        let dynamic_offers = vec![];

        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> =
            vec![DependencyNode::Child("childA".into(), None), DependencyNode::Self_];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_use_from_some_children_weak() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::protocol()
                    .name("serviceSelf")
                    .source(OfferSource::Self_)
                    .target_static_child("childA")
                    .dependency(DependencyType::Weak),
            )
            .child_default("childA")
            .child_default("childB")
            .use_(UseBuilder::protocol().name("test.protocol").source_static_child("childA"))
            .use_(
                UseBuilder::protocol()
                    .name("test.protocol2")
                    .source_static_child("childB")
                    .dependency(DependencyType::Weak),
            )
            .build()
            .into();
        let dynamic_offers = vec![];

        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Self_,
            DependencyNode::Child("childA".into(), None),
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    fn test_resolver_capability_creates_dependency() {
        let fidl_decl = ComponentDeclBuilder::new()
            .offer(
                OfferBuilder::resolver()
                    .name("resolver")
                    .source_static_child("childA")
                    .target_static_child("childB"),
            )
            .child_default("childA")
            .child_default("childB")
            .build()
            .into();
        let dynamic_offers = vec![];

        let sorted_map = process_deps_test(&fidl_decl, &vec![], &dynamic_offers)
            .topological_sort()
            .unwrap()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let ans: Vec<DependencyNode> = vec![
            DependencyNode::Child("childB".into(), None),
            DependencyNode::Child("childA".into(), None),
            DependencyNode::Self_,
        ];
        assert_eq!(ans, sorted_map)
    }

    #[fuchsia::test]
    async fn action_shutdown_blocks_stop() {
        let test = ActionsTest::new("root", vec![], None).await;
        let component = test.model.root().clone();

        let (mock_shutdown_barrier, mock_shutdown_action) = MockAction::new(ActionKey::Shutdown);

        // Register some actions, and get notifications.
        let shutdown_notifier = component.actions().register_no_wait(mock_shutdown_action).await;
        let stop_notifier = component.actions().register_no_wait(StopAction::new(false)).await;

        // The stop action should be blocked on the shutdown action completing.
        assert!(stop_notifier.fut.peek().is_none());

        // allow the shutdown action to finish running
        mock_shutdown_barrier.send(Ok(())).unwrap();
        shutdown_notifier.await.expect("shutdown failed unexpectedly");

        // The stop action should now finish running.
        stop_notifier.await.expect("stop failed unexpectedly");
    }

    #[fuchsia::test]
    async fn action_shutdown_stop_stop() {
        let test = ActionsTest::new("root", vec![], None).await;
        let component = test.model.root().clone();
        let (mock_shutdown_barrier, mock_shutdown_action) = MockAction::new(ActionKey::Shutdown);

        // Register some actions, and get notifications.
        let shutdown_notifier = component.actions().register_no_wait(mock_shutdown_action).await;
        let stop_notifier_1 = component.actions().register_no_wait(StopAction::new(false)).await;
        let stop_notifier_2 = component.actions().register_no_wait(StopAction::new(false)).await;

        // The stop action should be blocked on the shutdown action completing.
        assert!(stop_notifier_1.fut.peek().is_none());
        assert!(stop_notifier_2.fut.peek().is_none());

        // allow the shutdown action to finish running
        mock_shutdown_barrier.send(Ok(())).unwrap();
        shutdown_notifier.await.expect("shutdown failed unexpectedly");

        // The stop actions should now finish running.
        stop_notifier_1.await.expect("stop failed unexpectedly");
        stop_notifier_2.await.expect("stop failed unexpectedly");
    }

    #[fuchsia::test]
    async fn shutdown_one_component() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();

        // Start the component. This should cause the component to have an `Execution`.
        let component = test.look_up(["a"].try_into().unwrap()).await;
        root.start_instance(&component.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");
        assert!(component.is_started().await);
        let a_info = ComponentInfo::new(component.clone()).await;

        // Register shutdown action, and wait for it. Component should shut down (no more
        // `Execution`).
        ActionsManager::register(
            a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        a_info.check_is_shut_down(&test.runner).await;

        // Trying to start the component should fail because it's shut down.
        root.start_instance(&a_info.component.moniker, &StartReason::Eager)
            .await
            .expect_err("successfully bound to a after shutdown");

        // Shut down the component again. This succeeds, but has no additional effect.
        ActionsManager::register(
            a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        a_info.check_is_shut_down(&test.runner).await;
    }

    #[fuchsia::test]
    async fn shutdown_collection() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("container").build()),
            (
                "container",
                ComponentDeclBuilder::new().collection_default("coll").child_default("c").build(),
            ),
            ("a", component_decl_with_test_runner()),
            ("b", component_decl_with_test_runner()),
            ("c", component_decl_with_test_runner()),
        ];
        let test =
            ActionsTest::new("root", components, Some(["container"].try_into().unwrap())).await;
        let root = test.model.root();

        // Create dynamic instances in "coll".
        test.create_dynamic_child("coll", "a").await;
        test.create_dynamic_child("coll", "b").await;

        // Start the components. This should cause them to have an `Execution`.
        let component_container = test.look_up(["container"].try_into().unwrap()).await;
        let component_a = test.look_up(["container", "coll:a"].try_into().unwrap()).await;
        let component_b = test.look_up(["container", "coll:b"].try_into().unwrap()).await;
        let component_c = test.look_up(["container", "c"].try_into().unwrap()).await;
        root.start_instance(&component_container.moniker, &StartReason::Eager)
            .await
            .expect("could not start container");
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start coll:a");
        root.start_instance(&component_b.moniker, &StartReason::Eager)
            .await
            .expect("could not start coll:b");
        root.start_instance(&component_c.moniker, &StartReason::Eager)
            .await
            .expect("could not start c");
        assert!(component_container.is_started().await);
        assert!(component_a.is_started().await);
        assert!(component_b.is_started().await);
        assert!(component_c.is_started().await);
        assert!(has_child(&component_container, "coll:a").await);
        assert!(has_child(&component_container, "coll:b").await);

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_container_info = ComponentInfo::new(component_container).await;

        // Register shutdown action, and wait for it. Components should shut down (no more
        // `Execution`). Also, the instances in the collection should have been destroyed because
        // they were transient.
        ActionsManager::register(
            component_container_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_container_info.check_is_shut_down(&test.runner).await;
        assert!(!has_child(&component_container_info.component, "coll:a").await);
        assert!(!has_child(&component_container_info.component, "coll:b").await);
        assert!(has_child(&component_container_info.component, "c").await);
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;

        // Verify events.
        {
            let mut events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) | Lifecycle::Destroy(_) => true,
                    _ => false,
                })
                .collect();
            // The leaves could be stopped in any order.
            let mut next: Vec<_> = events.drain(0..3).collect();
            next.sort_unstable();
            let expected: Vec<_> = vec![
                Lifecycle::Stop(["container", "c"].try_into().unwrap()),
                Lifecycle::Stop(["container", "coll:a"].try_into().unwrap()),
                Lifecycle::Stop(["container", "coll:b"].try_into().unwrap()),
            ];
            assert_eq!(next, expected);

            // These components were destroyed because they lived in a transient collection.
            let mut next: Vec<_> = events.drain(0..2).collect();
            next.sort_unstable();
            let expected: Vec<_> = vec![
                Lifecycle::Destroy(["container", "coll:a"].try_into().unwrap()),
                Lifecycle::Destroy(["container", "coll:b"].try_into().unwrap()),
            ];
            assert_eq!(next, expected);
        }
    }

    #[fuchsia::test]
    async fn shutdown_dynamic_offers() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("container").build()),
            (
                "container",
                ComponentDeclBuilder::new()
                    .collection(
                        CollectionBuilder::new()
                            .name("coll")
                            .allowed_offers(AllowedOffers::StaticAndDynamic),
                    )
                    .child_default("c")
                    .offer(
                        OfferBuilder::protocol()
                            .name("static_offer_source")
                            .target_name("static_offer_target")
                            .source(OfferSource::Child(ChildRef {
                                name: "c".parse().unwrap(),
                                collection: None,
                            }))
                            .target(OfferTarget::Collection("coll".parse().unwrap())),
                    )
                    .build(),
            ),
            ("a", component_decl_with_test_runner()),
            ("b", component_decl_with_test_runner()),
            ("c", component_decl_with_test_runner()),
        ];
        let test =
            ActionsTest::new("root", components, Some(["container"].try_into().unwrap())).await;
        let root = test.model.root();

        // Create dynamic instances in "coll".
        test.create_dynamic_child("coll", "a").await;
        test.create_dynamic_child_with_args(
            "coll",
            "b",
            fcomponent::CreateChildArgs {
                dynamic_offers: Some(vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                    source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "a".into(),
                        collection: Some("coll".into()),
                    })),
                    source_name: Some("dyn_offer_source_name".to_string()),
                    target_name: Some("dyn_offer_target_name".to_string()),
                    dependency_type: Some(fdecl::DependencyType::Strong),
                    ..Default::default()
                })]),
                ..Default::default()
            },
        )
        .await
        .expect("failed to create child");

        // Start the components. This should cause them to have an `Execution`.
        let component_container = test.look_up(["container"].try_into().unwrap()).await;
        let component_a = test.look_up(["container", "coll:a"].try_into().unwrap()).await;
        let component_b = test.look_up(["container", "coll:b"].try_into().unwrap()).await;
        let component_c = test.look_up(["container", "c"].try_into().unwrap()).await;
        root.start_instance(&component_container.moniker, &StartReason::Eager)
            .await
            .expect("could not start container");
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start coll:a");
        root.start_instance(&component_b.moniker, &StartReason::Eager)
            .await
            .expect("could not start coll:b");
        root.start_instance(&component_c.moniker, &StartReason::Eager)
            .await
            .expect("could not start c");
        assert!(component_container.is_started().await);
        assert!(component_a.is_started().await);
        assert!(component_b.is_started().await);
        assert!(component_c.is_started().await);
        assert!(has_child(&component_container, "coll:a").await);
        assert!(has_child(&component_container, "coll:b").await);

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_container_info = ComponentInfo::new(component_container).await;

        // Register shutdown action, and wait for it. Components should shut down (no more
        // `Execution`). Also, the instances in the collection should have been destroyed because
        // they were transient.
        ActionsManager::register(
            component_container_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_container_info.check_is_shut_down(&test.runner).await;
        assert!(!has_child(&component_container_info.component, "coll:a").await);
        assert!(!has_child(&component_container_info.component, "coll:b").await);
        assert!(has_child(&component_container_info.component, "c").await);
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;

        // Verify events.
        {
            let mut events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) | Lifecycle::Destroy(_) => true,
                    _ => false,
                })
                .collect();

            pretty_assertions::assert_eq!(
                vec![
                    Lifecycle::Stop(["container", "coll:b"].try_into().unwrap()),
                    Lifecycle::Stop(["container", "coll:a"].try_into().unwrap()),
                    Lifecycle::Stop(["container", "c"].try_into().unwrap()),
                ],
                events.drain(0..3).collect::<Vec<_>>()
            );

            // The order here is nondeterministic.
            pretty_assertions::assert_eq!(
                btreeset![
                    Lifecycle::Destroy(["container", "coll:b"].try_into().unwrap()),
                    Lifecycle::Destroy(["container", "coll:a"].try_into().unwrap()),
                ],
                events.drain(0..2).collect::<BTreeSet<_>>()
            );
            pretty_assertions::assert_eq!(
                vec![Lifecycle::Stop(["container"].try_into().unwrap()),],
                events
            );
        }
    }

    #[fuchsia::test]
    async fn shutdown_not_started() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child_default("b").build()),
            ("b", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        assert!(!component_a.is_started().await);
        assert!(!component_b.is_started().await);

        // Register shutdown action on "a", and wait for it.
        ActionsManager::register(component_a.clone(), ShutdownAction::new(ShutdownType::Instance))
            .await
            .expect("shutdown failed");
        assert!(execution_is_shut_down(&component_a).await);
        assert!(execution_is_shut_down(&component_b).await);

        // Now "a" is shut down. There should be no events though because the component was
        // never started.
        ActionsManager::register(component_a.clone(), ShutdownAction::new(ShutdownType::Instance))
            .await
            .expect("shutdown failed");
        assert!(execution_is_shut_down(&component_a).await);
        assert!(execution_is_shut_down(&component_b).await);
        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            assert_eq!(events, Vec::<Lifecycle>::new());
        }
    }

    #[fuchsia::test]
    async fn shutdown_not_resolved() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child_default("b").build()),
            ("b", ComponentDeclBuilder::new().child_default("c").build()),
            ("c", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");
        assert!(component_a.is_started().await);

        // Register shutdown action on "a", and wait for it.
        ActionsManager::register(component_a.clone(), ShutdownAction::new(ShutdownType::Instance))
            .await
            .expect("shutdown failed");
        assert!(execution_is_shut_down(&component_a).await);
        // Get component without resolving it.
        let component_b = {
            let state = component_a.lock_state().await;
            match *state {
                InstanceState::Shutdown(ref state, _) => {
                    state.children.get_by_name("b").expect("child b not found").clone()
                }
                _ => panic!("not shutdown"),
            }
        };
        assert!(execution_is_shut_down(&component_b).await);

        // Now "a" is shut down. There should be no event for "b" because it was never started
        // (or resolved).
        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            assert_eq!(events, vec![Lifecycle::Stop(["a"].try_into().unwrap())]);
        }
    }

    /// Shut down `a`:
    ///  a
    ///   \
    ///    b
    ///   / \
    ///  c   d
    #[fuchsia::test]
    async fn shutdown_hierarchy() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child(ChildBuilder::new().name("b").eager()).build()),
            (
                "b",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("c").eager())
                    .child(ChildBuilder::new().name("d").eager())
                    .build(),
            ),
            ("c", component_decl_with_test_runner()),
            ("d", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let component_c = test.look_up(["a", "b", "c"].try_into().unwrap()).await;
        let component_d = test.look_up(["a", "b", "d"].try_into().unwrap()).await;

        // Component startup was eager, so they should all have an `Execution`.
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");
        assert!(component_a.is_started().await);
        assert!(component_b.is_started().await);
        assert!(component_c.is_started().await);
        assert!(component_d.is_started().await);

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_c_info = ComponentInfo::new(component_c).await;
        let component_d_info = ComponentInfo::new(component_d).await;

        // Register shutdown action on "a", and wait for it. This should cause all components
        // to shut down, in bottom-up order.
        ActionsManager::register(
            component_a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_c_info.check_is_shut_down(&test.runner).await;
        component_d_info.check_is_shut_down(&test.runner).await;
        {
            let mut events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            let mut first: Vec<_> = events.drain(0..2).collect();
            first.sort_unstable();
            let expected: Vec<_> = vec![
                Lifecycle::Stop(["a", "b", "c"].try_into().unwrap()),
                Lifecycle::Stop(["a", "b", "d"].try_into().unwrap()),
            ];
            assert_eq!(first, expected);
            assert_eq!(
                events,
                vec![
                    Lifecycle::Stop(["a", "b"].try_into().unwrap()),
                    Lifecycle::Stop(["a"].try_into().unwrap())
                ]
            );
        }
    }

    /// Shut down `a`:
    ///   a
    ///    \
    ///     b
    ///   / | \
    ///  c<-d->e
    /// In this case C and E use a service provided by d
    #[fuchsia::test]
    async fn shutdown_with_multiple_deps() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child(ChildBuilder::new().name("b").eager()).build()),
            (
                "b",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("c").eager())
                    .child(ChildBuilder::new().name("d").eager())
                    .child(ChildBuilder::new().name("e").eager())
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceD")
                            .source_static_child("d")
                            .target_static_child("c"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceD")
                            .source_static_child("d")
                            .target_static_child("e"),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new().use_(UseBuilder::protocol().name("serviceD")).build(),
            ),
            (
                "d",
                ComponentDeclBuilder::new()
                    .protocol_default("serviceD")
                    .expose(ExposeBuilder::protocol().name("serviceD").source(ExposeSource::Self_))
                    .build(),
            ),
            (
                "e",
                ComponentDeclBuilder::new().use_(UseBuilder::protocol().name("serviceD")).build(),
            ),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let component_c = test.look_up(["a", "b", "c"].try_into().unwrap()).await;
        let component_d = test.look_up(["a", "b", "d"].try_into().unwrap()).await;
        let component_e = test.look_up(["a", "b", "e"].try_into().unwrap()).await;

        // Component startup was eager, so they should all have an `Execution`.
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");
        assert!(component_a.is_started().await);
        assert!(component_b.is_started().await);
        assert!(component_c.is_started().await);
        assert!(component_d.is_started().await);
        assert!(component_e.is_started().await);

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_c_info = ComponentInfo::new(component_c).await;
        let component_d_info = ComponentInfo::new(component_d).await;
        let component_e_info = ComponentInfo::new(component_e).await;

        // Register shutdown action on "a", and wait for it. This should cause all components
        // to shut down, in bottom-up order.
        ActionsManager::register(
            component_a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_c_info.check_is_shut_down(&test.runner).await;
        component_d_info.check_is_shut_down(&test.runner).await;
        component_e_info.check_is_shut_down(&test.runner).await;

        {
            let mut events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            let mut first: Vec<_> = events.drain(0..2).collect();
            first.sort_unstable();
            let mut expected: Vec<_> = vec![
                Lifecycle::Stop(["a", "b", "c"].try_into().unwrap()),
                Lifecycle::Stop(["a", "b", "e"].try_into().unwrap()),
            ];
            assert_eq!(first, expected);

            let next: Vec<_> = events.drain(0..1).collect();
            expected = vec![Lifecycle::Stop(["a", "b", "d"].try_into().unwrap())];
            assert_eq!(next, expected);

            assert_eq!(
                events,
                vec![
                    Lifecycle::Stop(["a", "b"].try_into().unwrap()),
                    Lifecycle::Stop(["a"].try_into().unwrap())
                ]
            );
        }
    }

    /// Shut down `a`:
    ///    a
    ///     \
    ///      b
    ///   / / \  \
    ///  c<-d->e->f
    /// In this case C and E use a service provided by D and
    /// F uses a service provided by E, shutdown order should be
    /// {F}, {C, E}, {D}, {B}, {A}
    /// Note that C must stop before D, but may stop before or after
    /// either of F and E.
    #[fuchsia::test]
    async fn shutdown_with_multiple_out_and_longer_chain() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child(ChildBuilder::new().name("b").eager()).build()),
            (
                "b",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("c").eager())
                    .child(ChildBuilder::new().name("d").eager())
                    .child(ChildBuilder::new().name("e").eager())
                    .child(ChildBuilder::new().name("f").eager())
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceD")
                            .source_static_child("d")
                            .target_static_child("c"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceD")
                            .source_static_child("d")
                            .target_static_child("e"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceE")
                            .source_static_child("e")
                            .target_static_child("f"),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new().use_(UseBuilder::protocol().name("serviceD")).build(),
            ),
            (
                "d",
                ComponentDeclBuilder::new()
                    .protocol_default("serviceD")
                    .expose(ExposeBuilder::protocol().name("serviceD").source(ExposeSource::Self_))
                    .build(),
            ),
            (
                "e",
                ComponentDeclBuilder::new()
                    .protocol_default("serviceE")
                    .use_(UseBuilder::protocol().name("serviceD"))
                    .expose(ExposeBuilder::protocol().name("serviceE").source(ExposeSource::Self_))
                    .build(),
            ),
            (
                "f",
                ComponentDeclBuilder::new().use_(UseBuilder::protocol().name("serviceE")).build(),
            ),
        ];
        let moniker_a: Moniker = ["a"].try_into().unwrap();
        let moniker_b: Moniker = ["a", "b"].try_into().unwrap();
        let moniker_c: Moniker = ["a", "b", "c"].try_into().unwrap();
        let moniker_d: Moniker = ["a", "b", "d"].try_into().unwrap();
        let moniker_e: Moniker = ["a", "b", "e"].try_into().unwrap();
        let moniker_f: Moniker = ["a", "b", "f"].try_into().unwrap();
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();
        let component_a = test.look_up(moniker_a.clone()).await;
        let component_b = test.look_up(moniker_b.clone()).await;
        let component_c = test.look_up(moniker_c.clone()).await;
        let component_d = test.look_up(moniker_d.clone()).await;
        let component_e = test.look_up(moniker_e.clone()).await;
        let component_f = test.look_up(moniker_f.clone()).await;

        // Component startup was eager, so they should all have an `Execution`.
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");
        assert!(component_a.is_started().await);
        assert!(component_b.is_started().await);
        assert!(component_c.is_started().await);
        assert!(component_d.is_started().await);
        assert!(component_e.is_started().await);
        assert!(component_f.is_started().await);

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_c_info = ComponentInfo::new(component_c).await;
        let component_d_info = ComponentInfo::new(component_d).await;
        let component_e_info = ComponentInfo::new(component_e).await;
        let component_f_info = ComponentInfo::new(component_f).await;

        // Register shutdown action on "a", and wait for it. This should cause all components
        // to shut down, in bottom-up order.
        ActionsManager::register(
            component_a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_c_info.check_is_shut_down(&test.runner).await;
        component_d_info.check_is_shut_down(&test.runner).await;
        component_e_info.check_is_shut_down(&test.runner).await;
        component_f_info.check_is_shut_down(&test.runner).await;

        let mut comes_after: HashMap<Moniker, Vec<Moniker>> = HashMap::new();
        comes_after.insert(moniker_a.clone(), vec![moniker_b.clone()]);
        // technically we could just depend on 'D' since it is the last of b's
        // children, but we add all the children for resilence against the
        // future
        comes_after.insert(
            moniker_b.clone(),
            vec![moniker_c.clone(), moniker_d.clone(), moniker_e.clone(), moniker_f.clone()],
        );
        comes_after.insert(moniker_d.clone(), vec![moniker_c.clone(), moniker_e.clone()]);
        comes_after.insert(moniker_c.clone(), vec![]);
        comes_after.insert(moniker_e.clone(), vec![moniker_f.clone()]);
        comes_after.insert(moniker_f.clone(), vec![]);
        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();

            for e in events {
                match e {
                    Lifecycle::Stop(moniker) => match comes_after.remove(&moniker) {
                        Some(dependents) => {
                            for d in dependents {
                                if comes_after.contains_key(&d) {
                                    panic!("{} stopped before its dependent {}", moniker, d);
                                }
                            }
                        }
                        None => {
                            panic!("{} was unknown or shut down more than once", moniker);
                        }
                    },
                    _ => {
                        panic!("Unexpected lifecycle type");
                    }
                }
            }
        }
    }

    /// Shut down `a`:
    ///           a
    ///
    ///           |
    ///
    ///     +---- b ----+
    ///    /             \
    ///   /     /   \     \
    ///
    ///  c <~~ d ~~> e ~~> f
    ///          \       /
    ///           +~~>~~+
    /// In this case C and E use a service provided by D and
    /// F uses a services provided by E and D, shutdown order should be F must
    /// stop before E and {C,E,F} must stop before D. C may stop before or
    /// after either of {F, E}.
    #[fuchsia::test]
    async fn shutdown_with_multiple_out_multiple_in() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child(ChildBuilder::new().name("b").eager()).build()),
            (
                "b",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("c").eager())
                    .child(ChildBuilder::new().name("d").eager())
                    .child(ChildBuilder::new().name("e").eager())
                    .child(ChildBuilder::new().name("f").eager())
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceD")
                            .source_static_child("d")
                            .target_static_child("c"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceD")
                            .source_static_child("d")
                            .target_static_child("e"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceD")
                            .source_static_child("d")
                            .target_static_child("f"),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceE")
                            .source_static_child("e")
                            .target_static_child("f"),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new().use_(UseBuilder::protocol().name("serviceD")).build(),
            ),
            (
                "d",
                ComponentDeclBuilder::new()
                    .protocol_default("serviceD")
                    .expose(ExposeBuilder::protocol().name("serviceD").source(ExposeSource::Self_))
                    .build(),
            ),
            (
                "e",
                ComponentDeclBuilder::new()
                    .protocol_default("serviceE")
                    .use_(UseBuilder::protocol().name("serviceE"))
                    .expose(ExposeBuilder::protocol().name("serviceE").source(ExposeSource::Self_))
                    .build(),
            ),
            (
                "f",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("serviceE"))
                    .use_(UseBuilder::protocol().name("serviceD"))
                    .build(),
            ),
        ];
        let moniker_a: Moniker = ["a"].try_into().unwrap();
        let moniker_b: Moniker = ["a", "b"].try_into().unwrap();
        let moniker_c: Moniker = ["a", "b", "c"].try_into().unwrap();
        let moniker_d: Moniker = ["a", "b", "d"].try_into().unwrap();
        let moniker_e: Moniker = ["a", "b", "e"].try_into().unwrap();
        let moniker_f: Moniker = ["a", "b", "f"].try_into().unwrap();
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();
        let component_a = test.look_up(moniker_a.clone()).await;
        let component_b = test.look_up(moniker_b.clone()).await;
        let component_c = test.look_up(moniker_c.clone()).await;
        let component_d = test.look_up(moniker_d.clone()).await;
        let component_e = test.look_up(moniker_e.clone()).await;
        let component_f = test.look_up(moniker_f.clone()).await;

        // Component startup was eager, so they should all have an `Execution`.
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");
        assert!(component_a.is_started().await);
        assert!(component_b.is_started().await);
        assert!(component_c.is_started().await);
        assert!(component_d.is_started().await);
        assert!(component_e.is_started().await);
        assert!(component_f.is_started().await);

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_c_info = ComponentInfo::new(component_c).await;
        let component_d_info = ComponentInfo::new(component_d).await;
        let component_e_info = ComponentInfo::new(component_e).await;
        let component_f_info = ComponentInfo::new(component_f).await;

        // Register shutdown action on "a", and wait for it. This should cause all components
        // to shut down, in bottom-up order.
        ActionsManager::register(
            component_a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_c_info.check_is_shut_down(&test.runner).await;
        component_d_info.check_is_shut_down(&test.runner).await;
        component_e_info.check_is_shut_down(&test.runner).await;
        component_f_info.check_is_shut_down(&test.runner).await;

        let mut comes_after: HashMap<Moniker, Vec<Moniker>> = HashMap::new();
        comes_after.insert(moniker_a.clone(), vec![moniker_b.clone()]);
        // technically we could just depend on 'D' since it is the last of b's
        // children, but we add all the children for resilence against the
        // future
        comes_after.insert(
            moniker_b.clone(),
            vec![moniker_c.clone(), moniker_d.clone(), moniker_e.clone(), moniker_f.clone()],
        );
        comes_after.insert(
            moniker_d.clone(),
            vec![moniker_c.clone(), moniker_e.clone(), moniker_f.clone()],
        );
        comes_after.insert(moniker_c.clone(), vec![]);
        comes_after.insert(moniker_e.clone(), vec![moniker_f.clone()]);
        comes_after.insert(moniker_f.clone(), vec![]);
        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();

            for e in events {
                match e {
                    Lifecycle::Stop(moniker) => {
                        let dependents = comes_after.remove(&moniker).unwrap_or_else(|| {
                            panic!("{} was unknown or shut down more than once", moniker)
                        });
                        for d in dependents {
                            if comes_after.contains_key(&d) {
                                panic!("{} stopped before its dependent {}", moniker, d);
                            }
                        }
                    }
                    _ => {
                        panic!("Unexpected lifecycle type");
                    }
                }
            }
        }
    }

    /// Shut down `a`:
    ///  a
    ///   \
    ///    b
    ///   / \
    ///  c-->d
    /// In this case D uses a resource exposed by C
    #[fuchsia::test]
    async fn shutdown_with_dependency() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child(ChildBuilder::new().name("b").eager()).build()),
            (
                "b",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("c").eager())
                    .child(ChildBuilder::new().name("d").eager())
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceC")
                            .source_static_child("c")
                            .target_static_child("d"),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .protocol_default("serviceC")
                    .expose(ExposeBuilder::protocol().name("serviceC").source(ExposeSource::Self_))
                    .build(),
            ),
            (
                "d",
                ComponentDeclBuilder::new().use_(UseBuilder::protocol().name("serviceC")).build(),
            ),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let component_c = test.look_up(["a", "b", "c"].try_into().unwrap()).await;
        let component_d = test.look_up(["a", "b", "d"].try_into().unwrap()).await;

        // Component startup was eager, so they should all have an `Execution`.
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_c_info = ComponentInfo::new(component_c).await;
        let component_d_info = ComponentInfo::new(component_d).await;

        // Register shutdown action on "a", and wait for it. This should cause all components
        // to shut down, in bottom-up and dependency order.
        ActionsManager::register(
            component_a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_c_info.check_is_shut_down(&test.runner).await;
        component_d_info.check_is_shut_down(&test.runner).await;

        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            let expected: Vec<_> = vec![
                Lifecycle::Stop(["a", "b", "d"].try_into().unwrap()),
                Lifecycle::Stop(["a", "b", "c"].try_into().unwrap()),
                Lifecycle::Stop(["a", "b"].try_into().unwrap()),
                Lifecycle::Stop(["a"].try_into().unwrap()),
            ];
            assert_eq!(events, expected);
        }
    }

    /// Shut down `a`:
    ///  a
    ///   \
    ///    b
    ///   / \
    ///  c-->d
    /// In this case D uses a resource exposed by C, routed through a dictionary defined by B
    #[fuchsia::test]
    async fn shutdown_with_dictionary_dependency() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child(ChildBuilder::new().name("b").eager()).build()),
            (
                "b",
                ComponentDeclBuilder::new()
                    .dictionary_default("dict")
                    .child(ChildBuilder::new().name("c").eager())
                    .child(ChildBuilder::new().name("d").eager())
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceC")
                            .source_static_child("c")
                            .target(OfferTarget::Capability("dict".parse().unwrap())),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceC")
                            .source(OfferSource::Self_)
                            .from_dictionary("dict")
                            .target_static_child("d"),
                    )
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .protocol_default("serviceC")
                    .expose(ExposeBuilder::protocol().name("serviceC").source(ExposeSource::Self_))
                    .build(),
            ),
            (
                "d",
                ComponentDeclBuilder::new().use_(UseBuilder::protocol().name("serviceC")).build(),
            ),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let component_c = test.look_up(["a", "b", "c"].try_into().unwrap()).await;
        let component_d = test.look_up(["a", "b", "d"].try_into().unwrap()).await;

        // Component startup was eager, so they should all have an `Execution`.
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_c_info = ComponentInfo::new(component_c).await;
        let component_d_info = ComponentInfo::new(component_d).await;

        // Register shutdown action on "a", and wait for it. This should cause all components
        // to shut down, in bottom-up and dependency order.
        ActionsManager::register(
            component_a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_c_info.check_is_shut_down(&test.runner).await;
        component_d_info.check_is_shut_down(&test.runner).await;

        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            let expected: Vec<_> = vec![
                Lifecycle::Stop(["a", "b", "d"].try_into().unwrap()),
                Lifecycle::Stop(["a", "b", "c"].try_into().unwrap()),
                Lifecycle::Stop(["a", "b"].try_into().unwrap()),
                Lifecycle::Stop(["a"].try_into().unwrap()),
            ];
            assert_eq!(events, expected);
        }
    }

    /// Shut down `a`:
    ///   a     (a use b)
    ///  / \
    /// b    c
    /// In this case, c shuts down first, then a, then b.
    #[fuchsia::test]
    async fn shutdown_use_from_child() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            (
                "a",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("b").eager())
                    .child(ChildBuilder::new().name("c").eager())
                    .use_(UseBuilder::protocol().source_static_child("b").name("serviceC"))
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .protocol_default("serviceC")
                    .expose(ExposeBuilder::protocol().name("serviceC").source(ExposeSource::Self_))
                    .build(),
            ),
            ("c", ComponentDeclBuilder::new().build()),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let component_c = test.look_up(["a", "c"].try_into().unwrap()).await;

        // Component startup was eager, so they should all have an `Execution`.
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_c_info = ComponentInfo::new(component_c).await;

        // Register shutdown action on "a", and wait for it. This should cause all components
        // to shut down.
        ActionsManager::register(
            component_a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_c_info.check_is_shut_down(&test.runner).await;

        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            let expected: Vec<_> = vec![
                Lifecycle::Stop(["a", "c"].try_into().unwrap()),
                Lifecycle::Stop(["a"].try_into().unwrap()),
                Lifecycle::Stop(["a", "b"].try_into().unwrap()),
            ];
            assert_eq!(events, expected);
        }
    }

    /// Shut down `a`:
    ///   a     (a uses runner from b)
    ///  / \
    /// b    c
    /// In this case, c shuts down first, then a, then b.
    #[fuchsia::test]
    async fn test_shutdown_use_runner_from_child() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            (
                "a",
                ComponentDeclBuilder::new_empty_component()
                    .child(ChildBuilder::new().name("b").eager())
                    .child(ChildBuilder::new().name("c").eager())
                    .use_(UseBuilder::runner().source_static_child("b").name("test.runner"))
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .expose(ExposeBuilder::runner().name("test.runner").source(ExposeSource::Self_))
                    .capability(CapabilityBuilder::runner().name("test.runner"))
                    .build(),
            ),
            ("c", ComponentDeclBuilder::new().build()),
        ];
        let test = ActionsTest::new("root", components, None).await;

        // Set up component `b`'s outgoing directory to provide the test runner protocol, so that
        // component `a` can also be started. Without this the outgoing directory for `b` will be
        // closed by the mock runner and start requests for `a` will thus be ignored.
        //
        // We need a weak runner because otherwise the runner will hold the outgoing directory's
        // host function, which will hold a reference to the runner, creating a cycle.
        let weak_runner = Arc::downgrade(&test.runner);
        let mut test_runner_out_dir = OutDir::new();
        test_runner_out_dir.add_entry(
            "/svc/fuchsia.component.runner.ComponentRunner".parse().unwrap(),
            vfs::service::endpoint(move |_scope, channel| {
                fuchsia_async::Task::spawn(
                    weak_runner.upgrade().unwrap().handle_stream(
                        fcrunner::ComponentRunnerRequestStream::from_channel(channel),
                    ),
                )
                .detach();
            }),
        );
        test.runner.add_host_fn("test:///b", test_runner_out_dir.host_fn());

        let root = test.model.root();
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let component_c = test.look_up(["a", "c"].try_into().unwrap()).await;

        // Component startup was eager, so they should all have an `Execution`.
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_c_info = ComponentInfo::new(component_c).await;

        // Register shutdown action on "a", and wait for it. This should cause all components
        // to shut down.
        ActionsManager::register(
            component_a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_c_info.check_is_shut_down(&test.runner).await;

        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            let expected: Vec<_> = vec![
                Lifecycle::Stop(["a", "c"].try_into().unwrap()),
                Lifecycle::Stop(["a"].try_into().unwrap()),
                Lifecycle::Stop(["a", "b"].try_into().unwrap()),
            ];
            assert_eq!(events, expected);
        }
    }

    /// Shut down `a`:
    ///   a     (a use b, and b use c)
    ///  / \
    /// b    c
    /// In this case, a shuts down first, then b, then c.
    #[fuchsia::test]
    async fn shutdown_use_from_child_that_uses_from_sibling() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            (
                "a",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("b").eager())
                    .child(ChildBuilder::new().name("c").eager())
                    .use_(UseBuilder::protocol().source_static_child("b").name("serviceB"))
                    .offer(
                        OfferBuilder::protocol()
                            .name("serviceC")
                            .target_name("serviceB")
                            .source_static_child("c")
                            .target_static_child("b"),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .protocol_default("serviceB")
                    .expose(ExposeBuilder::protocol().name("serviceB").source(ExposeSource::Self_))
                    .use_(UseBuilder::protocol().name("serviceC"))
                    .build(),
            ),
            (
                "c",
                ComponentDeclBuilder::new()
                    .protocol_default("serviceC")
                    .expose(ExposeBuilder::protocol().name("serviceC").source(ExposeSource::Self_))
                    .build(),
            ),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let component_c = test.look_up(["a", "c"].try_into().unwrap()).await;

        // Component startup was eager, so they should all have an `Execution`.
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_c_info = ComponentInfo::new(component_c).await;

        // Register shutdown action on "a", and wait for it. This should cause all components
        // to shut down.
        ActionsManager::register(
            component_a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_c_info.check_is_shut_down(&test.runner).await;

        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            let expected: Vec<_> = vec![
                Lifecycle::Stop(["a"].try_into().unwrap()),
                Lifecycle::Stop(["a", "b"].try_into().unwrap()),
                Lifecycle::Stop(["a", "c"].try_into().unwrap()),
            ];
            assert_eq!(events, expected);
        }
    }

    /// Shut down `a`:
    ///   a     (a use b weak)
    ///  / \
    /// b    c
    /// In this case, b or c shutdown first (arbitrary order), then a.
    #[fuchsia::test]
    async fn shutdown_use_from_child_weak() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            (
                "a",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("b").eager())
                    .child(ChildBuilder::new().name("c").eager())
                    .use_(
                        UseBuilder::protocol()
                            .source_static_child("b")
                            .name("serviceC")
                            .dependency(DependencyType::Weak),
                    )
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .protocol_default("serviceC")
                    .expose(ExposeBuilder::protocol().name("serviceC").source(ExposeSource::Self_))
                    .build(),
            ),
            ("c", ComponentDeclBuilder::new().build()),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let component_c = test.look_up(["a", "c"].try_into().unwrap()).await;

        // Component startup was eager, so they should all have an `Execution`.
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start a");

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_c_info = ComponentInfo::new(component_c).await;

        // Register shutdown action on "a", and wait for it. This should cause all components
        // to shut down.
        ActionsManager::register(
            component_a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_c_info.check_is_shut_down(&test.runner).await;

        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            let expected1: Vec<_> = vec![
                Lifecycle::Stop(["a", "c"].try_into().unwrap()),
                Lifecycle::Stop(["a", "b"].try_into().unwrap()),
                Lifecycle::Stop(["a"].try_into().unwrap()),
            ];
            let expected2: Vec<_> = vec![
                Lifecycle::Stop(["a", "b"].try_into().unwrap()),
                Lifecycle::Stop(["a", "c"].try_into().unwrap()),
                Lifecycle::Stop(["a"].try_into().unwrap()),
            ];
            assert!(events == expected1 || events == expected2);
        }
    }

    /// Shut down `b`:
    ///  a
    ///   \
    ///    b
    ///     \
    ///      b
    ///       \
    ///      ...
    ///
    /// `b` is a child of itself, but shutdown should still be able to complete.
    #[fuchsia::test]
    async fn shutdown_self_referential() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child_default("b").build()),
            ("b", ComponentDeclBuilder::new().child_default("b").build()),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let component_b2 = test.look_up(["a", "b", "b"].try_into().unwrap()).await;

        // Start second `b`.
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start b2");
        root.start_instance(&component_b.moniker, &StartReason::Eager)
            .await
            .expect("could not start b2");
        root.start_instance(&component_b2.moniker, &StartReason::Eager)
            .await
            .expect("could not start b2");
        assert!(component_a.is_started().await);
        assert!(component_b.is_started().await);
        assert!(component_b2.is_started().await);

        let component_a_info = ComponentInfo::new(component_a).await;
        let component_b_info = ComponentInfo::new(component_b).await;
        let component_b2_info = ComponentInfo::new(component_b2).await;

        // Register shutdown action on "a", and wait for it. This should cause all components
        // to shut down, in bottom-up and dependency order.
        ActionsManager::register(
            component_a_info.component.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        component_a_info.check_is_shut_down(&test.runner).await;
        component_b_info.check_is_shut_down(&test.runner).await;
        component_b2_info.check_is_shut_down(&test.runner).await;
        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            assert_eq!(
                events,
                vec![
                    Lifecycle::Stop(["a", "b", "b"].try_into().unwrap()),
                    Lifecycle::Stop(["a", "b"].try_into().unwrap()),
                    Lifecycle::Stop(["a"].try_into().unwrap())
                ]
            );
        }
    }

    /// Shut down `a`:
    ///  a
    ///   \
    ///    b
    ///   / \
    ///  c   d
    ///
    /// `a` fails to finish shutdown the first time, but succeeds the second time.
    #[fuchsia::test]
    fn shutdown_error() {
        let mut executor = fasync::TestExecutor::new();
        let mut test_body = Box::pin(async move {
            let components = vec![
                ("root", ComponentDeclBuilder::new().child_default("a").build()),
                (
                    "a",
                    ComponentDeclBuilder::new()
                        .child(ChildBuilder::new().name("b").eager())
                        .build(),
                ),
                (
                    "b",
                    ComponentDeclBuilder::new()
                        .child(ChildBuilder::new().name("c").eager())
                        .child(ChildBuilder::new().name("d").eager())
                        .build(),
                ),
                ("c", component_decl_with_test_runner()),
                ("d", component_decl_with_test_runner()),
            ];
            let test = ActionsTest::new("root", components, None).await;
            let root = test.model.root();
            let component_a = test.look_up(["a"].try_into().unwrap()).await;
            let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
            let component_c = test.look_up(["a", "b", "c"].try_into().unwrap()).await;
            let component_d = test.look_up(["a", "b", "d"].try_into().unwrap()).await;

            // Component startup was eager, so they should all have an `Execution`.
            root.start_instance(&component_a.moniker, &StartReason::Eager)
                .await
                .expect("could not start a");
            assert!(component_a.is_started().await);
            assert!(component_b.is_started().await);
            assert!(component_c.is_started().await);
            assert!(component_d.is_started().await);

            let component_a_info = ComponentInfo::new(component_a.clone()).await;
            let component_b_info = ComponentInfo::new(component_b).await;
            let component_c_info = ComponentInfo::new(component_c).await;
            let component_d_info = ComponentInfo::new(component_d.clone()).await;

            // Mock a failure to shutdown "d".
            let (shutdown_completer, mock_shutdown_action) = MockAction::new(ActionKey::Shutdown);
            let _shutdown_notifier =
                component_d.actions().register_no_wait(mock_shutdown_action).await;

            // Register shutdown action on "a", and wait for it. "d" fails to shutdown, so "a"
            // fails too. The state of "c" is unknown at this point. The shutdown of stop targets
            // occur simultaneously. "c" could've shutdown before "d" or it might not have.
            let a_shutdown_notifier = component_a
                .actions()
                .register_no_wait(ShutdownAction::new(ShutdownType::Instance))
                .await;

            // We need to wait for the shutdown action of "b" to register a shutdown action on "d",
            // which will be deduplicated with the shutdown action we registered on "d" earlier.
            _ = fasync::TestExecutor::poll_until_stalled(std::future::pending::<()>()).await;

            // Now we can allow the mock shutdown action to complete with an error, and wait for
            // our destroy child call to finish.
            shutdown_completer
                .send(Err(ActionError::from(StopActionError::GetParentFailed)))
                .unwrap();
            a_shutdown_notifier.await.expect_err("shutdown succeeded unexpectedly");

            component_a_info.check_not_shut_down(&test.runner).await;
            component_b_info.check_not_shut_down(&test.runner).await;
            component_d_info.check_not_shut_down(&test.runner).await;

            // Register shutdown action on "a" again. Without our mock action queued up on it,
            // "d"'s shutdown succeeds, and "a" is shutdown this time.
            ActionsManager::register(
                component_a_info.component.clone(),
                ShutdownAction::new(ShutdownType::Instance),
            )
            .await
            .expect("shutdown failed");
            component_a_info.check_is_shut_down(&test.runner).await;
            component_b_info.check_is_shut_down(&test.runner).await;
            component_c_info.check_is_shut_down(&test.runner).await;
            component_d_info.check_is_shut_down(&test.runner).await;
            {
                let mut events: Vec<_> = test
                    .test_hook
                    .lifecycle()
                    .into_iter()
                    .filter(|e| match e {
                        Lifecycle::Stop(_) => true,
                        _ => false,
                    })
                    .collect();
                // The leaves could be stopped in any order.
                let mut first: Vec<_> = events.drain(0..2).collect();
                first.sort_unstable();
                let expected: Vec<_> = vec![
                    Lifecycle::Stop(["a", "b", "c"].try_into().unwrap()),
                    Lifecycle::Stop(["a", "b", "d"].try_into().unwrap()),
                ];
                assert_eq!(first, expected);
                assert_eq!(
                    events,
                    vec![
                        Lifecycle::Stop(["a", "b"].try_into().unwrap()),
                        Lifecycle::Stop(["a"].try_into().unwrap())
                    ]
                );
            }
        });
        executor.run_until_stalled(&mut test_body).unwrap();
    }

    fn make_dep(offer: fdecl::Offer) -> (DependencyNode, DependencyNode) {
        let (a, b) = cm_graph::get_dependency_from_offer(&offer);
        (a.unwrap(), b.unwrap())
    }
}
