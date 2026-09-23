// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::ComponentInstance;
use crate::model::component::instance::ResolvedInstanceState;
use cm_types::Name;
use diagnostics_log::{BufferedPublisher, PublisherOptions};
use fidl::endpoints;
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_logger as flogger;
use fuchsia_sync::Mutex;
use log::Log;
use moniker::Moniker;
use routing::DictExt;
use runtime_capabilities::{Capability, WeakInstanceToken};
use std::collections::LinkedList;
use std::sync::{Arc, LazyLock};

const CACHE_SIZE: usize = 10;
static LOGGER_CACHE: LazyLock<Mutex<LoggerCache>> =
    LazyLock::new(|| Mutex::new(LoggerCache { list: LinkedList::new() }));

pub struct LoggerCache {
    list: LinkedList<(Moniker, Arc<BufferedPublisher>)>,
}

impl LoggerCache {
    /// Tries to logs on behalf of the component and falls back to the global log on failure.
    pub fn log(
        moniker: &Moniker,
        resolved_instance_state: &ResolvedInstanceState,
        record: &log::Record,
        target: Arc<WeakInstanceToken>,
    ) {
        if !Self::try_attributed_log(moniker, resolved_instance_state, record, target) {
            log::logger().log(record);
        }
    }

    /// Purges the cache for the component.
    pub fn purge(component: &ComponentInstance) {
        LOGGER_CACHE.lock().list.extract_if(|e| &e.0 == &component.moniker).next();
    }

