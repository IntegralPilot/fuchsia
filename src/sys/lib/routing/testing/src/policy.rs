// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use assert_matches::assert_matches;
use async_trait::async_trait;
use capability_source::{
    BuiltinSource, CapabilitySource, CapabilityToCapabilitySource, ComponentCapability,
    ComponentSource, FrameworkSource, InternalCapability, NamespaceSource,
};
use cm_config::{
    AllowlistEntry, AllowlistEntryBuilder, CapabilityAllowlistKey, CapabilityAllowlistSource,
    ChildPolicyAllowlists, DebugCapabilityAllowlistEntry, DebugCapabilityKey, JobPolicyAllowlists,
    SecurityPolicy,
};
use cm_rust::{CapabilityTypeName, ProtocolDecl, StorageDecl, StorageDirectorySource};
use cm_types::Name;
use fidl_fuchsia_component_decl as fdecl;
use moniker::{ExtendedMoniker, Moniker};
use routing::component_instance::ComponentInstanceInterface;
use routing::policy::GlobalPolicyChecker;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// These GlobalPolicyChecker tests are run under multiple contexts, e.g. both on Fuchsia under
/// component_manager and on the build host under cm_fidl_analyzer. This macro helps ensure that all
/// tests are run in each context.
#[macro_export]
macro_rules! instantiate_global_policy_checker_tests {
    ($fixture_impl:path) => {
        // New GlobalPolicyCheckerTest tests must be added to this list to run.
        instantiate_global_policy_checker_tests! {
            $fixture_impl,
            global_policy_checker_can_route_capability_framework_cap,
            global_policy_checker_can_route_capability_namespace_cap,
            global_policy_checker_can_route_capability_component_cap,
            global_policy_checker_can_route_capability_capability_cap,
            global_policy_checker_can_route_debug_capability_capability_cap,
            global_policy_checker_can_route_debug_capability_with_realm_allowlist_entry,
            global_policy_checker_can_route_debug_capability_with_collection_allowlist_entry,
            global_policy_checker_can_route_capability_builtin_cap,
            global_policy_checker_can_route_capability_with_realm_allowlist_entry,
            global_policy_checker_can_route_capability_with_collection_allowlist_entry,
        }
    };
    ($fixture_impl:path, $test:ident, $($remaining:ident),+ $(,)?) => {
        instantiate_global_policy_checker_tests! { $fixture_impl, $test }
        instantiate_global_policy_checker_tests! { $fixture_impl, $($remaining),+ }
    };
    ($fixture_impl:path, $test:ident) => {
        fn $test() -> Result<(), Error> {
            let mut executor = fuchsia_async::LocalExecutor::default();
            executor.run_singlethreaded(<$fixture_impl as Default>::default().$test())
        }
    };
}

