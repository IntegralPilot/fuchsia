// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use async_trait::async_trait;
use fidl_fuchsia_power_broker as fbroker;
use fidl_fuchsia_power_system as fsystem;
use log::warn;

pub const POWER_LEVEL_OFF: u8 = 0;
pub const POWER_LEVEL_SUSPEND: u8 = 1;
pub const POWER_LEVEL_ACTIVE: u8 = 2;

#[cfg_attr(test, derive(Debug))]
pub struct WakeLease {
    _token: fsystem::LeaseToken,
}

impl WakeLease {
    /// Constructs a WakeLease from an underlying token.
    /// This is intended strictly for use in test environments.
    #[doc(hidden)]
    pub fn from_token_for_test(token: fsystem::LeaseToken) -> Self {
        Self { _token: token }
    }
}

#[async_trait]
pub trait PowerManager: Send + Sync {
    async fn take_wake_lease(&self, name: &str) -> Option<WakeLease>;
    async fn power_element_lease(
        &self,
        lease_name: &str,
        dependency_token: fbroker::DependencyToken,
        dependency_level: u8,
    ) -> Result<fbroker::LeaseToken, Error>;
    async fn register_suspend_blocker(
        &self,
        suspend_blocker: fidl::endpoints::ClientEnd<fsystem::SuspendBlockerMarker>,
        name: &str,
    ) -> Result<WakeLease, Error>;
}

pub struct DevicePowerManager {
    proxies: Option<(fsystem::ActivityGovernorProxy, fbroker::TopologyProxy)>,
}

impl DevicePowerManager {
    pub fn new(proxies: Option<(fsystem::ActivityGovernorProxy, fbroker::TopologyProxy)>) -> Self {
        Self { proxies }
    }
}

#[async_trait]
impl PowerManager for DevicePowerManager {
    async fn take_wake_lease(&self, name: &str) -> Option<WakeLease> {
        let (activity_governor, _) = self.proxies.as_ref()?;
        match activity_governor.acquire_wake_lease(name).await {
            Ok(Ok(token)) => Some(WakeLease { _token: token }),
            Ok(Err(e)) => {
                warn!("Failed to acquire wake lease {}: {:?}", name, e);
                None
            }
            Err(e) => {
                warn!("FIDL error when acquiring wake lease {}: {:?}", name, e);
                None
            }
        }
    }

    async fn power_element_lease(
        &self,
        lease_name: &str,
        dependency_token: fbroker::DependencyToken,
        dependency_level: u8,
    ) -> Result<fbroker::LeaseToken, Error> {
        let (_, power_broker) =
            self.proxies.as_ref().ok_or_else(|| anyhow::anyhow!("Power broker not available"))?;

        let dep = fbroker::LeaseDependency {
            requires_token: Some(dependency_token),
            requires_level: Some(dependency_level),
            ..Default::default()
        };
        let (lease_token_local, lease_token_server) = zx::EventPair::create();
        let schema = fbroker::LeaseSchema {
            lease_token: Some(lease_token_server),
            lease_name: Some(lease_name.to_string()),
            dependencies: Some(vec![dep]),
            ..Default::default()
        };
        power_broker
            .lease(schema)
            .await?
            .map_err(|e| anyhow::anyhow!("Power broker returned error: {:?}", e))?;
        Ok(lease_token_local)
    }

