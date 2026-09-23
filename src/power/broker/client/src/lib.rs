// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::Result;
use fidl::endpoints::{ClientEnd, ServerEnd, create_proxy};
use fidl_fuchsia_power_broker as fbroker;
use fuchsia_inspect::{self, Property};
use futures::TryStreamExt;
use futures::future::LocalBoxFuture;
use zx::Rights;

/// A well-known set of PowerLevels to be specified as the valid_levels for a
/// power element. This is the set of levels in fbroker::BinaryPowerLevel.
pub const BINARY_POWER_LEVELS: [fbroker::PowerLevel; 2] = [
    fbroker::BinaryPowerLevel::Off.into_primitive(),
    fbroker::BinaryPowerLevel::On.into_primitive(),
];

pub struct PowerElementContext {
    pub element_control: fbroker::ElementControlProxy,
    pub lessor: fbroker::LessorProxy,
    assertive_dependency_token: Option<fbroker::DependencyToken>,
    name: String,
    initial_level: fbroker::PowerLevel,
}

impl PowerElementContext {
    pub fn builder<'a>(
        topology: &'a fbroker::TopologyProxy,
        element_name: &'a str,
        valid_levels: &'a [fbroker::PowerLevel],
        element_runner_client: ClientEnd<fbroker::ElementRunnerMarker>,
    ) -> PowerElementContextBuilder<'a> {
        PowerElementContextBuilder::new(topology, element_name, valid_levels, element_runner_client)
    }

    pub fn assertive_dependency_token(&self) -> Option<fbroker::DependencyToken> {
        self.assertive_dependency_token.as_ref().and_then(|token| {
            Some(token.duplicate_handle(Rights::SAME_RIGHTS).expect("failed to duplicate token"))
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Runs a procedure that calls an update function when the required power level changes.
    ///
    /// The power element's power level is expected to be updated in `update_fn`, if supplied.
    pub async fn run<'a>(
        &self,
        element_runner: ServerEnd<fbroker::ElementRunnerMarker>,
        inspect_node: Option<fuchsia_inspect::Node>,
        update_fn: Option<Box<dyn Fn(fbroker::PowerLevel) -> LocalBoxFuture<'a, ()> + 'a>>,
    ) {
        let mut stream = element_runner.into_stream();

        let mut last_required_level: fbroker::PowerLevel = self.initial_level;
        let power_level_node = inspect_node
            .as_ref()
            .map(|node| node.create_uint("power_level", last_required_level.into()));

        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fbroker::ElementRunnerRequest::SetLevel { level: required_level, responder } => {
                    log::debug!(
                        element_name:? = &self.name,
                        required_level:?,
                        last_required_level:?;
                        "PowerElementContext::run: new level requested"
                    );
                    if required_level != last_required_level {
                        if let Some(update_fn) = &update_fn {
                            update_fn(required_level).await;
                        }
                        if let Some(ref power_level_node) = power_level_node {
                            power_level_node.set(required_level.into());
                        }
                        last_required_level = required_level;
                    } else {
                        log::debug!(
                            element_name:? = &self.name,
                            required_level:?,
                            last_required_level:?;
                            "PowerElementContext::run: required level has not changed, skipping."
                        );
                    }
                    if let Some(err) = responder.send().err() {
                        log::warn!("PowerElementContext::run: SetLevel response failed: {err}");
                    }
                }
                fbroker::ElementRunnerRequest::_UnknownMethod { .. } => {}
            }
        }
    }
}

pub struct PowerElementContextBuilder<'a> {
    topology: &'a fbroker::TopologyProxy,
    element_name: &'a str,
    initial_current_level: fbroker::PowerLevel,
    element_runner_client: ClientEnd<fbroker::ElementRunnerMarker>,
    valid_levels: &'a [fbroker::PowerLevel],
    dependencies: Vec<fbroker::LevelDependency>,
    register_dependency_tokens: bool,
}

