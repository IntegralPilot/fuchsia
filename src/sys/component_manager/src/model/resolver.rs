// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ::routing::resolving::{ComponentAddress, ResolvedComponent, ResolverError};
use async_trait::async_trait;

/// Resolves a component URL to its content.
#[async_trait]
pub trait Resolver: std::fmt::Debug {
    /// Resolves a component URL to its content. This function takes in the
    /// `component_address` (from an absolute or relative URL), and the `target`
    /// component that is trying to be resolved.
    async fn resolve(
        &self,
        component_address: &ComponentAddress,
    ) -> Result<ResolvedComponent, ResolverError>;
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::component::instance::InstanceState;
    use crate::model::component::manager::ComponentManagerInstance;
    use crate::model::component::{ComponentInstance, WeakComponentInstance, WeakExtendedInstance};
    use crate::model::context::ModelContext;
    use crate::root_input_builder::RootInputBuilder;
    use anyhow::Error;
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use directed_graph::DirectedGraph;
    use fidl_fuchsia_component_decl as fdecl;
    use fuchsia_sync::Mutex;
    use hooks::Hooks;
    use moniker::Moniker;
    use routing::bedrock::structured_dict::ComponentInput;
    use routing::resolving::ComponentResolutionContext;
    use std::sync::{Arc, Weak};

    #[derive(Debug)]
    struct MockOkResolver {
        pub expected_url: String,
    }

    #[async_trait]
    impl Resolver for MockOkResolver {
        async fn resolve(
            &self,
            component_address: &ComponentAddress,
        ) -> Result<ResolvedComponent, ResolverError> {
            assert_eq!(&self.expected_url, component_address.url());
            Ok(ResolvedComponent {
                // MockOkResolver only resolves one component, so it does not
                // need to provide a context for resolving children.
                context_to_resolve_children: None,
                decl: cm_rust::ComponentDecl::default(),
                package: None,
                config_values: None,
                abi_revision: Some(
                    version_history_data::HISTORY
                        .get_example_supported_version_for_tests()
                        .abi_revision,
                ),
                dependencies: DirectedGraph::new(),
            })
        }
    }

    #[derive(Debug, Clone)]
    struct ResolveState {
        pub expected_url: String,
        pub expected_context: Option<ComponentResolutionContext>,
        pub context_to_resolve_children: Option<ComponentResolutionContext>,
    }

    impl ResolveState {
        fn new(
            url: &str,
            expected_context: Option<ComponentResolutionContext>,
            context_to_resolve_children: Option<ComponentResolutionContext>,
        ) -> Self {
            Self { expected_url: url.to_string(), expected_context, context_to_resolve_children }
        }
    }

    #[derive(Debug)]
    struct MockMultipleOkResolver {
        pub resolve_states: Arc<Mutex<Vec<ResolveState>>>,
    }

    impl MockMultipleOkResolver {
        fn new(resolve_states: Vec<ResolveState>) -> Self {
            Self { resolve_states: Arc::new(Mutex::new(resolve_states)) }
        }
    }

    #[async_trait]
    impl Resolver for MockMultipleOkResolver {
        async fn resolve(
            &self,
            component_address: &ComponentAddress,
        ) -> Result<ResolvedComponent, ResolverError> {
            let ResolveState { expected_url, expected_context, context_to_resolve_children } =
                self.resolve_states.lock().remove(0);
            let (component_url, some_context) = component_address.to_url_and_context();
            assert_eq!(expected_url, component_url);
            assert_eq!(expected_context.as_ref(), some_context, "resolving {}", component_url);
            Ok(ResolvedComponent {
                context_to_resolve_children,

                // We don't actually need to return a valid component here as these unit tests only
                // cover the process of going from relative -> full URL.
                decl: cm_rust::ComponentDecl::default(),
                package: None,
                config_values: None,
                abi_revision: Some(
                    version_history_data::HISTORY
                        .get_example_supported_version_for_tests()
                        .abi_revision,
                ),
                dependencies: DirectedGraph::new(),
            })
        }
    }

    async fn new_root_component(
        resolvers: Vec<(&'static str, Arc<dyn Resolver + Send + Sync + 'static>)>,
        top_instance: &Arc<ComponentManagerInstance>,
        context: Arc<ModelContext>,
        component_manager_instance: Weak<ComponentManagerInstance>,
        component_url: &str,
    ) -> Arc<ComponentInstance> {
        let mut root_input_builder = RootInputBuilder::new(top_instance, context.runtime_config());
        for (resolver_name, resolver) in resolvers.into_iter() {
            root_input_builder.add_resolver(resolver_name.to_string(), resolver);
        }
        ComponentInstance::new_root(
            root_input_builder.build(),
            context,
            component_manager_instance,
            component_url.parse().unwrap(),
        )
        .await
    }

