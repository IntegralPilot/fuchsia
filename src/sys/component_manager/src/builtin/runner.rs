// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use ::routing::policy::ScopedPolicyChecker;
use cm_types::Name;
use std::sync::Arc;
use vfs::directory::entry::OpenRequest;

/// Trait for built-in runner services. Wraps the generic Runner trait to provide a
/// ScopedPolicyChecker for the realm of the component being started, so that runners can enforce
/// security policy.
pub trait BuiltinRunnerFactory: Send + Sync {
    /// Get a connection to a scoped runner by pipelining a
    /// `fuchsia.component.runner/ComponentRunner` server endpoint.
    fn get_scoped_runner(
        self: Arc<Self>,
        checker: ScopedPolicyChecker,
        open_request: OpenRequest<'_>,
    ) -> Result<(), zx::Status>;
}

/// Provides a hook for routing built-in runners to realms.
#[derive(Clone)]
pub struct BuiltinRunner {
    name: Name,
    factory: Arc<dyn BuiltinRunnerFactory>,
    add_to_env: bool,
}

impl BuiltinRunner {
    pub fn new(name: Name, factory: Arc<dyn BuiltinRunnerFactory>, add_to_env: bool) -> Self {
        Self { name, factory, add_to_env }
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    pub fn factory(&self) -> &Arc<dyn BuiltinRunnerFactory> {
        &self.factory
    }

    pub fn add_to_env(&self) -> bool {
        self.add_to_env
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::testing::mocks::MockRunner;
    use crate::model::testing::routing_test_helpers::*;
    use cm_rust::{CapabilityDecl, RunnerDecl};
    use cm_rust_testing::*;
    use moniker::Moniker;

    //   (cm)
    //    |
    //    a
    //
    // a: uses runner "elf" offered from the component mananger.
    #[fuchsia::test]
    async fn use_runner_from_component_manager() {
        let mock_runner = Arc::new(MockRunner::new());

        let components = vec![(
            "a",
            ComponentDeclBuilder::new_empty_component().program_runner("my_runner").build(),
        )];

        // Set up the system.
        let universe = RoutingTestBuilder::new("a", components)
            .set_builtin_capabilities(vec![CapabilityDecl::Runner(RunnerDecl {
                name: "my_runner".parse().unwrap(),
                source_path: None,
            })])
            .add_builtin_runner("my_runner", mock_runner.clone())
            .build()
            .await;

        // Bind the root component.
        universe.start_instance(&Moniker::root()).await.expect("bind failed");

        // Ensure the instance starts up.
        mock_runner.wait_for_url("test:///a").await;
    }

    //   (cm)
    //    |
    //    a
    //    |
    //    b
    //
    // (cm): registers runner "elf".
    // b: uses runner "elf".
    #[fuchsia::test]
    async fn use_runner_from_component_manager_environment() {
        let mock_runner = Arc::new(MockRunner::new());

        let components = vec![
            (
                "a",
                ComponentDeclBuilder::new_empty_component()
                    .child_default("b")
                    .program_runner("elf")
                    .build(),
            ),
            ("b", ComponentDeclBuilder::new_empty_component().program_runner("elf").build()),
        ];

        // Set up the system.
        let universe = RoutingTestBuilder::new("a", components)
            .set_builtin_capabilities(vec![CapabilityDecl::Runner(RunnerDecl {
                name: "elf".parse().unwrap(),
                source_path: None,
            })])
            .add_builtin_runner("elf", mock_runner.clone())
            .build()
            .await;

        // Bind the child component.
        universe.start_instance(&["b"].try_into().unwrap()).await.expect("bind failed");

        // Ensure the instances started up.
        mock_runner.wait_for_urls(&["test:///b"]).await;
    }
}