impl<'a> PowerElementContextBuilder<'a> {
    pub fn new(
        topology: &'a fbroker::TopologyProxy,
        element_name: &'a str,
        valid_levels: &'a [fbroker::PowerLevel],
        element_runner_client: ClientEnd<fbroker::ElementRunnerMarker>,
    ) -> Self {
        Self {
            topology,
            element_name,
            valid_levels,
            element_runner_client,
            initial_current_level: Default::default(),
            dependencies: Default::default(),
            register_dependency_tokens: true,
        }
    }

    pub fn initial_current_level(mut self, value: fbroker::PowerLevel) -> Self {
        self.initial_current_level = value;
        self
    }

    pub fn dependencies(mut self, value: Vec<fbroker::LevelDependency>) -> Self {
        self.dependencies = value;
        self
    }

    pub fn register_dependency_tokens(mut self, enable: bool) -> Self {
        self.register_dependency_tokens = enable;
        self
    }

    pub async fn build(self) -> Result<PowerElementContext> {
        let (lessor, lessor_server_end) = create_proxy::<fbroker::LessorMarker>();
        let (element_control, element_control_server_end) =
            create_proxy::<fbroker::ElementControlMarker>();
        self.topology
            .add_element(fbroker::ElementSchema {
                element_name: Some(self.element_name.into()),
                initial_current_level: Some(self.initial_current_level),
                valid_levels: Some(self.valid_levels.to_vec()),
                dependencies: Some(self.dependencies),
                lessor_channel: Some(lessor_server_end),
                element_control: Some(element_control_server_end),
                element_runner: Some(self.element_runner_client),
                ..Default::default()
            })
            .await?
            .map_err(|d| anyhow::anyhow!("{d:?}"))?;

        let assertive_dependency_token = match self.register_dependency_tokens {
            true => {
                let token = fbroker::DependencyToken::create();
                let _ = element_control
                    .register_dependency_token(
                        token
                            .duplicate_handle(Rights::SAME_RIGHTS)
                            .expect("failed to duplicate token"),
                    )
                    .await?
                    .expect("register assertive dependency token");
                Some(token)
            }
            false => None,
        };

        Ok(PowerElementContext {
            element_control,
            lessor,
            assertive_dependency_token,
            name: self.element_name.to_string(),
            initial_level: self.initial_current_level,
        })
    }
}