    async fn new_component(
        input: ComponentInput,
        moniker: Moniker,
        component_url: &str,
        startup: fdecl::StartupMode,
        on_terminate: fdecl::OnTerminate,
        config_parent_overrides: Option<Box<[cm_rust::ConfigOverride]>>,
        context: Arc<ModelContext>,
        parent: WeakExtendedInstance,
        hooks: Arc<Hooks>,
        persistent_storage: bool,
    ) -> Arc<ComponentInstance> {
        ComponentInstance::new(
            input,
            moniker,
            0,
            component_url.parse().unwrap(),
            startup,
            on_terminate,
            config_parent_overrides,
            context,
            parent,
            hooks,
            persistent_storage,
        )
        .await
    }

    async fn get_input(component: &Arc<ComponentInstance>) -> ComponentInput {
        match &*component.lock_state().await {
            InstanceState::Unresolved(state) => state.component_input.clone(),
            InstanceState::Resolved(state) | InstanceState::Started(state, _) => {
                state.sandbox.component_input.clone()
            }
            _ => panic!("unexpected state"),
        }
    }

    async fn resolve_component(
        url: &cm_types::Url,
        component: &Arc<ComponentInstance>,
    ) -> Result<ResolvedComponent, ResolverError> {
        let component_address = ComponentAddress::from_url(url, &component)
            .await
            .expect("failed to make component address");
        component.perform_resolve(None, &component_address).await
    }