// Tests `GlobalPolicyChecker` for implementations of `ComponentInstanceInterface`.
#[async_trait]
pub trait GlobalPolicyCheckerTest<C>
where
    C: ComponentInstanceInterface + 'static,
{
    // Creates a `ComponentInstanceInterface` with the given `Moniker`.
    async fn make_component(&self, moniker: Moniker) -> Arc<C>;

    // Tests `GlobalPolicyChecker::can_route_capability()` for framework capability sources.
    async fn global_policy_checker_can_route_capability_framework_cap(&self) -> Result<(), Error> {
        let mut policy_builder = CapabilityAllowlistPolicyBuilder::new();
        policy_builder.add_capability_policy(
            CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentInstance(
                    Moniker::try_from(["foo", "bar"]).unwrap(),
                ),
                source_name: "fuchsia.component.Realm".parse().unwrap(),
                source: CapabilityAllowlistSource::Framework,
                capability: CapabilityTypeName::Protocol,
            },
            vec![
                AllowlistEntryBuilder::new().exact("foo").exact("bar").build(),
                AllowlistEntryBuilder::new().exact("foo").exact("bar").exact("baz").build(),
            ],
        );
        let global_policy_checker = GlobalPolicyChecker::new(Arc::new(policy_builder.build()));
        let component = self.make_component(["foo:0", "bar:0"].try_into().unwrap()).await;

        let protocol_capability = CapabilitySource::Framework(FrameworkSource {
            capability: InternalCapability::Protocol("fuchsia.component.Realm".parse().unwrap()),
            moniker: component.moniker().clone(),
        });
        let valid_path_0 = Moniker::try_from(["foo", "bar"]).unwrap();
        let valid_path_1 = Moniker::try_from(["foo", "bar", "baz"]).unwrap();
        let invalid_path_0 = Moniker::try_from(["foobar"]).unwrap();
        let invalid_path_1 = Moniker::try_from(["foo", "bar", "foobar"]).unwrap();

        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &valid_path_0),
            Ok(())
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &valid_path_1),
            Ok(())
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &invalid_path_0),
            Err(_)
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &invalid_path_1),
            Err(_)
        );
        Ok(())
    }

    // Tests `GlobalPolicyChecker::can_route_capability()` for namespace capability sources.
    async fn global_policy_checker_can_route_capability_namespace_cap(&self) -> Result<(), Error> {
        let mut policy_builder = CapabilityAllowlistPolicyBuilder::new();
        policy_builder.add_capability_policy(
            CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentManager,
                source_name: "fuchsia.kernel.MmioResource".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
            },
            vec![
                AllowlistEntryBuilder::new().exact("root").build(),
                AllowlistEntryBuilder::new().exact("root").exact("bootstrap").build(),
                AllowlistEntryBuilder::new().exact("root").exact("core").build(),
            ],
        );
        let global_policy_checker = GlobalPolicyChecker::new(Arc::new(policy_builder.build()));

        let protocol_capability = CapabilitySource::Namespace(NamespaceSource {
            capability: ComponentCapability::Protocol(ProtocolDecl {
                name: "fuchsia.kernel.MmioResource".parse().unwrap(),
                source_path: Some("/svc/fuchsia.kernel.MmioResource".parse().unwrap()),
                delivery: Default::default(),
            }),
        });
        let valid_path_0 = Moniker::try_from(["root"]).unwrap();
        let valid_path_2 = Moniker::try_from(["root", "core"]).unwrap();
        let valid_path_1 = Moniker::try_from(["root", "bootstrap"]).unwrap();
        let invalid_path_0 = Moniker::try_from(["foobar"]).unwrap();
        let invalid_path_1 = Moniker::try_from(["foo", "bar", "foobar"]).unwrap();

        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &valid_path_0),
            Ok(())
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &valid_path_1),
            Ok(())
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &valid_path_2),
            Ok(())
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &invalid_path_0),
            Err(_)
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &invalid_path_1),
            Err(_)
        );
        Ok(())
    }

    // Tests `GlobalPolicyChecker::can_route_capability()` for component capability sources.
    async fn global_policy_checker_can_route_capability_component_cap(&self) -> Result<(), Error> {
        let mut policy_builder = CapabilityAllowlistPolicyBuilder::new();
        policy_builder.add_capability_policy(
            CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentInstance(
                    Moniker::try_from(["foo"]).unwrap(),
                ),
                source_name: "fuchsia.foo.FooBar".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
            },
            vec![
                AllowlistEntryBuilder::new().exact("foo").build(),
                AllowlistEntryBuilder::new().exact("root").exact("bootstrap").build(),
                AllowlistEntryBuilder::new().exact("root").exact("core").build(),
            ],
        );
        let global_policy_checker = GlobalPolicyChecker::new(Arc::new(policy_builder.build()));
        let component = self.make_component(["foo:0"].try_into().unwrap()).await;

        let protocol_capability = CapabilitySource::Component(ComponentSource {
            capability: ComponentCapability::Protocol(ProtocolDecl {
                name: "fuchsia.foo.FooBar".parse().unwrap(),
                source_path: Some("/svc/fuchsia.foo.FooBar".parse().unwrap()),
                delivery: Default::default(),
            }),
            moniker: component.moniker().clone(),
        });
        let valid_path_0 = Moniker::try_from(["root", "bootstrap"]).unwrap();
        let valid_path_1 = Moniker::try_from(["root", "core"]).unwrap();
        let invalid_path_0 = Moniker::try_from(["foobar"]).unwrap();
        let invalid_path_1 = Moniker::try_from(["foo", "bar", "foobar"]).unwrap();

        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &valid_path_0),
            Ok(())
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &valid_path_1),
            Ok(())
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &invalid_path_0),
            Err(_)
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &invalid_path_1),
            Err(_)
        );
        Ok(())
    }

    // Tests `GlobalPolicyChecker::can_route_capability()` for capability sources of type `Capability`.
    async fn global_policy_checker_can_route_capability_capability_cap(&self) -> Result<(), Error> {
        let mut policy_builder = CapabilityAllowlistPolicyBuilder::new();
        policy_builder.add_capability_policy(
            CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentInstance(
                    Moniker::try_from(["foo"]).unwrap(),
                ),
                source_name: "cache".parse().unwrap(),
                source: CapabilityAllowlistSource::Capability,
                capability: CapabilityTypeName::Storage,
            },
            vec![
                AllowlistEntryBuilder::new().exact("foo").build(),
                AllowlistEntryBuilder::new().exact("root").exact("bootstrap").build(),
                AllowlistEntryBuilder::new().exact("root").exact("core").build(),
            ],
        );
        let global_policy_checker = GlobalPolicyChecker::new(Arc::new(policy_builder.build()));
        let component = self.make_component(["foo:0"].try_into().unwrap()).await;

        let protocol_capability = CapabilitySource::Capability(CapabilityToCapabilitySource {
            source_capability: ComponentCapability::Storage(StorageDecl {
                backing_dir: "cache".parse().unwrap(),
                name: "cache".parse().unwrap(),
                source: StorageDirectorySource::Parent,
                subdir: Default::default(),
                storage_id: fdecl::StorageId::StaticInstanceIdOrMoniker,
            }),
            moniker: component.moniker().clone(),
        });
        let valid_path_0 = Moniker::try_from(["root", "bootstrap"]).unwrap();
        let valid_path_1 = Moniker::try_from(["root", "core"]).unwrap();
        let invalid_path_0 = Moniker::try_from(["foobar"]).unwrap();
        let invalid_path_1 = Moniker::try_from(["foo", "bar", "foobar"]).unwrap();

        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &valid_path_0),
            Ok(())
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &valid_path_1),
            Ok(())
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &invalid_path_0),
            Err(_)
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&protocol_capability, &invalid_path_1),
            Err(_)
        );
        Ok(())
    }

    // Tests `GlobalPolicyChecker::can_route_debug_capability()` for capability sources of type `Capability`.
    async fn global_policy_checker_can_route_debug_capability_capability_cap(
        &self,
    ) -> Result<(), Error> {
        let mut policy_builder = CapabilityAllowlistPolicyBuilder::new();
        policy_builder.add_debug_capability_policy(
            DebugCapabilityKey {
                name: "debug_service1".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
                env_name: "foo_env".parse().unwrap(),
            },
            AllowlistEntryBuilder::new().exact("foo").build(),
        );
        policy_builder.add_debug_capability_policy(
            DebugCapabilityKey {
                name: "debug_service1".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
                env_name: "bootstrap_env".parse().unwrap(),
            },
            AllowlistEntryBuilder::new().exact("root").exact("bootstrap").build(),
        );
        let global_policy_checker = GlobalPolicyChecker::new(Arc::new(policy_builder.build()));
        let protocol_name: Name = "debug_service1".parse().unwrap();

        let valid_cases = vec![
            (Moniker::try_from(["root", "bootstrap"]).unwrap(), "bootstrap_env"),
            (Moniker::try_from(["foo"]).unwrap(), "foo_env"),
        ];

        let invalid_cases = vec![
            (Moniker::try_from(["foobar"]).unwrap(), "foobar_env"),
            (Moniker::try_from(["foo", "bar", "foobar"]).unwrap(), "foobar_env"),
            (Moniker::try_from(["root", "bootstrap"]).unwrap(), "foo_env"),
            (Moniker::try_from(["root", "baz"]).unwrap(), "foo_env"),
        ];

        for valid_case in valid_cases {
            assert_matches!(
                global_policy_checker.can_register_debug_capability(
                    CapabilityTypeName::Protocol,
                    &protocol_name,
                    &valid_case.0,
                    &valid_case.1.parse().unwrap(),
                ),
                Ok(()),
                "{:?}",
                valid_case
            );
        }

        for invalid_case in invalid_cases {
            assert_matches!(
                global_policy_checker.can_register_debug_capability(
                    CapabilityTypeName::Protocol,
                    &protocol_name,
                    &invalid_case.0,
                    &invalid_case.1.parse().unwrap(),
                ),
                Err(_),
                "{:?}",
                invalid_case
            );
        }

        Ok(())
    }

    // Tests `GlobalPolicyChecker::can_route_debug_capability()` for capability sources of type
    // `Capability` with realm allowlist entries.
    async fn global_policy_checker_can_route_debug_capability_with_realm_allowlist_entry(
        &self,
    ) -> Result<(), Error> {
        let mut policy_builder = CapabilityAllowlistPolicyBuilder::new();
        policy_builder.add_debug_capability_policy(
            DebugCapabilityKey {
                name: "debug_service1".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
                env_name: "bar_env".parse().unwrap(),
            },
            AllowlistEntryBuilder::new().exact("root").exact("bootstrap1").any_descendant(),
        );
        policy_builder.add_debug_capability_policy(
            DebugCapabilityKey {
                name: "debug_service1".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
                env_name: "foo_env".parse().unwrap(),
            },
            AllowlistEntryBuilder::new().exact("root").exact("bootstrap2").build(),
        );
        policy_builder.add_debug_capability_policy(
            DebugCapabilityKey {
                name: "debug_service1".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
                env_name: "baz_env".parse().unwrap(),
            },
            AllowlistEntryBuilder::new().exact("root").exact("bootstrap3").any_descendant(),
        );
        let global_policy_checker = GlobalPolicyChecker::new(Arc::new(policy_builder.build()));

        // dest, env
        const VALID_CASES: &[(&[&str], &str)] = &[
            (&["root", "bootstrap1", "child"], "bar_env"),
            (&["root", "bootstrap1", "child", "grandchild"], "bar_env"),
            (&["root", "bootstrap2"], "foo_env"),
            (&["root", "bootstrap3", "child"], "baz_env"),
            (&["root", "bootstrap3", "child", "grandchild"], "baz_env"),
        ];

        const INVALID_CASES: &[(&[&str], &str)] = &[
            (&["root", "not_bootstrap"], "bar_env"),
            (&["root", "not_bootstrap"], "foo_env"),
            (&["root", "bootstrap1"], "baz_env"),
        ];

        for (dest, env) in VALID_CASES {
            let protocol_name: Name = "debug_service1".parse().unwrap();
            let env: Name = env.parse().unwrap();
            assert_matches!(
                global_policy_checker.can_register_debug_capability(
                    CapabilityTypeName::Protocol,
                    &protocol_name,
                    &Moniker::try_from(*dest).unwrap(),
                    &env,
                ),
                Ok(()),
                "{:?}",
                (dest, env)
            );
        }

        for (dest, env) in INVALID_CASES {
            let protocol_name: Name = "debug_service1".parse().unwrap();
            let env: Name = env.parse().unwrap();
            assert_matches!(
                global_policy_checker.can_register_debug_capability(
                    CapabilityTypeName::Protocol,
                    &protocol_name,
                    &Moniker::try_from(*dest).unwrap(),
                    &env,
                ),
                Err(_),
                "{:?}",
                (dest, env)
            );
        }

        Ok(())
    }

    // Tests `GlobalPolicyChecker::can_route_debug_capability()` for capability sources of type
    // `Capability` with collection allowlist entries.
    async fn global_policy_checker_can_route_debug_capability_with_collection_allowlist_entry(
        &self,
    ) -> Result<(), Error> {
        let mut policy_builder = CapabilityAllowlistPolicyBuilder::new();
        policy_builder.add_debug_capability_policy(
            DebugCapabilityKey {
                name: "debug_service1".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
                env_name: "bar_env".parse().unwrap(),
            },
            AllowlistEntryBuilder::new()
                .exact("root")
                .exact("bootstrap")
                .any_descendant_in_collection("coll1"),
        );
        policy_builder.add_debug_capability_policy(
            DebugCapabilityKey {
                name: "debug_service1".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
                env_name: "foo_env".parse().unwrap(),
            },
            AllowlistEntryBuilder::new().exact("root").exact("bootstrap2").build(),
        );
        policy_builder.add_debug_capability_policy(
            DebugCapabilityKey {
                name: "debug_service1".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
                env_name: "baz_env".parse().unwrap(),
            },
            AllowlistEntryBuilder::new()
                .exact("root")
                .exact("bootstrap3")
                .any_descendant_in_collection("coll4"),
        );
        let global_policy_checker = GlobalPolicyChecker::new(Arc::new(policy_builder.build()));

        // dest, env
        const VALID_CASES: &[(&[&str], &str)] = &[
            (&["root", "bootstrap", "coll1:instance1"], "bar_env"),
            (&["root", "bootstrap", "coll1:instance1", "child"], "bar_env"),
            (&["root", "bootstrap2"], "foo_env"),
            (&["root", "bootstrap3", "coll4:instance4"], "baz_env"),
            (&["root", "bootstrap3", "coll4:instance4", "child"], "baz_env"),
        ];

        const INVALID_CASES: &[(&[&str], &str)] = &[
            (&["root", "bootstrap"], "bar_env"),
            (&["root", "not_bootstrap"], "bar_env"),
            (&["root", "not_bootstrap"], "foo_env"),
            (&["root", "bootstrap3", "child"], "baz_env"),
            (&["root", "bootstrap"], "baz_env"),
        ];

        for (dest, env) in VALID_CASES {
            let protocol_name: Name = "debug_service1".parse().unwrap();
            let env: Name = env.parse().unwrap();
            assert_matches!(
                global_policy_checker.can_register_debug_capability(
                    CapabilityTypeName::Protocol,
                    &protocol_name,
                    &Moniker::try_from(*dest).unwrap(),
                    &env,
                ),
                Ok(()),
                "{:?}",
                (dest, env)
            );
        }

        for (dest, env) in INVALID_CASES {
            let protocol_name: Name = "debug_service1".parse().unwrap();
            let env: Name = env.parse().unwrap();
            assert_matches!(
                global_policy_checker.can_register_debug_capability(
                    CapabilityTypeName::Protocol,
                    &protocol_name,
                    &Moniker::try_from(*dest).unwrap(),
                    &env,
                ),
                Err(_),
                "{:?}",
                (dest, env)
            );
        }

        Ok(())
    }

    // Tests `GlobalPolicyChecker::can_route_capability()` for builtin capabilities.
    async fn global_policy_checker_can_route_capability_builtin_cap(&self) -> Result<(), Error> {
        let mut policy_builder = CapabilityAllowlistPolicyBuilder::new();
        policy_builder.add_capability_policy(
            CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentManager,
                source_name: "test".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Directory,
            },
            vec![
                AllowlistEntryBuilder::new().exact("root").build(),
                AllowlistEntryBuilder::new().exact("root").exact("core").build(),
            ],
        );
        let global_policy_checker = GlobalPolicyChecker::new(Arc::new(policy_builder.build()));

        let dir_capability = CapabilitySource::Builtin(BuiltinSource {
            capability: InternalCapability::Directory("test".parse().unwrap()),
        });
        let valid_path_0 = Moniker::try_from(["root"]).unwrap();
        let valid_path_1 = Moniker::try_from(["root", "core"]).unwrap();
        let invalid_path_0 = Moniker::try_from(["foobar"]).unwrap();
        let invalid_path_1 = Moniker::try_from(["foo", "bar", "foobar"]).unwrap();

        assert_matches!(
            global_policy_checker.can_route_capability(&dir_capability, &valid_path_0),
            Ok(())
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&dir_capability, &valid_path_1),
            Ok(())
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&dir_capability, &invalid_path_0),
            Err(_)
        );
        assert_matches!(
            global_policy_checker.can_route_capability(&dir_capability, &invalid_path_1),
            Err(_)
        );
        Ok(())
    }

    // Tests `GlobalPolicyChecker::can_route_capability()` for policy that includes non-exact
    // `AllowlistEntry::Realm` entries.
    async fn global_policy_checker_can_route_capability_with_realm_allowlist_entry(
        &self,
    ) -> Result<(), Error> {
        let mut policy_builder = CapabilityAllowlistPolicyBuilder::new();
        policy_builder.add_capability_policy(
            CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentManager,
                source_name: "fuchsia.kernel.MmioResource".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
            },
            vec![
                AllowlistEntryBuilder::new().exact("tests").any_descendant(),
                AllowlistEntryBuilder::new().exact("core").exact("tests").any_descendant(),
            ],
        );
        let global_policy_checker = GlobalPolicyChecker::new(Arc::new(policy_builder.build()));
        let protocol_capability = CapabilitySource::Namespace(NamespaceSource {
            capability: ComponentCapability::Protocol(ProtocolDecl {
                name: "fuchsia.kernel.MmioResource".parse().unwrap(),
                source_path: Some("/svc/fuchsia.kernel.MmioResource".parse().unwrap()),
                delivery: Default::default(),
            }),
        });

        macro_rules! can_route {
            ($moniker:expr) => {
                global_policy_checker.can_route_capability(&protocol_capability, $moniker)
            };
        }

        assert!(can_route!(&Moniker::try_from(["tests", "test1"]).unwrap()).is_ok());
        assert!(can_route!(&Moniker::try_from(["tests", "coll:test1"]).unwrap()).is_ok());
        assert!(can_route!(&Moniker::try_from(["tests", "test1", "util"]).unwrap()).is_ok());
        assert!(can_route!(&Moniker::try_from(["tests", "test2"]).unwrap()).is_ok());
        assert!(can_route!(&Moniker::try_from(["core", "tests", "test"]).unwrap()).is_ok());
        assert!(can_route!(&Moniker::try_from(["core", "tests", "coll:t"]).unwrap()).is_ok());

        assert!(can_route!(&Moniker::try_from(["foo"]).unwrap()).is_err());
        assert!(can_route!(&Moniker::try_from(["tests"]).unwrap()).is_err());
        assert!(can_route!(&Moniker::try_from(["core", "foo"]).unwrap()).is_err());
        assert!(can_route!(&Moniker::try_from(["core", "tests"]).unwrap()).is_err());
        assert!(can_route!(&Moniker::try_from(["core", "tests:test"]).unwrap()).is_err());
        Ok(())
    }

    // Tests `GlobalPolicyChecker::can_route_capability()` for policy that includes non-exact
    // `AllowlistEntry::Collection` entries.
    async fn global_policy_checker_can_route_capability_with_collection_allowlist_entry(
        &self,
    ) -> Result<(), Error> {
        let mut policy_builder = CapabilityAllowlistPolicyBuilder::new();
        policy_builder.add_capability_policy(
            CapabilityAllowlistKey {
                source_moniker: ExtendedMoniker::ComponentManager,
                source_name: "fuchsia.kernel.MmioResource".parse().unwrap(),
                source: CapabilityAllowlistSource::Self_,
                capability: CapabilityTypeName::Protocol,
            },
            vec![
                AllowlistEntryBuilder::new().any_descendant_in_collection("tests"),
                AllowlistEntryBuilder::new().exact("core").any_descendant_in_collection("tests"),
            ],
        );
        let global_policy_checker = GlobalPolicyChecker::new(Arc::new(policy_builder.build()));
        let protocol_capability = CapabilitySource::Namespace(NamespaceSource {
            capability: ComponentCapability::Protocol(ProtocolDecl {
                name: "fuchsia.kernel.MmioResource".parse().unwrap(),
                source_path: Some("/svc/fuchsia.kernel.MmioResource".parse().unwrap()),
                delivery: Default::default(),
            }),
        });

        macro_rules! can_route {
            ($moniker:expr) => {
                global_policy_checker.can_route_capability(&protocol_capability, $moniker)
            };
        }

        assert!(can_route!(&Moniker::try_from(["tests:t1"]).unwrap()).is_ok());
        assert!(can_route!(&Moniker::try_from(["tests:t2"]).unwrap()).is_ok());
        assert!(can_route!(&Moniker::try_from(["tests:t1", "util"]).unwrap()).is_ok());
        assert!(can_route!(&Moniker::try_from(["core", "tests:t1"]).unwrap()).is_ok());
        assert!(can_route!(&Moniker::try_from(["core", "tests:t2"]).unwrap()).is_ok());

        assert!(can_route!(&Moniker::try_from(["foo"]).unwrap()).is_err());
        assert!(can_route!(&Moniker::try_from(["tests"]).unwrap()).is_err());
        assert!(can_route!(&Moniker::try_from(["coll:foo"]).unwrap()).is_err());
        assert!(can_route!(&Moniker::try_from(["core", "foo"]).unwrap()).is_err());
        assert!(can_route!(&Moniker::try_from(["core", "coll:tests"]).unwrap()).is_err());
        Ok(())
    }
}