// TODO(https://fxbug.dev/349841776): Use this as a demo case for test library support for faking
// Power Broker interfaces.
#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fidl::endpoints::{ClientEnd, create_endpoints};
    use fuchsia_async as fasync;
    use futures::channel::mpsc;
    use futures::{FutureExt, StreamExt};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn drive_element_runner(
        element_runner: ClientEnd<fbroker::ElementRunnerMarker>,
        required_power_levels: Vec<fbroker::PowerLevel>,
    ) {
        let proxy = element_runner.into_proxy();
        fasync::Task::local(async move {
            for level in required_power_levels.into_iter().rev() {
                let _ = proxy.set_level(level).await;
            }
        })
        .detach();
    }

    #[fuchsia::test]
    async fn power_element_context_run_passes_required_level_to_update_fn() -> Result<()> {
        let (tx, mut rx) = mpsc::channel(5);

        let (element_control, _element_control_stream) =
            fidl::endpoints::create_proxy_and_stream::<fbroker::ElementControlMarker>();
        let (lessor, _lessor_stream) =
            fidl::endpoints::create_proxy_and_stream::<fbroker::LessorMarker>();
        let (element_runner_client, element_runner) =
            create_endpoints::<fbroker::ElementRunnerMarker>();
        drive_element_runner(element_runner_client, vec![1, 2]);
        let power_element = PowerElementContext {
            element_control,
            lessor,
            assertive_dependency_token: Some(fbroker::DependencyToken::create()),
            name: "test_element".to_string(),
            initial_level: 0,
        };

        power_element
            .run(
                element_runner,
                None,
                Some(Box::new(|power_level| {
                    let mut tx = tx.clone();
                    async move {
                        tx.start_send(power_level).unwrap();
                    }
                    .boxed_local()
                })),
            )
            .await;

        assert_eq!(2, rx.next().await.unwrap());
        assert_eq!(1, rx.next().await.unwrap());
        Ok(())
    }

    #[fuchsia::test]
    async fn power_element_context_run_skips_update_on_same_level() -> Result<()> {
        let (tx, mut rx) = mpsc::channel(5);
        let initial_level = 5;

        let (element_control, _element_control_stream) =
            fidl::endpoints::create_proxy_and_stream::<fbroker::ElementControlMarker>();
        let (lessor, _lessor_stream) =
            fidl::endpoints::create_proxy_and_stream::<fbroker::LessorMarker>();
        let (element_runner_client, element_runner) =
            create_endpoints::<fbroker::ElementRunnerMarker>();
        drive_element_runner(element_runner_client, vec![3, 1, 1, 2, 2, initial_level]);

        let power_element = PowerElementContext {
            element_control,
            lessor,
            assertive_dependency_token: Some(fbroker::DependencyToken::create()),
            name: "test_element".to_string(),
            initial_level,
        };

        power_element
            .run(
                element_runner,
                None,
                Some(Box::new(|power_level| {
                    let mut tx = tx.clone();
                    async move {
                        tx.start_send(power_level).unwrap();
                    }
                    .boxed_local()
                })),
            )
            .await;

        assert_eq!(2, rx.next().await.unwrap());
        assert_eq!(1, rx.next().await.unwrap());
        assert_eq!(3, rx.next().await.unwrap());
        Ok(())
    }

    #[fuchsia::test]
    async fn power_element_context_run_updates_inspect_node() -> Result<()> {
        let inspector = fuchsia_inspect::Inspector::default();
        let (mut tx, rx) = mpsc::channel(5);
        let (tx2, mut rx2) = mpsc::channel(5);
        let rx = Rc::new(RefCell::new(rx));

        let (element_control, _element_control_stream) =
            fidl::endpoints::create_proxy_and_stream::<fbroker::ElementControlMarker>();
        let (lessor, _lessor_stream) =
            fidl::endpoints::create_proxy_and_stream::<fbroker::LessorMarker>();
        let (element_runner_client, element_runner) =
            create_endpoints::<fbroker::ElementRunnerMarker>();
        drive_element_runner(element_runner_client, vec![1, 4, 0, 3]);

        let power_element = PowerElementContext {
            element_control,
            lessor,
            assertive_dependency_token: Some(fbroker::DependencyToken::create()),
            name: "test_element".to_string(),
            initial_level: 0,
        };

        let root = inspector.root().clone_weak();
        fasync::Task::local(async move {
            power_element
                .run(
                    element_runner,
                    Some(root),
                    Some(Box::new(|_| {
                        let rx = rx.clone();
                        let mut tx2 = tx2.clone();
                        async move {
                            tx2.start_send(()).unwrap();
                            rx.borrow_mut().next().await.unwrap();
                        }
                        .boxed_local()
                    })),
                )
                .await;
        })
        .detach();

        // The first communication hasn't updated the tree yet.
        rx2.next().await.unwrap();
        tx.start_send(()).unwrap();

        // Now that the update function has been called twice, the inspect tree
        // should show the first power level. This pattern continues
        rx2.next().await.unwrap();
        assert_data_tree!(inspector, root: {
            power_level: 3u64
        });
        tx.start_send(()).unwrap();

        rx2.next().await.unwrap();
        assert_data_tree!(inspector, root: {
            power_level: 0u64
        });
        tx.start_send(()).unwrap();

        rx2.next().await.unwrap();
        assert_data_tree!(inspector, root: {
            power_level: 4u64
        });
        Ok(())
    }
}
