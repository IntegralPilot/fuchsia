// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, anyhow};
use fidl::endpoints::{ClientEnd, Proxy, create_endpoints};
use fidl_fuchsia_power_broker as fbroker;
use fidl_fuchsia_power_system as fsystem;
use fuchsia_async as fasync;
use power_broker_client::PowerElementContext;
use rand::RngExt as _;
use rand::distr::Alphanumeric;
use std::sync::Arc;

/// A power element representing the session.
///
/// This power element is owned and registered by `session_manager`. This power element is
/// added in the power topology as a dependent on the Application Activity element that is
/// owned by the SAG.
///
/// After `session_manager` starts, a power-on lease will be created and retained.
/// The session component may fetch the lease from `session_manager` and decide when to
/// drop it.
///
/// When stopping or restarting the session, the power element and the power-on lease will
/// be recreated, returning thing to the initial started state.
pub struct PowerElement {
    // Keeps the element alive.
    #[allow(dead_code)]
    power_element_context: Arc<PowerElementContext>,

    /// The first lease on the power element.
    lease: Option<ClientEnd<fbroker::LeaseControlMarker>>,
}

/// The power levels defined for the session manager power element.
///
/// | Power Mode        | Level |
/// | ----------------- | ----- |
/// | On                | 1     |
/// | Off               | 0     |
///
static POWER_ON_LEVEL: fbroker::PowerLevel = 1;

impl PowerElement {
    /// # Panics
    /// If internal invariants about the state of the `lease` field are violated.
    pub async fn new() -> Result<Self, anyhow::Error> {
        let topology = fuchsia_component::client::connect_to_protocol::<fbroker::TopologyMarker>()?;
        let activity_governor =
            fuchsia_component::client::connect_to_protocol::<fsystem::ActivityGovernorMarker>()?;

        // Create the PowerMode power element depending on the Execution State of SAG.
        let power_elements = activity_governor
            .get_power_elements()
            .await
            .context("cannot get power elements from SAG")?;
        let Some(Some(application_activity_token)) = power_elements
            .application_activity
            .map(|application_activity| application_activity.assertive_dependency_token)
        else {
            return Err(anyhow!("Did not find application activity assertive dependency token"));
        };

        // TODO(https://fxbug.dev/316023943): also depend on execution_resume_latency after implemented.
        let power_levels: Vec<u8> = (0..=POWER_ON_LEVEL).collect();
        let random_string: String =
            rand::rng().sample_iter(&Alphanumeric).take(8).map(char::from).collect();
        let (element_runner_client, element_runner) =
            create_endpoints::<fbroker::ElementRunnerMarker>();
        let power_element_context = Arc::new(
            PowerElementContext::builder(
                &topology,
                format!("session-manager-element-{random_string}").as_str(),
                &power_levels,
                element_runner_client,
            )
            .initial_current_level(POWER_ON_LEVEL)
            .dependencies(vec![fbroker::LevelDependency {
                dependent_level: Some(POWER_ON_LEVEL),
                requires_token: Some(application_activity_token),
                requires_level_by_preference: Some(vec![
                    fsystem::ApplicationActivityLevel::Active.into_primitive(),
                ]),
                ..Default::default()
            }])
            .build()
            .await
            .map_err(|e| anyhow!("PowerBroker::AddElementError({e:?}"))?,
        );
        let pe_context = power_element_context.clone();
        fasync::Task::local(async move {
            pe_context.run(element_runner, None /* inspect_node */, None /* update_fn */).await;
        })
        .detach();

        // Power on by holding a lease.
        let lease = power_element_context
            .lessor
            .lease(POWER_ON_LEVEL)
            .await?
            .map_err(|e| anyhow!("PowerBroker::LeaseError({e:?})"))?;

        // Wait for the lease to be satisfied.
        let lease = lease.into_proxy();
        let mut status = fbroker::LeaseStatus::Unknown;
        loop {
            match lease.watch_status(status).await? {
                fbroker::LeaseStatus::Satisfied => break,
                new_status => status = new_status,
            }
        }
        let lease = lease
            .into_client_end()
            .expect("Proxy should be in a valid state to convert into client end");

        let boot_control =
            fuchsia_component::client::connect_to_protocol::<fsystem::BootControlMarker>()?;
        let () = boot_control.set_boot_complete().await?;

        Ok(Self { power_element_context, lease: Some(lease) })
    }

    pub fn take_lease(&mut self) -> Option<ClientEnd<fbroker::LeaseControlMarker>> {
        self.lease.take()
    }

    pub fn has_lease(&self) -> bool {
        self.lease.is_some()
    }
}