// Creates a SecurityPolicy based on the capability allowlist entries provided during
// construction.
struct CapabilityAllowlistPolicyBuilder {
    capability_policy: HashMap<CapabilityAllowlistKey, HashSet<AllowlistEntry>>,
    debug_capability_policy: HashMap<DebugCapabilityKey, HashSet<DebugCapabilityAllowlistEntry>>,
}

impl CapabilityAllowlistPolicyBuilder {
    pub fn new() -> Self {
        Self { capability_policy: HashMap::new(), debug_capability_policy: HashMap::new() }
    }

    /// Add a new entry to the configuration.
    pub fn add_capability_policy<'a>(
        &'a mut self,
        key: CapabilityAllowlistKey,
        value: Vec<AllowlistEntry>,
    ) -> &'a mut Self {
        let value_set = HashSet::from_iter(value.iter().cloned());
        self.capability_policy.insert(key, value_set);
        self
    }

    /// Add a new entry to the configuration.
    pub fn add_debug_capability_policy<'a>(
        &'a mut self,
        key: DebugCapabilityKey,
        dest: AllowlistEntry,
    ) -> &'a mut Self {
        self.debug_capability_policy
            .entry(key)
            .or_default()
            .insert(DebugCapabilityAllowlistEntry::new(dest));
        self
    }

    /// Creates a configuration from the provided policies.
    pub fn build(&self) -> SecurityPolicy {
        SecurityPolicy {
            job_policy: JobPolicyAllowlists {
                ambient_mark_vmo_exec: vec![],
                main_process_critical: vec![],
                create_raw_processes: vec![],
            },
            capability_policy: self.capability_policy.clone(),
            debug_capability_policy: self.debug_capability_policy.clone(),
            child_policy: ChildPolicyAllowlists { reboot_on_terminate: vec![] },
            ..Default::default()
        }
    }
}