    async fn register_suspend_blocker(
        &self,
        suspend_blocker: fidl::endpoints::ClientEnd<fsystem::SuspendBlockerMarker>,
        name: &str,
    ) -> Result<WakeLease, Error> {
        let (activity_governor, _) = self
            .proxies
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Activity governor not available"))?;
        let payload = fsystem::ActivityGovernorRegisterSuspendBlockerRequest {
            suspend_blocker: Some(suspend_blocker),
            name: Some(name.to_string()),
            ..Default::default()
        };
        match activity_governor.register_suspend_blocker(payload).await {
            Ok(Ok(token)) => Ok(WakeLease { _token: token }),
            Ok(Err(e)) => Err(anyhow::anyhow!("RegisterSuspendBlocker error: {:?}", e)),
            Err(e) => Err(anyhow::anyhow!("FIDL error: {:?}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use futures::StreamExt;

    #[fasync::run_singlethreaded(test)]
    async fn test_power_manager_disabled_take_wake_lease() {
        let pm = DevicePowerManager::new(None);
        assert!(pm.take_wake_lease("test-lease").await.is_none());
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_power_manager_disabled_power_element_lease() {
        let pm = DevicePowerManager::new(None);
        let token = zx::Event::create();
        assert!(pm.power_element_lease("test-lease", token, 1).await.is_err());
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_power_manager_disabled_register_suspend_blocker() {
        let pm = DevicePowerManager::new(None);
        let (client, _) = fidl::endpoints::create_endpoints::<fsystem::SuspendBlockerMarker>();
        assert!(pm.register_suspend_blocker(client, "test-blocker").await.is_err());
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_power_manager_take_wake_lease_success() {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fsystem::ActivityGovernorMarker>();
        let (topology_proxy, _topology_server) =
            fidl::endpoints::create_proxy::<fbroker::TopologyMarker>();
        let pm = DevicePowerManager::new(Some((proxy, topology_proxy)));

        let server_fut = async move {
            while let Some(Ok(req)) = stream.next().await {
                match req {
                    fsystem::ActivityGovernorRequest::AcquireWakeLease { name, responder } => {
                        assert_eq!(name, "test-wake-lease");
                        let (_server_token, client_token) = fsystem::LeaseToken::create();
                        let _ = responder.send(Ok(client_token));
                    }
                    other => panic!("Unexpected request: {:?}", other),
                }
            }
        };
        let client_fut = async move {
            let lease = pm.take_wake_lease("test-wake-lease").await;
            assert!(lease.is_some());
        };
        futures::join!(server_fut, client_fut);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_power_manager_take_wake_lease_error() {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fsystem::ActivityGovernorMarker>();
        let (topology_proxy, _topology_server) =
            fidl::endpoints::create_proxy::<fbroker::TopologyMarker>();
        let pm = DevicePowerManager::new(Some((proxy, topology_proxy)));

        let server_fut = async move {
            while let Some(Ok(req)) = stream.next().await {
                match req {
                    fsystem::ActivityGovernorRequest::AcquireWakeLease { name, responder } => {
                        assert_eq!(name, "test-wake-lease");
                        let _ = responder.send(Err(fsystem::AcquireWakeLeaseError::Internal));
                    }
                    other => panic!("Unexpected request: {:?}", other),
                }
            }
        };
        let client_fut = async move {
            let lease = pm.take_wake_lease("test-wake-lease").await;
            assert!(lease.is_none());
        };
        futures::join!(server_fut, client_fut);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_power_manager_register_suspend_blocker_success() {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fsystem::ActivityGovernorMarker>();
        let (topology_proxy, _topology_server) =
            fidl::endpoints::create_proxy::<fbroker::TopologyMarker>();
        let pm = DevicePowerManager::new(Some((proxy, topology_proxy)));

        let server_fut = async move {
            while let Some(Ok(req)) = stream.next().await {
                match req {
                    fsystem::ActivityGovernorRequest::RegisterSuspendBlocker {
                        payload,
                        responder,
                    } => {
                        assert_eq!(payload.name.as_deref(), Some("test-blocker"));
                        let (_server_token, client_token) = fsystem::LeaseToken::create();
                        let _ = responder.send(Ok(client_token));
                    }
                    other => panic!("Unexpected request: {:?}", other),
                }
            }
        };
        let client_fut = async move {
            let (client, _server) =
                fidl::endpoints::create_endpoints::<fsystem::SuspendBlockerMarker>();
            let lease = pm.register_suspend_blocker(client, "test-blocker").await;
            assert_matches!(lease, Ok(_));
        };
        futures::join!(server_fut, client_fut);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_power_manager_register_suspend_blocker_error() {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fsystem::ActivityGovernorMarker>();
        let (topology_proxy, _topology_server) =
            fidl::endpoints::create_proxy::<fbroker::TopologyMarker>();
        let pm = DevicePowerManager::new(Some((proxy, topology_proxy)));

        let server_fut = async move {
            while let Some(Ok(req)) = stream.next().await {
                match req {
                    fsystem::ActivityGovernorRequest::RegisterSuspendBlocker {
                        payload,
                        responder,
                    } => {
                        assert_eq!(payload.name.as_deref(), Some("test-blocker"));
                        let _ = responder.send(Err(fsystem::RegisterSuspendBlockerError::Internal));
                    }
                    other => panic!("Unexpected request: {:?}", other),
                }
            }
        };
        let client_fut = async move {
            let (client, _server) =
                fidl::endpoints::create_endpoints::<fsystem::SuspendBlockerMarker>();
            let lease = pm.register_suspend_blocker(client, "test-blocker").await;
            assert_matches!(lease, Err(_));
        };
        futures::join!(server_fut, client_fut);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_power_manager_power_element_lease_success() {
        let (ag_proxy, _ag_server) =
            fidl::endpoints::create_proxy::<fsystem::ActivityGovernorMarker>();
        let (topology_proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fbroker::TopologyMarker>();
        let pm = DevicePowerManager::new(Some((ag_proxy, topology_proxy)));

        let server_fut = async move {
            while let Some(Ok(req)) = stream.next().await {
                match req {
                    fbroker::TopologyRequest::Lease { responder, .. } => {
                        let _ = responder.send(Ok(()));
                    }
                    other => panic!("Unexpected request: {:?}", other),
                }
            }
        };
        let client_fut = async move {
            let token = zx::Event::create();
            let lease = pm.power_element_lease("test-lease", token, 1).await;
            assert_matches!(lease, Ok(_));
        };
        futures::join!(server_fut, client_fut);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_power_manager_power_element_lease_error() {
        let (ag_proxy, _ag_server) =
            fidl::endpoints::create_proxy::<fsystem::ActivityGovernorMarker>();
        let (topology_proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fbroker::TopologyMarker>();
        let pm = DevicePowerManager::new(Some((ag_proxy, topology_proxy)));

        let server_fut = async move {
            while let Some(Ok(req)) = stream.next().await {
                match req {
                    fbroker::TopologyRequest::Lease { responder, .. } => {
                        let _ = responder.send(Err(fbroker::LeaseError::Internal));
                    }
                    other => panic!("Unexpected request: {:?}", other),
                }
            }
        };
        let client_fut = async move {
            let token = zx::Event::create();
            let lease = pm.power_element_lease("test-lease", token, 1).await;
            assert_matches!(lease, Err(_));
        };
        futures::join!(server_fut, client_fut);
    }
}