    #[fuchsia::test]
    async fn relative_to_fuchsia_pkg() -> Result<(), Error> {
        let expected_urls_and_contexts = vec![
            ResolveState::new(
                "fuchsia-pkg://fuchsia.com/my-package#meta/my-root.cm",
                None,
                Some(ComponentResolutionContext::new("package_context".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "fuchsia-pkg://fuchsia.com/my-package#meta/my-child.cm",
                None,
                Some(ComponentResolutionContext::new("package_context".as_bytes().to_vec())),
            ),
        ];

        let resolvers: Vec<(&'static str, Arc<dyn Resolver + Send + Sync + 'static>)> = vec![(
            "fuchsia-pkg",
            Arc::new(MockMultipleOkResolver::new(expected_urls_and_contexts.clone())),
        )];
        let top_instance = Arc::new(ComponentManagerInstance::new(vec![], vec![]));
        let root = new_root_component(
            resolvers,
            &top_instance,
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "fuchsia-pkg://fuchsia.com/my-package#meta/my-root.cm",
        )
        .await;

        let child = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/child")?,
            "#meta/my-child.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&root)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let resolved = resolve_component(&child.component_url, &child).await?;
        let expected = expected_urls_and_contexts.as_slice().last().unwrap();
        assert_eq!(&resolved.context_to_resolve_children, &expected.context_to_resolve_children);

        Ok(())
    }

    #[fuchsia::test]
    async fn two_relative_to_fuchsia_pkg() -> Result<(), Error> {
        let expected_urls_and_contexts = vec![
            ResolveState::new(
                "fuchsia-pkg://fuchsia.com/my-package#meta/my-root.cm",
                None,
                Some(ComponentResolutionContext::new("package_context".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "fuchsia-pkg://fuchsia.com/my-package#meta/my-child.cm",
                None,
                Some(ComponentResolutionContext::new("package_context".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "fuchsia-pkg://fuchsia.com/my-package#meta/my-child2.cm",
                None,
                Some(ComponentResolutionContext::new("package_context".as_bytes().to_vec())),
            ),
        ];

        let resolvers: Vec<(&'static str, Arc<dyn Resolver + Send + Sync + 'static>)> = vec![(
            "fuchsia-pkg",
            Arc::new(MockMultipleOkResolver::new(expected_urls_and_contexts.clone())),
        )];

        let top_instance = Arc::new(ComponentManagerInstance::new(vec![], vec![]));
        let root = new_root_component(
            resolvers,
            &top_instance,
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "fuchsia-pkg://fuchsia.com/my-package#meta/my-root.cm",
        )
        .await;

        let child_one = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/child")?,
            "#meta/my-child.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&root)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let child_two = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/child")?,
            "#meta/my-child2.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&child_one)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let resolved = resolve_component(&child_two.component_url, &child_two).await?;
        let expected = expected_urls_and_contexts.as_slice().last().unwrap();
        assert_eq!(&resolved.context_to_resolve_children, &expected.context_to_resolve_children);
        Ok(())
    }

    #[fuchsia::test]
    async fn relative_to_fuchsia_boot() -> Result<(), Error> {
        let expected_urls_and_contexts = vec![
            ResolveState::new(
                "fuchsia-boot:///#meta/my-root.cm",
                None,
                Some(ComponentResolutionContext::new("package_context".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "fuchsia-boot:///#meta/my-child.cm",
                None,
                Some(ComponentResolutionContext::new("package_context".as_bytes().to_vec())),
            ),
        ];

        let resolvers: Vec<(&'static str, Arc<dyn Resolver + Send + Sync + 'static>)> = vec![(
            "fuchsia-boot",
            Arc::new(MockMultipleOkResolver::new(expected_urls_and_contexts.clone())),
        )];

        let top_instance = Arc::new(ComponentManagerInstance::new(vec![], vec![]));
        let root = new_root_component(
            resolvers,
            &top_instance,
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "fuchsia-boot:///#meta/my-root.cm",
        )
        .await;

        let child = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/child")?,
            "#meta/my-child.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&root)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let resolved = resolve_component(&child.component_url, &child).await?;
        let expected = expected_urls_and_contexts.as_slice().last().unwrap();
        assert_eq!(&resolved.context_to_resolve_children, &expected.context_to_resolve_children);
        Ok(())
    }

    #[fuchsia::test]
    async fn relative_to_cast() -> Result<(), Error> {
        let expected_urls_and_contexts = vec![
            ResolveState::new(
                "cast:00000000#meta/my-root.cm",
                None,
                Some(ComponentResolutionContext::new("package_context".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "cast:00000000#meta/my-child.cm",
                None,
                Some(ComponentResolutionContext::new("package_context".as_bytes().to_vec())),
            ),
        ];
        let resolvers: Vec<(&'static str, Arc<dyn Resolver + Send + Sync + 'static>)> = vec![(
            "cast",
            Arc::new(MockMultipleOkResolver::new(expected_urls_and_contexts.clone())),
        )];

        let top_instance = Arc::new(ComponentManagerInstance::new(vec![], vec![]));
        let root = new_root_component(
            resolvers,
            &top_instance,
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "cast:00000000#meta/my-root.cm",
        )
        .await;

        let child = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/child")?,
            "#meta/my-child.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&root)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let resolved = resolve_component(&child.component_url, &child).await?;
        let expected = expected_urls_and_contexts.as_slice().last().unwrap();
        assert_eq!(&resolved.context_to_resolve_children, &expected.context_to_resolve_children);
        Ok(())
    }

    #[fuchsia::test]
    async fn resolve_above_root_error() -> Result<(), Error> {
        let top_instance = Arc::new(ComponentManagerInstance::new(vec![], vec![]));
        let root = new_root_component(
            vec![],
            &top_instance,
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "#meta/my-root.cm",
        )
        .await;

        let child = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/child")?,
            "#meta/my-child.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&root)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let result = ComponentAddress::from_url(&child.component_url, &child).await;
        assert_matches!(result, Err(ResolverError::Internal(..)));
        Ok(())
    }

    #[fuchsia::test]
    async fn relative_resource_and_path_to_fuchsia_pkg() -> Result<(), Error> {
        let expected_urls_and_contexts = vec![
            ResolveState::new(
                "fuchsia-pkg://fuchsia.com/my-package#meta/my-root.cm",
                None,
                Some(ComponentResolutionContext::new("fuchsia.com...".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "my-subpackage#meta/my-child.cm",
                Some(ComponentResolutionContext::new("fuchsia.com...".as_bytes().to_vec())),
                Some(ComponentResolutionContext::new("my-subpackage...".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "my-subpackage#meta/my-child2.cm",
                Some(ComponentResolutionContext::new("fuchsia.com...".as_bytes().to_vec())),
                Some(ComponentResolutionContext::new("my-subpackage...".as_bytes().to_vec())),
            ),
        ];

        let resolvers: Vec<(&'static str, Arc<dyn Resolver + Send + Sync + 'static>)> = vec![(
            "fuchsia-pkg",
            Arc::new(MockMultipleOkResolver::new(expected_urls_and_contexts.clone())),
        )];

        let top_instance = Arc::new(ComponentManagerInstance::new(vec![], vec![]));
        let root = new_root_component(
            resolvers,
            &top_instance,
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "fuchsia-pkg://fuchsia.com/my-package#meta/my-root.cm",
        )
        .await;

        let child_one = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/child")?,
            "my-subpackage#meta/my-child.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&root)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let child_two = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/child/child2")?,
            "#meta/my-child2.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&child_one)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let resolved = resolve_component(&child_two.component_url, &child_two).await?;
        let expected = expected_urls_and_contexts.as_slice().last().unwrap();
        assert_eq!(&resolved.context_to_resolve_children, &expected.context_to_resolve_children);
        Ok(())
    }

    #[fuchsia::test]
    async fn two_relative_resources_and_path_to_fuchsia_pkg() -> Result<(), Error> {
        let expected_urls_and_contexts = vec![
            ResolveState::new(
                "fuchsia-pkg://fuchsia.com/my-package#meta/my-root.cm",
                None,
                Some(ComponentResolutionContext::new("fuchsia.com...".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "my-subpackage#meta/my-child.cm",
                Some(ComponentResolutionContext::new("fuchsia.com...".as_bytes().to_vec())),
                Some(ComponentResolutionContext::new("my-subpackage...".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "my-subpackage#meta/my-child2.cm",
                Some(ComponentResolutionContext::new("fuchsia.com...".as_bytes().to_vec())),
                Some(ComponentResolutionContext::new("my-subpackage...".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "my-subpackage#meta/my-child3.cm",
                Some(ComponentResolutionContext::new("fuchsia.com...".as_bytes().to_vec())),
                Some(ComponentResolutionContext::new("my-subpackage...".as_bytes().to_vec())),
            ),
        ];

        let resolvers: Vec<(&'static str, Arc<dyn Resolver + Send + Sync + 'static>)> = vec![(
            "fuchsia-pkg",
            Arc::new(MockMultipleOkResolver::new(expected_urls_and_contexts.clone())),
        )];

        let top_instance = Arc::new(ComponentManagerInstance::new(vec![], vec![]));
        let root = new_root_component(
            resolvers,
            &top_instance,
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "fuchsia-pkg://fuchsia.com/my-package#meta/my-root.cm",
        )
        .await;

        let child_one = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/child")?,
            "my-subpackage#meta/my-child.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&root)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let child_two = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/child/child2")?,
            "#meta/my-child2.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&child_one)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let child_three = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/child/child2/child3")?,
            "#meta/my-child3.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&child_two)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let resolved = resolve_component(&child_three.component_url, &child_three).await?;
        let expected = expected_urls_and_contexts.as_slice().last().unwrap();
        assert_eq!(&resolved.context_to_resolve_children, &expected.context_to_resolve_children);
        Ok(())
    }

    #[fuchsia::test]
    async fn relative_resources_and_paths_to_realm_builder() -> Result<(), Error> {
        let expected_urls_and_contexts = vec![
            ResolveState::new(
                "fuchsia-pkg://fuchsia.com/my-package#meta/my-root.cm",
                None,
                Some(ComponentResolutionContext::new("fuchsia.com...".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "my-subpackage1#meta/sub1.cm",
                Some(ComponentResolutionContext::new("fuchsia.com...".as_bytes().to_vec())),
                Some(ComponentResolutionContext::new("my-subpackage1...".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "my-subpackage1#meta/sub1-child.cm",
                Some(ComponentResolutionContext::new("fuchsia.com...".as_bytes().to_vec())),
                Some(ComponentResolutionContext::new("my-subpackage1...".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "my-subpackage2#meta/sub2.cm",
                Some(ComponentResolutionContext::new("my-subpackage1...".as_bytes().to_vec())),
                Some(ComponentResolutionContext::new("my-subpackage2...".as_bytes().to_vec())),
            ),
            ResolveState::new(
                "my-subpackage2#meta/sub2-child.cm",
                Some(ComponentResolutionContext::new("my-subpackage1...".as_bytes().to_vec())),
                Some(ComponentResolutionContext::new("my-subpackage2...".as_bytes().to_vec())),
            ),
        ];

        let resolvers: Vec<(&'static str, Arc<dyn Resolver + Send + Sync + 'static>)> = vec![
            (
                "fuchsia-pkg",
                Arc::new(MockMultipleOkResolver::new(expected_urls_and_contexts.clone())),
            ),
            (
                "realm-builder",
                Arc::new(MockOkResolver { expected_url: "realm-builder://0/my-realm".to_string() }),
            ),
        ];

        let top_instance = Arc::new(ComponentManagerInstance::new(vec![], vec![]));
        let root = new_root_component(
            resolvers,
            &top_instance,
            Arc::new(ModelContext::new_for_test()),
            Weak::new(),
            "fuchsia-pkg://fuchsia.com/my-package#meta/my-root.cm",
        )
        .await;

        let realm = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/realm/child")?,
            "realm-builder://0/my-realm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&root)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let child_one = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/realm/child")?,
            "my-subpackage1#meta/sub1.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&realm)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let child_two = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/realm/child/child2")?,
            "#meta/sub1-child.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&child_one)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let child_three = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/realm/child/child2/child3")?,
            "my-subpackage2#meta/sub2.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&child_two)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let child_four = new_component(
            get_input(&root).await,
            Moniker::parse_str("root/realm/child/child2/child3/child4")?,
            "#meta/sub2-child.cm",
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::from(&child_three)),
            Arc::new(Hooks::new()),
            false,
        )
        .await;

        let resolved = resolve_component(&child_four.component_url, &child_four).await?;
        let expected = expected_urls_and_contexts.as_slice().last().unwrap();
        assert_eq!(&resolved.context_to_resolve_children, &expected.context_to_resolve_children);
        Ok(())
    }
}
