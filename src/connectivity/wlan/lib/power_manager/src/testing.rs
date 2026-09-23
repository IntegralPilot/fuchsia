// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use async_trait::async_trait;
use fidl_fuchsia_power_system as fsystem;
use fuchsia_sync::Mutex;
use std::sync::Arc;
use wlan_power_manager::{PowerManager, WakeLease};

pub struct TestPowerManager {
    pub calls: Arc<Mutex<Vec<String>>>,
    pub active_leases: Arc<Mutex<Vec<(String, fsystem::LeaseToken)>>>,
}

impl TestPowerManager {
    pub fn new() -> Self {
        Self { calls: Arc::new(Mutex::new(vec![])), active_leases: Arc::new(Mutex::new(vec![])) }
    }

    pub fn is_lease_dropped(&self, name: &str) -> bool {
        let leases = self.active_leases.lock();
        if let Some((_, token)) = leases.iter().rev().find(|(n, _)| n == name) {
            #[allow(clippy::match_like_matches_macro)]
            match token
                .wait_one(zx::Signals::EVENTPAIR_PEER_CLOSED, zx::MonotonicInstant::INFINITE_PAST)
            {
                zx::WaitResult::TimedOut(_) => false,
                _ => true,
            }
        } else {
            true
        }
    }
}

#[async_trait]
impl PowerManager for TestPowerManager {
    async fn take_wake_lease(&self, name: &str) -> Option<WakeLease> {
        self.calls.lock().push(name.to_string());
        let (local_token, token) = fsystem::LeaseToken::create();
        self.active_leases.lock().push((name.to_string(), local_token));
        Some(WakeLease::from_token_for_test(token))
    }

    async fn power_element_lease(
        &self,
        lease_name: &str,
        _dependency_token: fidl_fuchsia_power_broker::DependencyToken,
        _dependency_level: u8,
    ) -> Result<fidl_fuchsia_power_broker::LeaseToken, Error> {
        self.calls.lock().push(lease_name.to_string());
        let (local_token, remote_token) = zx::EventPair::create();
        self.active_leases.lock().push((lease_name.to_string(), remote_token));
        Ok(local_token)
    }

    async fn register_suspend_blocker(
        &self,
        _suspend_blocker: fidl::endpoints::ClientEnd<fsystem::SuspendBlockerMarker>,
        name: &str,
    ) -> Result<WakeLease, Error> {
        self.calls.lock().push(name.to_string());
        let (local_token, token) = fsystem::LeaseToken::create();
        self.active_leases.lock().push((name.to_string(), local_token));
        Ok(WakeLease::from_token_for_test(token))
    }
}
