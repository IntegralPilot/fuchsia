// Copyright 2024 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

use crate::installer::{MinimalInstallPlan, MinimalInstallResult};
use crate::metrics::MinimalMetricsReporter;
use futures::{FutureExt as _, future::BoxFuture};
use omaha_client::{
    common::{App, CheckOptions, CheckTiming, ProtocolState, UpdateCheckSchedule},
    policy::{CheckDecision, Policy, PolicyEngine, UpdateDecision},
    request_builder::RequestParams,
    time::{PartialComplexTime, TimeSource},
};

pub struct UnusedPolicyData;

// This minimal implementation simply returns that it
// wants the state machine to check for updates in
// this many seconds from the last loop iteration.
const UPDATE_DELAY_SECS: u64 = 15;

/// A minimal implementation of a policy
pub struct MinimalPolicy;

impl Policy for MinimalPolicy {
    type ComputeNextUpdateTimePolicyData = UnusedPolicyData;
    type UpdateCheckAllowedPolicyData = UnusedPolicyData;
    type UpdateCanStartPolicyData = UnusedPolicyData;
    type RebootPolicyData = UnusedPolicyData;
    type InstallPlan = MinimalInstallPlan;

    fn compute_next_update_time(
        _policy_data: &Self::ComputeNextUpdateTimePolicyData,
        _apps: &[App],
        _scheduling: &UpdateCheckSchedule,
        _protocol_state: &ProtocolState,
    ) -> CheckTiming {
        let next_update_time =
            std::time::SystemTime::now() + std::time::Duration::from_secs(UPDATE_DELAY_SECS);
        CheckTiming::builder()
            .time(PartialComplexTime::Wall(next_update_time))
            .minimum_wait(std::time::Duration::from_millis(100))
            .build()
    }

    // This minimal example always allows updates with the default args
    fn update_check_allowed(
        _policy_data: &Self::UpdateCheckAllowedPolicyData,
        _apps: &[App],
        _scheduling: &UpdateCheckSchedule,
        _protocol_state: &ProtocolState,
        _check_options: &CheckOptions,
    ) -> CheckDecision {
        CheckDecision::Ok(RequestParams { ..Default::default() })
    }

    // This minimal example always accepts updates
    fn update_can_start(
        _policy_data: &Self::UpdateCanStartPolicyData,
        _proposed_install_plan: &Self::InstallPlan,
    ) -> UpdateDecision {
        UpdateDecision::Ok
    }

    // This minimal example always allows a "reboot"
    fn reboot_allowed(
        _policy_data: &Self::RebootPolicyData,
        _check_options: &CheckOptions,
    ) -> bool {
        true
    }

    // This minimal example does not need to reboot
    fn reboot_needed(_install_plan: &Self::InstallPlan) -> bool {
        false
    }
}

pub struct MinimalPolicyEngine<T> {
    time_source: T,
    #[expect(dead_code)]
    metrics: MinimalMetricsReporter,
}

impl<T> MinimalPolicyEngine<T> {
    pub fn new(time_source: T, metrics: MinimalMetricsReporter) -> Self {
        Self { time_source, metrics }
    }
}

impl<T> PolicyEngine for MinimalPolicyEngine<T>
where
    T: TimeSource + Clone + Send + Sync,
{
    type TimeSource = T;
    type InstallResult = MinimalInstallResult;
    type InstallPlan = MinimalInstallPlan;

    fn time_source(&self) -> &Self::TimeSource {
        &self.time_source
    }

    fn compute_next_update_time<'a>(
        &'a mut self,
        apps: &'a [App],
        scheduling: &'a UpdateCheckSchedule,
        protocol_state: &'a ProtocolState,
    ) -> BoxFuture<'a, CheckTiming> {
        async move {
            let policy_data = UnusedPolicyData {};
            MinimalPolicy::compute_next_update_time(&policy_data, apps, scheduling, protocol_state)
        }
        .boxed()
    }

    fn update_check_allowed<'a>(
        &'a mut self,
        apps: &'a [App],
        scheduling: &'a UpdateCheckSchedule,
        protocol_state: &'a ProtocolState,
        check_options: &'a CheckOptions,
    ) -> BoxFuture<'a, CheckDecision> {
        async move {
            let policy_data = UnusedPolicyData {};
            MinimalPolicy::update_check_allowed(
                &policy_data,
                apps,
                scheduling,
                protocol_state,
                check_options,
            )
        }
        .boxed()
    }

    fn update_can_start<'a>(
        &'a mut self,
        proposed_install_plan: &'a Self::InstallPlan,
    ) -> BoxFuture<'a, UpdateDecision> {
        async move {
            let policy_data = UnusedPolicyData {};
            MinimalPolicy::update_can_start(&policy_data, proposed_install_plan)
        }
        .boxed()
    }

    fn reboot_allowed<'a>(
        &'a mut self,
        check_options: &'a CheckOptions,
        _install_result: &'a Self::InstallResult,
    ) -> BoxFuture<'a, bool> {
        async move {
            let policy_data = UnusedPolicyData {};
            MinimalPolicy::reboot_allowed(&policy_data, check_options)
        }
        .boxed()
    }

    fn reboot_needed<'a>(&'a mut self, install_plan: &'a Self::InstallPlan) -> BoxFuture<'a, bool> {
        async move { MinimalPolicy::reboot_needed(install_plan) }.boxed()
    }
}