    /// Tries to logs on behalf of the component.
    pub fn try_attributed_log(
        moniker: &Moniker,
        resolved_instance_state: &ResolvedInstanceState,
        record: &log::Record,
        target: Arc<WeakInstanceToken>,
    ) -> bool {
        let publisher_opt = {
            let mut cache = LOGGER_CACHE.lock();
            if let Some(element) = cache.list.extract_if(|e| &e.0 == moniker).next() {
                let publisher = element.1.clone();
                cache.list.push_front(element);
                Some(publisher)
            } else {
                None
            }
        };

        if let Some(publisher) = publisher_opt {
            publisher.log(record);
            return true;
        }

        // Check that the component includes the logsink capability before logging on its behalf.
        let Some(decl) = &resolved_instance_state.logger_decl else {
            return false;
        };

        let program_input_dict = &resolved_instance_state.sandbox.program_input;
        let router = match decl {
            cm_rust::UseProtocolDecl {
                target_path: Some(target_path),
                numbered_handle: None,
                ..
            } => {
                let Some(Capability::ConnectorRouter(router)) =
                    program_input_dict.namespace().get_capability(target_path)
                else {
                    return false;
                };
                router
            }
            cm_rust::UseProtocolDecl {
                numbered_handle: Some(numbered_handle),
                target_path: None,
                ..
            } => {
                let numbered_handle = Name::from(*numbered_handle);
                let Some(Capability::ConnectorRouter(router)) =
                    program_input_dict.numbered_handles().get_capability(&numbered_handle)
                else {
                    return false;
                };
                router
            }
            _ => {
                panic!(
                    "UseProtocolDecl had neither or both of numbered_handle and target_path. \
                    Validation prevents this."
                );
            }
        };

        let (logsink, server) = endpoints::create_endpoints::<flogger::LogSinkMarker>();
        let scope = &resolved_instance_state.execution_scope;
        scope.spawn(async move {
            // the router itself should handle logging things in event of an error
            if let Ok(Some(c)) = router.route(RouteRequest::default(), target).await {
                let _ = c.send(server.into());
            }
        });

        let Ok(publisher) =
            BufferedPublisher::new(PublisherOptions::default().use_log_sink(logsink))
        else {
            return false;
        };

        publisher.log(record);

        {
            let mut cache = LOGGER_CACHE.lock();
            // Just in case another thread simultaneously handled a cache miss for the same moniker
            cache.list.extract_if(|e| &e.0 == moniker).for_each(drop);

            cache.list.push_front((moniker.clone(), publisher));
            if cache.list.len() > CACHE_SIZE {
                cache.list.pop_back();
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::{CACHE_SIZE, LOGGER_CACHE, LoggerCache};
    use crate::model::component::instance::{ComponentDomain, ResolvedInstanceState};
    use crate::model::component::{Component, ComponentInstance, WeakExtendedInstance};
    use crate::model::context::ModelContext;
    use cm_rust::{UseDecl, UseProtocolDecl, UseSource};
    use cm_rust_testing::ComponentDeclBuilder;
    use cm_types::{BoundedName, Path, Url};
    use directed_graph::DirectedGraph;
    use fidl::endpoints::DiscoverableProtocolMarker;
    use fidl_fuchsia_component_decl::{self as fdecl, OnTerminate};
    use fidl_fuchsia_logger as flogger;
    use hooks::Hooks;
    use routing::bedrock::structured_dict::ComponentInput;
    use routing::component_instance::ComponentInstanceInterface;
    use runtime_capabilities::{Capability, Connector, Receiver, Router};
    use std::str::FromStr;
    use std::sync::{Arc, Weak};

    const LOG_SINK_PROTOCOL: &str = flogger::LogSinkMarker::PROTOCOL_NAME;

    async fn new_instance_with_name(
        name: &str,
        decl: cm_rust::ComponentDecl,
        parent_capabilities: ComponentInput,
    ) -> (Arc<ComponentInstance>, ResolvedInstanceState) {
        let url: Url = "test:///foo".parse().unwrap();
        let instance = ComponentInstance::new(
            ComponentInput::default(),
            name.try_into().unwrap(),
            1,
            url.clone(),
            fdecl::StartupMode::Lazy,
            OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::AboveRoot(Weak::new()),
            Arc::new(Hooks::new()),
            false,
        )
        .await;
        let resolved_component = Component {
            context_to_resolve_children: None,
            decl: Some(Arc::new(decl)),
            package: None,
            config: None,
            abi_revision: None,
            dependencies: Arc::new(DirectedGraph::new()),
        };
        let resolved_state = ResolvedInstanceState::new(
            &instance,
            resolved_component,
            ComponentDomain::Absolute,
            Default::default(),
            parent_capabilities,
        )
        .await
        .unwrap();
        (instance, resolved_state)
    }

    async fn new_instance_with_valid_logsink(
        name: &str,
    ) -> (Arc<ComponentInstance>, ResolvedInstanceState, Receiver) {
        let target_path: Path = format!("/svc/{LOG_SINK_PROTOCOL}").parse().unwrap();
        let decl = ComponentDeclBuilder::new_empty_component()
            .use_(UseDecl::Protocol(UseProtocolDecl {
                source: UseSource::Parent,
                source_name: LOG_SINK_PROTOCOL.parse().unwrap(),
                source_dictionary: Default::default(),
                target_path: Some(target_path.clone()),
                numbered_handle: None,
                dependency_type: cm_rust::DependencyType::Strong,
                availability: cm_rust::Availability::Required,
            }))
            .build();
        let input = ComponentInput::default();
        let (rx, connector) = Connector::new();
        let prev = input.insert_capability(
            &BoundedName::from_str(LOG_SINK_PROTOCOL).unwrap(),
            Capability::ConnectorRouter(Router::new_ok(connector)),
        );
        assert!(prev.is_none());
        let (instance, resolved_state) = new_instance_with_name(name, decl, input).await;
        (instance, resolved_state, rx)
    }

    async fn new_instance_without_logsink_decl() -> (Arc<ComponentInstance>, ResolvedInstanceState)
    {
        let decl = ComponentDeclBuilder::new_empty_component().build();
        new_instance_with_name("child", decl, ComponentInput::default()).await
    }

    fn new_record() -> log::Record<'static> {
        log::Record::builder().args(format_args!("foo")).build()
    }

    #[fuchsia::test]
    async fn try_attributed_log_with_logsink() {
        let (instance, resolved_instance, rx) = new_instance_with_valid_logsink("foo").await;
        assert!(LoggerCache::try_attributed_log(
            &instance.moniker,
            &resolved_instance,
            &new_record(),
            instance.as_weak().into(),
        ));
        assert!(rx.receive().await.is_some());
    }

    #[fuchsia::test]
    async fn try_attributed_log_without_logsink_decl() {
        let (instance, resolved_instance) = new_instance_without_logsink_decl().await;
        assert!(!LoggerCache::try_attributed_log(
            &instance.moniker,
            &resolved_instance,
            &new_record(),
            instance.as_weak().into(),
        ));
    }

    #[fuchsia::test]
    async fn log_twice_should_hit_cache() {
        let (instance, resolved_instance, rx) = new_instance_with_valid_logsink("foo").await;
        let record = new_record();

        // First log, populates cache.
        assert!(LoggerCache::try_attributed_log(
            &instance.moniker,
            &resolved_instance,
            &record,
            instance.as_weak().into()
        ));
        assert!(rx.receive().await.is_some());
        assert_eq!(LOGGER_CACHE.lock().list.len(), 1);

        // Second log, should hit cache. Use an instance that would fail.
        let (_, resolved_instance_fail) = new_instance_without_logsink_decl().await;
        assert!(LoggerCache::try_attributed_log(
            &instance.moniker,
            &resolved_instance_fail,
            &record,
            instance.as_weak().into(),
        ));
        assert_eq!(LOGGER_CACHE.lock().list.len(), 1);
    }

    #[fuchsia::test]
    async fn cache_eviction() {
        let record = new_record();

        let mut instances = Vec::new();
        for i in 0..CACHE_SIZE {
            let (instance, resolved_instance, rx) =
                new_instance_with_valid_logsink(&format!("child-{}", i)).await;
            assert!(LoggerCache::try_attributed_log(
                &instance.moniker,
                &resolved_instance,
                &record,
                instance.as_weak().into(),
            ));
            assert!(rx.receive().await.is_some());
            instances.push(instance);
        }
        assert_eq!(LOGGER_CACHE.lock().list.len(), CACHE_SIZE);

        // Log one more time, should evict the first one.
        let (instance, resolved_instance, rx) = new_instance_with_valid_logsink("new-child").await;
        assert!(LoggerCache::try_attributed_log(
            &instance.moniker,
            &resolved_instance,
            &record,
            instance.as_weak().into()
        ));
        assert!(rx.receive().await.is_some());

        let cache = LOGGER_CACHE.lock();
        assert_eq!(cache.list.len(), CACHE_SIZE);
        assert!(!cache.list.iter().any(|(m, _)| m == &instances[0].moniker));
        assert!(cache.list.iter().any(|(m, _)| m == &instance.moniker));
    }

    #[fuchsia::test]
    async fn purge_removes_from_cache() {
        let (instance, resolved_instance, rx) = new_instance_with_valid_logsink("foo").await;

        // Log to populate the cache.
        assert!(LoggerCache::try_attributed_log(
            &instance.moniker,
            &resolved_instance,
            &new_record(),
            instance.as_weak().into(),
        ));
        assert!(rx.receive().await.is_some());
        assert_eq!(LOGGER_CACHE.lock().list.len(), 1);

        // Purge and check that the cache is empty.
        LoggerCache::purge(&instance);
        assert_eq!(LOGGER_CACHE.lock().list.len(), 0);
    }
}
