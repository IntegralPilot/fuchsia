// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fuchsia_component::client::Service;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};

use {
    fidl_fuchsia_hardware_power_suspend as fhsuspend, fidl_fuchsia_power_broker as fbroker,
    fidl_fuchsia_power_suspend as fsuspend, fidl_fuchsia_power_system as fsystem,
    fidl_test_sagcontrol as ftsagcontrol, fidl_test_suspendcontrol as ftsuspendcontrol,
};

pub const COMPONENT_NAME: &str = "power_framework_test_realm";
pub const POWER_FRAMEWORK_TEST_REALM_URL: &str = "power-framework#meta/power-framework.cm";

#[async_trait::async_trait]
pub trait PowerFrameworkTestRealmBuilder {
    /// Set up the PowerFrameworkTestRealm component in the RealmBuilder realm.
    /// This configures proper input/output routing of capabilities.
    /// This takes a `manifest_url` to use, which is used by tests that need to
    /// specify a custom power framework test realm.
    async fn power_framework_test_realm_manifest_setup(&self, manifest_url: &str) -> Result<&Self>;
    /// Set up the PowerFrameworkTestRealm component in the RealmBuilder realm.
    /// This configures proper input/output routing of capabilities.
    async fn power_framework_test_realm_setup(&self) -> Result<&Self>;
}

#[async_trait::async_trait]
impl PowerFrameworkTestRealmBuilder for RealmBuilder {
    async fn power_framework_test_realm_manifest_setup(&self, manifest_url: &str) -> Result<&Self> {
        let power_framework_realm =
            self.add_child(COMPONENT_NAME, manifest_url, ChildOptions::new()).await?;

        // Exposes from the the power_framework_test_realm manifest.
        self.add_route(
            Route::new()
                .capability(Capability::protocol::<fbroker::TopologyMarker>())
                .capability(Capability::protocol::<fsystem::ActivityGovernorMarker>())
                .capability(Capability::protocol::<fsuspend::StatsMarker>())
                .capability(Capability::protocol::<ftsagcontrol::StateMarker>())
                .capability(Capability::protocol::<ftsuspendcontrol::DeviceMarker>())
                .capability(Capability::service::<fhsuspend::SuspendServiceMarker>())
                .from(&power_framework_realm)
                .to(Ref::parent()),
        )
        .await?;
        Ok(&self)
    }

    async fn power_framework_test_realm_setup(&self) -> Result<&Self> {
        self.power_framework_test_realm_manifest_setup(POWER_FRAMEWORK_TEST_REALM_URL).await
    }
}

#[async_trait::async_trait]
pub trait PowerFrameworkTestRealmInstance {
    /// Connect to the suspender hosted by PowerFrameworkTestRealm in this Instance.
    async fn power_framework_test_realm_connect_to_suspender(
        &self,
    ) -> Result<fhsuspend::SuspenderProxy>;
}

#[async_trait::async_trait]
impl PowerFrameworkTestRealmInstance for RealmInstance {
    async fn power_framework_test_realm_connect_to_suspender(
        &self,
    ) -> Result<fhsuspend::SuspenderProxy> {
        Service::open_from_dir(self.root.get_exposed_dir(), fhsuspend::SuspendServiceMarker)
            .unwrap()
            .connect_to_instance("default")?
            .connect_to_suspender()
            .map_err(|e| anyhow::anyhow!("Failed to connect to suspender: {:?}", e))
    }
}
