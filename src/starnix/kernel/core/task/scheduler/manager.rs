// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::{RoleOverrides, Task};
use fidl_fuchsia_scheduler::{
    RoleManagerGetProfileForRoleRequest, RoleManagerMarker, RoleManagerSetRoleRequest,
    RoleManagerSynchronousProxy, RoleName, RoleTarget, RoleType,
};
use fuchsia_component::client::connect_to_protocol_sync;
use starnix_logging::{impossible_error, log_debug, log_error, log_warn, track_stub};
use starnix_sync::{LockDepMutex, ProfileHandleCacheLock};
use starnix_task_command::TaskCommand;
use starnix_uapi::errors::Errno;
use starnix_uapi::{
    SCHED_BATCH, SCHED_DEADLINE, SCHED_FIFO, SCHED_IDLE, SCHED_NORMAL, SCHED_RESET_ON_FORK,
    SCHED_RR, errno, error, sched_param,
};
use std::collections::HashMap;

pub struct SchedulerManager {
    role_manager: Option<RoleManagerSynchronousProxy>,
    role_overrides: RoleOverrides,
    profile_handle_cache: LockDepMutex<HashMap<String, zx::Profile>, ProfileHandleCacheLock>,
}

impl SchedulerManager {
    /// Create a new SchedulerManager which will apply any provided `role_overrides` before
    /// computing a role name based on a Task's scheduler state.
    pub fn new(role_overrides: RoleOverrides) -> SchedulerManager {
        let profile_handle_cache = Default::default();
        let role_manager = fuchsia_runtime::with_thread_self(|thread| {
            let role_manager = connect_to_protocol_sync::<RoleManagerMarker>().unwrap();
            if let Err(e) = Self::set_thread_role_inner(
                &role_manager,
                thread,
                SchedulerState::default().role_name(),
                &profile_handle_cache,
            ) {
                log_debug!("Setting thread role failed ({e:?}), will not set thread priority.");
                None
            } else {
                log_debug!("Thread role set successfully, scheduler manager initialized.");
                Some(role_manager)
            }
        });

        SchedulerManager { role_manager, role_overrides, profile_handle_cache }
    }

    /// Create a new empty SchedulerManager for testing.
    pub fn empty_for_tests() -> Self {
        Self {
            role_manager: None,
            role_overrides: RoleOverrides::new().build().unwrap(),
            profile_handle_cache: Default::default(),
        }
    }

    /// Create a new SchedulerManager with a custom role manager and overrides for testing.
    pub fn new_for_tests(
        role_manager: Option<RoleManagerSynchronousProxy>,
        role_overrides: RoleOverrides,
    ) -> Self {
        Self { role_manager, role_overrides, profile_handle_cache: Default::default() }
    }

    /// Return the currently active role name for this task. Requires read access to `task`'s state,
    /// should only be called by code which is not already modifying the provided `task`.
    pub fn role_name(&self, task: &Task) -> Result<&str, Errno> {
        let scheduler_state = task.read().scheduler_state;
        self.role_name_inner(task, scheduler_state)
    }

    pub fn role_name_inner(
        &self,
        task: &Task,
        scheduler_state: SchedulerState,
    ) -> Result<&str, Errno> {
        let thread_group = task.thread_group();
        let thread_group_state = thread_group.read();
        // Only inherit custom roles if the process has executed `execve`. Child processes created
        // via `fork` only inherit standard scheduling policies (SCHED_NORMAL/SCHED_FIFO) with
        // default profiles to maintain basic parity with Linux and prevent demand amplification.
        if thread_group_state.did_exec {
            let process_name = if task.tid == task.pid {
                task.command()
            } else {
                task.pid.get_task().map_err(|_| errno!(EINVAL))?.command()
            };
            let thread_name = task.command();

            let cpuset_path = task.read().cpuset_path.clone();

            return Ok(self.resolve_role_name(
                &process_name,
                &thread_name,
                &cpuset_path,
                scheduler_state,
            ));
        }
        Ok(scheduler_state.role_name())
    }

    pub fn resolve_role_name(
        &self,
        process_name: &TaskCommand,
        thread_name: &TaskCommand,
        cgroup_path: &str,
        scheduler_state: SchedulerState,
    ) -> &str {
        if let Some(name) =
            self.role_overrides.get_role_name(process_name, thread_name, cgroup_path)
        {
            return name;
        }
        scheduler_state.role_name()
    }

    /// Give the provided `task`'s Zircon thread a role.
    pub fn set_thread_role(&self, task: &Task, role_name: &str) -> Result<(), Errno> {
        let Some(role_manager) = self.role_manager.as_ref() else {
            log_debug!("no role manager for setting role");
            return Ok(());
        };

        let zircon_thread = {
            let running_state = match task.running_state() {
                Ok(live) => live,
                Err(_) => {
                    log_debug!(
                        "thread role update requested for task without live state, skipping"
                    );
                    return Ok(());
                }
            };
            let Some(zircon_thread) = running_state.thread.get() else {
                log_debug!("thread role update requested for task without thread, skipping");
                return Ok(());
            };
            zircon_thread.clone()
        };
        Self::set_thread_role_inner(
            role_manager,
            &zircon_thread.thread,
            role_name,
            &self.profile_handle_cache,
        )?;

        Ok(())
    }

    fn set_thread_role_inner(
        role_manager: &RoleManagerSynchronousProxy,
        thread: &zx::Thread,
        role_name: &str,
        cache: &LockDepMutex<HashMap<String, zx::Profile>, ProfileHandleCacheLock>,
    ) -> Result<(), Errno> {
        log_debug!(role_name; "setting thread role");

        {
            let params = cache.lock();
            if let Some(profile) = params.get(role_name) {
                match thread.set_profile(&profile, 0) {
                    Ok(_) => return Ok(()),
                    Err(e) => log_error!("Failed to set role profile {:?}", e),
                }
            }
        }

        let request = RoleManagerGetProfileForRoleRequest {
            role: Some(RoleName { role: role_name.to_string() }),
            target: Some(RoleType::Task),
            ..Default::default()
        };
        match role_manager.get_profile_for_role(request, zx::MonotonicInstant::INFINITE) {
            Ok(Ok(response)) => {
                let Some(profile) = response.profile else {
                    log_warn!("GetRole returned success but no profile handle");
                    return error!(EINVAL);
                };

                if let Err(e) = thread.set_profile(&profile, 0) {
                    log_warn!(e:%; "Failed to set thread profile from handle");
                    return error!(EINVAL);
                }
                cache.lock().insert(role_name.to_string(), profile);
                Ok(())
            }
            Ok(Err(e)) => {
                log_warn!(e:%; "GetRole returned error");
                Self::set_thread_role_legacy(role_manager, thread, role_name)
            }
            Err(e) => {
                log_warn!(e:%; "GetRole FIDL call failed");
                Self::set_thread_role_legacy(role_manager, thread, role_name)
            }
        }
    }

    fn set_thread_role_legacy(
        role_manager: &RoleManagerSynchronousProxy,
        thread: &zx::Thread,
        role_name: &str,
    ) -> Result<(), Errno> {
        let thread = thread.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(impossible_error)?;
        let request = RoleManagerSetRoleRequest {
            target: Some(RoleTarget::Thread(thread)),
            role: Some(RoleName { role: role_name.to_string() }),
            ..Default::default()
        };
        let _ = role_manager.set_role(request, zx::MonotonicInstant::INFINITE).map_err(|err| {
            log_warn!(err:%; "Unable to set thread role.");
            errno!(EINVAL)
        })?;
        Ok(())
    }
}

/// The task normal priority, used for favoring or disfavoring a task running
/// with some non-real-time scheduling policies. Ranges from -20 to +19 in
/// "user-space" representation and +1 to +40 in "kernel-internal"
/// representation. See "The nice value" at sched(7) for full specification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct NormalPriority {
    /// 1 (weakest) to 40 (strongest) (in "kernel-internal" representation),
    /// from setpriority()
    value: u8,
}

impl NormalPriority {
    const MIN_VALUE: u8 = 1;
    const DEFAULT_VALUE: u8 = 20;
    const MAX_VALUE: u8 = 40;

    /// Creates a normal priority from a value to be interpreted according
    /// to the "user-space nice" (-20..=19) scale, clamping values outside
    /// that scale.
    ///
    /// It would be strange for this to be called from anywhere outside of
    /// the setpriority system call.
    pub(crate) fn from_setpriority_syscall(user_nice: i32) -> Self {
        Self {
            value: (Self::DEFAULT_VALUE as i32)
                .saturating_sub(user_nice)
                .clamp(Self::MIN_VALUE as i32, Self::MAX_VALUE as i32) as u8,
        }
    }

    /// Creates a normal priority from a value to be interpreted according
    /// to the "user-space nice" (-20..=19) scale, rejecting values outside
    /// that scale.
    ///
    /// It would be strange for this to be called from anywhere outside of
    /// our Binder implementation.
    pub fn from_binder(user_nice: i8) -> Result<Self, Errno> {
        let value = (Self::DEFAULT_VALUE as i8).saturating_sub(user_nice);
        if value < (Self::MIN_VALUE as i8) || value > (Self::MAX_VALUE as i8) {
            return error!(EINVAL);
        }
        Ok(Self { value: u8::try_from(value).expect("normal priority should fit in a u8") })
    }

    /// Returns this normal priority's integer representation according
    /// to the "user-space nice" (-20..=19) scale.
    pub fn as_nice(&self) -> i8 {
        (Self::DEFAULT_VALUE as i8) - (self.value as i8)
    }

    /// Returns this normal priority's integer representation according
    /// to the "kernel space nice" (1..=40) scale.
    pub(crate) fn raw_priority(&self) -> u8 {
        self.value
    }

    /// Returns whether this normal priority exceeds the given limit.
    pub(crate) fn exceeds(&self, limit: u64) -> bool {
        limit < (self.value as u64)
    }
}

impl std::default::Default for NormalPriority {
    fn default() -> Self {
        Self { value: Self::DEFAULT_VALUE }
    }
}

/// The task real-time priority, used for favoring or disfavoring a task
/// running with real-time scheduling policies. See "Scheduling policies"
/// at sched(7) for full specification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct RealtimePriority {
    /// 1 (weakest) to 99 (strongest), from sched_setscheduler() and
    /// sched_setparam(). Only meaningfully used for Fifo and
    /// Round-Robin; set to 0 for other policies.
    value: u8,
}

impl RealtimePriority {
    const NON_REAL_TIME_VALUE: u8 = 0;
    const MIN_VALUE: u8 = 1;
    const MAX_VALUE: u8 = 99;

    const NON_REAL_TIME: RealtimePriority = RealtimePriority { value: Self::NON_REAL_TIME_VALUE };

    pub(crate) fn exceeds(&self, limit: u64) -> bool {
        limit < (self.value as u64)
    }
}

/// The scheduling policies described in "Scheduling policies" at sched(7).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SchedulingPolicy {
    Normal,
    Batch,
    Idle,
    Fifo,
    RoundRobin,
}

impl SchedulingPolicy {
    fn realtime_priority_min(&self) -> u8 {
        match self {
            Self::Normal | Self::Batch | Self::Idle => RealtimePriority::NON_REAL_TIME_VALUE,
            Self::Fifo | Self::RoundRobin => RealtimePriority::MIN_VALUE,
        }
    }

    fn realtime_priority_max(&self) -> u8 {
        match self {
            Self::Normal | Self::Batch | Self::Idle => RealtimePriority::NON_REAL_TIME_VALUE,
            Self::Fifo | Self::RoundRobin => RealtimePriority::MAX_VALUE,
        }
    }

    pub(crate) fn realtime_priority_from(&self, priority: i32) -> Result<RealtimePriority, Errno> {
        let priority = u8::try_from(priority).map_err(|_| errno!(EINVAL))?;
        if priority < self.realtime_priority_min() || priority > self.realtime_priority_max() {
            return error!(EINVAL);
        }
        Ok(RealtimePriority { value: priority })
    }
}

impl TryFrom<u32> for SchedulingPolicy {
    type Error = Errno;

    fn try_from(value: u32) -> Result<Self, Errno> {
        Ok(match value {
            SCHED_NORMAL => Self::Normal,
            SCHED_BATCH => Self::Batch,
            SCHED_IDLE => Self::Idle,
            SCHED_FIFO => Self::Fifo,
            SCHED_RR => Self::RoundRobin,
            _ => {
                return error!(EINVAL);
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerState {
    pub(crate) policy: SchedulingPolicy,
    /// Although nice is only used for Normal and Batch, normal priority
    /// ("nice") is still maintained, observable, and alterable when a
    /// task is using Idle, Fifo, and RoundRobin.
    pub(crate) normal_priority: NormalPriority,
    /// 1 (weakest) to 99 (strongest), from sched_setscheduler() and
    /// sched_setparam(). Only used for Fifo and Round-Robin.
    pub(crate) realtime_priority: RealtimePriority,
    pub(crate) reset_on_fork: bool,
}

impl SchedulerState {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// Create a policy according to the "sched_policy" and "priority" bits of
    /// a flat_binder_object_flags bitmask (see uapi/linux/android/binder.h).
    ///
    /// It would be very strange for this to need to be called anywhere outside
    /// of our Binder implementation.
    pub fn from_binder(policy: u8, priority_or_nice: u8) -> Result<Self, Errno> {
        let (policy, normal_priority, realtime_priority) = match policy as u32 {
            SCHED_NORMAL => (
                SchedulingPolicy::Normal,
                NormalPriority::from_binder(priority_or_nice as i8)?,
                RealtimePriority::NON_REAL_TIME,
            ),
            SCHED_BATCH => (
                SchedulingPolicy::Batch,
                NormalPriority::from_binder(priority_or_nice as i8)?,
                RealtimePriority::NON_REAL_TIME,
            ),
            SCHED_FIFO => (
                SchedulingPolicy::Fifo,
                NormalPriority::default(),
                SchedulingPolicy::Fifo.realtime_priority_from(priority_or_nice as i32)?,
            ),
            SCHED_RR => (
                SchedulingPolicy::RoundRobin,
                NormalPriority::default(),
                SchedulingPolicy::RoundRobin.realtime_priority_from(priority_or_nice as i32)?,
            ),
            _ => return error!(EINVAL),
        };
        Ok(Self { policy, normal_priority, realtime_priority, reset_on_fork: false })
    }

    pub fn fork(self) -> Self {
        if self.reset_on_fork {
            let (policy, normal_priority, realtime_priority) = if self.is_realtime() {
                // If the calling task has a real-time scheduling policy, the
                // policy given to child processes is SCHED_OTHER and the nice is
                // NormalPriority::default() (in all such cases and without caring
                // about whether the caller's nice had been stronger or weaker than
                // NormalPriority::default()).
                (
                    SchedulingPolicy::Normal,
                    NormalPriority::default(),
                    RealtimePriority::NON_REAL_TIME,
                )
            } else {
                // If the calling task has a non-real-time scheduling policy, the
                // state given to child processes is the same as that of the
                // caller except with the caller's nice clamped to
                // NormalPriority::default() at the strongest.
                (
                    self.policy,
                    std::cmp::min(self.normal_priority, NormalPriority::default()),
                    RealtimePriority::NON_REAL_TIME,
                )
            };
            Self {
                policy,
                normal_priority,
                realtime_priority,
                // This flag is disabled in child processes created by fork(2).
                reset_on_fork: false,
            }
        } else {
            self
        }
    }

    /// Return the policy as an integer (SCHED_NORMAL, SCHED_BATCH, &c) bitwise-ored
    /// with the current reset-on-fork status (SCHED_RESET_ON_FORK or 0, depending).
    ///
    /// It would be strange for this to need to be called anywhere outside the
    /// implementation of the sched_getscheduler system call.
    pub fn policy_for_sched_getscheduler(&self) -> u32 {
        let mut base = match self.policy {
            SchedulingPolicy::Normal => SCHED_NORMAL,
            SchedulingPolicy::Batch => SCHED_BATCH,
            SchedulingPolicy::Idle => SCHED_IDLE,
            SchedulingPolicy::Fifo => SCHED_FIFO,
            SchedulingPolicy::RoundRobin => SCHED_RR,
        };
        if self.reset_on_fork {
            base |= SCHED_RESET_ON_FORK;
        }
        base
    }

    /// Return the priority as a field in a sched_param struct.
    ///
    /// It would be strange for this to need to be called anywhere outside the
    /// implementation of the sched_getparam system call.
    pub fn get_sched_param(&self) -> sched_param {
        sched_param {
            sched_priority: (if self.is_realtime() {
                self.realtime_priority.value
            } else {
                RealtimePriority::NON_REAL_TIME_VALUE
            }) as i32,
        }
    }

    pub fn normal_priority(&self) -> NormalPriority {
        self.normal_priority
    }

    pub fn is_realtime(&self) -> bool {
        match self.policy {
            SchedulingPolicy::Normal | SchedulingPolicy::Batch | SchedulingPolicy::Idle => false,
            SchedulingPolicy::Fifo | SchedulingPolicy::RoundRobin => true,
        }
    }

    /// Returns a number 0-31 (inclusive) mapping Linux scheduler priority to a Zircon priority
    /// level for the fair scheduler.
    ///
    /// The range of 32 Zircon priorities is divided into a region for each flavor of Linux
    /// scheduling:
    ///
    /// 1. 0 is used for SCHED_IDLE, the lowest priority Linux tasks.
    /// 2. 6-15 (inclusive) is used for lower-than-default-priority SCHED_OTHER/SCHED_BATCH tasks.
    /// 3. 16 is used for the default priority SCHED_OTHER/SCHED_BATCH, the same as Zircon's
    ///    default for Fuchsia processes.
    /// 4. 17-26 (inclusive) is used for higher-than-default-priority SCHED_OTHER/SCHED_BATCH tasks.
    /// 5. Realtime tasks receive their own profile name.
    pub(crate) fn role_name(&self) -> &'static str {
        match self.policy {
            // Mapped to 0; see "the [...] nice value has no influence for [the SCHED_IDLE] policy"
            // at sched(7).
            SchedulingPolicy::Idle => FAIR_PRIORITY_ROLE_NAMES[0],

            // Configured with nice 0-40 and mapped to 6-26. 20 is the default nice which we want to
            // map to 16.
            SchedulingPolicy::Normal => {
                FAIR_PRIORITY_ROLE_NAMES[(self.normal_priority.value as usize / 2) + 6]
            }
            SchedulingPolicy::Batch => {
                track_stub!(TODO("https://fxbug.dev/308055542"), "SCHED_BATCH hinting");
                FAIR_PRIORITY_ROLE_NAMES[(self.normal_priority.value as usize / 2) + 6]
            }

            // Configured with priority 1-99, mapped to a constant bandwidth profile. Priority
            // between realtime tasks is ignored because we don't currently have a way to tell the
            // scheduler that a given realtime task is more important than another without
            // specifying an earlier deadline for the higher priority task. We can't specify
            // deadlines at runtime, so we'll treat their priorities all the same.
            SchedulingPolicy::Fifo | SchedulingPolicy::RoundRobin => REALTIME_ROLE_NAME,
        }
    }

    // TODO: https://fxbug.dev/425726327 - better understand what are Binder's requirements when
    // comparing one scheduling with another.
    pub fn is_less_than_for_binder(&self, other: Self) -> bool {
        match self.policy {
            SchedulingPolicy::Fifo | SchedulingPolicy::RoundRobin => match other.policy {
                SchedulingPolicy::Fifo | SchedulingPolicy::RoundRobin => {
                    self.realtime_priority < other.realtime_priority
                }
                SchedulingPolicy::Normal | SchedulingPolicy::Batch | SchedulingPolicy::Idle => {
                    false
                }
            },
            SchedulingPolicy::Normal => match other.policy {
                SchedulingPolicy::Fifo | SchedulingPolicy::RoundRobin => true,
                SchedulingPolicy::Normal => {
                    self.normal_priority.value < other.normal_priority.value
                }
                SchedulingPolicy::Batch | SchedulingPolicy::Idle => false,
            },
            SchedulingPolicy::Batch => match other.policy {
                SchedulingPolicy::Fifo
                | SchedulingPolicy::RoundRobin
                | SchedulingPolicy::Normal => true,
                SchedulingPolicy::Batch => self.normal_priority.value < other.normal_priority.value,
                SchedulingPolicy::Idle => false,
            },
            // see "the [...] nice value has no influence for [the SCHED_IDLE] policy" at sched(7).
            SchedulingPolicy::Idle => match other.policy {
                SchedulingPolicy::Fifo
                | SchedulingPolicy::RoundRobin
                | SchedulingPolicy::Normal
                | SchedulingPolicy::Batch => true,
                SchedulingPolicy::Idle => false,
            },
        }
    }
}

impl std::default::Default for SchedulerState {
    fn default() -> Self {
        Self {
            policy: SchedulingPolicy::Normal,
            normal_priority: NormalPriority::default(),
            realtime_priority: RealtimePriority::NON_REAL_TIME,
            reset_on_fork: false,
        }
    }
}

pub fn min_priority_for_sched_policy(policy: u32) -> Result<u8, Errno> {
    Ok(match policy {
        SCHED_DEADLINE => RealtimePriority::NON_REAL_TIME_VALUE,
        _ => SchedulingPolicy::try_from(policy)?.realtime_priority_min(),
    })
}

pub fn max_priority_for_sched_policy(policy: u32) -> Result<u8, Errno> {
    Ok(match policy {
        SCHED_DEADLINE => RealtimePriority::NON_REAL_TIME_VALUE,
        _ => SchedulingPolicy::try_from(policy)?.realtime_priority_max(),
    })
}

/// Names of RoleManager roles for each static Zircon priority in the fair scheduler.
/// The index in the array is equal to the static priority.
// LINT.IfChange
const FAIR_PRIORITY_ROLE_NAMES: [&str; 32] = [
    "fuchsia.starnix.fair.0",
    "fuchsia.starnix.fair.1",
    "fuchsia.starnix.fair.2",
    "fuchsia.starnix.fair.3",
    "fuchsia.starnix.fair.4",
    "fuchsia.starnix.fair.5",
    "fuchsia.starnix.fair.6",
    "fuchsia.starnix.fair.7",
    "fuchsia.starnix.fair.8",
    "fuchsia.starnix.fair.9",
    "fuchsia.starnix.fair.10",
    "fuchsia.starnix.fair.11",
    "fuchsia.starnix.fair.12",
    "fuchsia.starnix.fair.13",
    "fuchsia.starnix.fair.14",
    "fuchsia.starnix.fair.15",
    "fuchsia.starnix.fair.16",
    "fuchsia.starnix.fair.17",
    "fuchsia.starnix.fair.18",
    "fuchsia.starnix.fair.19",
    "fuchsia.starnix.fair.20",
    "fuchsia.starnix.fair.21",
    "fuchsia.starnix.fair.22",
    "fuchsia.starnix.fair.23",
    "fuchsia.starnix.fair.24",
    "fuchsia.starnix.fair.25",
    "fuchsia.starnix.fair.26",
    "fuchsia.starnix.fair.27",
    "fuchsia.starnix.fair.28",
    "fuchsia.starnix.fair.29",
    "fuchsia.starnix.fair.30",
    "fuchsia.starnix.fair.31",
];
const REALTIME_ROLE_NAME: &str = "fuchsia.starnix.realtime";
// LINT.ThenChange(src/starnix/config/starnix.profiles)

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[fuchsia::test]
    fn default_role_name() {
        assert_eq!(SchedulerState::default().role_name(), "fuchsia.starnix.fair.16");
    }

    #[fuchsia::test]
    fn normal_with_non_default_nice_role_name() {
        assert_eq!(
            SchedulerState {
                policy: SchedulingPolicy::Normal,
                normal_priority: NormalPriority { value: 10 },
                realtime_priority: RealtimePriority::NON_REAL_TIME,
                reset_on_fork: false
            }
            .role_name(),
            "fuchsia.starnix.fair.11"
        );
        assert_eq!(
            SchedulerState {
                policy: SchedulingPolicy::Normal,
                normal_priority: NormalPriority { value: 27 },
                realtime_priority: RealtimePriority::NON_REAL_TIME,
                reset_on_fork: false
            }
            .role_name(),
            "fuchsia.starnix.fair.19"
        );
    }

    #[fuchsia::test]
    fn fifo_role_name() {
        assert_eq!(
            SchedulerState {
                policy: SchedulingPolicy::Fifo,
                normal_priority: NormalPriority::default(),
                realtime_priority: RealtimePriority { value: 1 },
                reset_on_fork: false
            }
            .role_name(),
            "fuchsia.starnix.realtime",
        );
        assert_eq!(
            SchedulerState {
                policy: SchedulingPolicy::Fifo,
                normal_priority: NormalPriority::default(),
                realtime_priority: RealtimePriority { value: 2 },
                reset_on_fork: false
            }
            .role_name(),
            "fuchsia.starnix.realtime",
        );
        assert_eq!(
            SchedulerState {
                policy: SchedulingPolicy::Fifo,
                normal_priority: NormalPriority::default(),
                realtime_priority: RealtimePriority { value: 99 },
                reset_on_fork: false
            }
            .role_name(),
            "fuchsia.starnix.realtime",
        );
    }

    #[fuchsia::test]
    fn idle_role_name() {
        assert_eq!(
            SchedulerState {
                policy: SchedulingPolicy::Idle,
                normal_priority: NormalPriority { value: 1 },
                realtime_priority: RealtimePriority::NON_REAL_TIME,
                reset_on_fork: false,
            }
            .role_name(),
            "fuchsia.starnix.fair.0"
        );
        assert_eq!(
            SchedulerState {
                policy: SchedulingPolicy::Idle,
                normal_priority: NormalPriority::default(),
                realtime_priority: RealtimePriority::NON_REAL_TIME,
                reset_on_fork: false,
            }
            .role_name(),
            "fuchsia.starnix.fair.0"
        );
        assert_eq!(
            SchedulerState {
                policy: SchedulingPolicy::Idle,
                normal_priority: NormalPriority { value: 40 },
                realtime_priority: RealtimePriority::NON_REAL_TIME,
                reset_on_fork: false,
            }
            .role_name(),
            "fuchsia.starnix.fair.0"
        );
    }

    #[fuchsia::test]
    fn build_policy_from_binder() {
        assert_matches!(SchedulerState::from_binder(SCHED_NORMAL as u8, 0), Ok(_));
        assert_matches!(
            SchedulerState::from_binder(SCHED_NORMAL as u8, ((-21) as i8) as u8),
            Err(_)
        );
        assert_matches!(
            SchedulerState::from_binder(SCHED_NORMAL as u8, ((-20) as i8) as u8),
            Ok(SchedulerState {
                policy: SchedulingPolicy::Normal,
                normal_priority: NormalPriority { value: 40 },
                realtime_priority: RealtimePriority::NON_REAL_TIME,
                reset_on_fork: false,
            })
        );
        assert_matches!(SchedulerState::from_binder(SCHED_NORMAL as u8, 1), Ok(_));
        assert_matches!(SchedulerState::from_binder(SCHED_NORMAL as u8, 19), Ok(_));
        assert_matches!(SchedulerState::from_binder(SCHED_NORMAL as u8, 20), Err(_));
        assert_matches!(SchedulerState::from_binder(SCHED_FIFO as u8, 0), Err(_));
        assert_matches!(SchedulerState::from_binder(SCHED_FIFO as u8, 1), Ok(_));
        assert_matches!(SchedulerState::from_binder(SCHED_FIFO as u8, 99), Ok(_));
        assert_matches!(SchedulerState::from_binder(SCHED_FIFO as u8, 100), Err(_));
        assert_matches!(SchedulerState::from_binder(SCHED_RR as u8, 0), Err(_));
        assert_matches!(SchedulerState::from_binder(SCHED_RR as u8, 1), Ok(_));
        assert_matches!(SchedulerState::from_binder(SCHED_RR as u8, 99), Ok(_));
        assert_matches!(SchedulerState::from_binder(SCHED_RR as u8, 100), Err(_));
        assert_matches!(SchedulerState::from_binder(SCHED_BATCH as u8, 11), Ok(_));
        assert_eq!(SchedulerState::from_binder(SCHED_IDLE as u8, 11), error!(EINVAL));
        assert_matches!(SchedulerState::from_binder(42, 0), Err(_));
        assert_matches!(SchedulerState::from_binder(42, 0), Err(_));
    }

    // NOTE(https://fxbug.dev/425726327): some or all of this test may need to change based
    // on what is learned in https://fxbug.dev/425726327.
    #[fuchsia::test]
    fn is_less_than_for_binder() {
        let rr_50 = SchedulerState {
            policy: SchedulingPolicy::RoundRobin,
            normal_priority: NormalPriority { value: 1 },
            realtime_priority: RealtimePriority { value: 50 },
            reset_on_fork: false,
        };
        let rr_40 = SchedulerState {
            policy: SchedulingPolicy::RoundRobin,
            normal_priority: NormalPriority { value: 1 },
            realtime_priority: RealtimePriority { value: 40 },
            reset_on_fork: false,
        };
        let fifo_50 = SchedulerState {
            policy: SchedulingPolicy::Fifo,
            normal_priority: NormalPriority { value: 1 },
            realtime_priority: RealtimePriority { value: 50 },
            reset_on_fork: false,
        };
        let fifo_40 = SchedulerState {
            policy: SchedulingPolicy::Fifo,
            normal_priority: NormalPriority { value: 1 },
            realtime_priority: RealtimePriority { value: 40 },
            reset_on_fork: false,
        };
        let normal_40 = SchedulerState {
            policy: SchedulingPolicy::Normal,
            normal_priority: NormalPriority { value: 40 },
            realtime_priority: RealtimePriority::NON_REAL_TIME,
            reset_on_fork: true,
        };
        let normal_10 = SchedulerState {
            policy: SchedulingPolicy::Normal,
            normal_priority: NormalPriority { value: 10 },
            realtime_priority: RealtimePriority::NON_REAL_TIME,
            reset_on_fork: true,
        };
        let batch_40 = SchedulerState {
            policy: SchedulingPolicy::Batch,
            normal_priority: NormalPriority { value: 40 },
            realtime_priority: RealtimePriority::NON_REAL_TIME,
            reset_on_fork: true,
        };
        let batch_30 = SchedulerState {
            policy: SchedulingPolicy::Batch,
            normal_priority: NormalPriority { value: 30 },
            realtime_priority: RealtimePriority::NON_REAL_TIME,
            reset_on_fork: true,
        };
        let idle_40 = SchedulerState {
            policy: SchedulingPolicy::Idle,
            normal_priority: NormalPriority { value: 40 },
            realtime_priority: RealtimePriority::NON_REAL_TIME,
            reset_on_fork: true,
        };
        let idle_30 = SchedulerState {
            policy: SchedulingPolicy::Idle,
            normal_priority: NormalPriority { value: 30 },
            realtime_priority: RealtimePriority::NON_REAL_TIME,
            reset_on_fork: true,
        };
        assert!(!fifo_50.is_less_than_for_binder(fifo_50));
        assert!(!rr_50.is_less_than_for_binder(rr_50));
        assert!(!fifo_50.is_less_than_for_binder(rr_50));
        assert!(!rr_50.is_less_than_for_binder(fifo_50));
        assert!(!fifo_50.is_less_than_for_binder(rr_40));
        assert!(rr_40.is_less_than_for_binder(fifo_50));
        assert!(!rr_50.is_less_than_for_binder(fifo_40));
        assert!(fifo_40.is_less_than_for_binder(rr_50));
        assert!(!fifo_40.is_less_than_for_binder(normal_40));
        assert!(normal_40.is_less_than_for_binder(fifo_40));
        assert!(!rr_40.is_less_than_for_binder(normal_40));
        assert!(normal_40.is_less_than_for_binder(rr_40));
        assert!(!normal_40.is_less_than_for_binder(normal_40));
        assert!(!normal_40.is_less_than_for_binder(normal_10));
        assert!(normal_10.is_less_than_for_binder(normal_40));
        assert!(!normal_10.is_less_than_for_binder(batch_40));
        assert!(batch_40.is_less_than_for_binder(normal_10));
        assert!(!batch_40.is_less_than_for_binder(batch_40));
        assert!(!batch_40.is_less_than_for_binder(batch_30));
        assert!(batch_30.is_less_than_for_binder(batch_40));
        assert!(!batch_30.is_less_than_for_binder(idle_40));
        assert!(idle_40.is_less_than_for_binder(batch_30));
        assert!(!idle_40.is_less_than_for_binder(idle_40));
        assert!(!idle_40.is_less_than_for_binder(idle_30));
        assert!(!idle_30.is_less_than_for_binder(idle_40));
    }

    #[fuchsia::test]
    async fn role_overrides_non_realtime() {
        crate::testing::spawn_kernel_and_run_sync(|current_task| {
            let mut builder = RoleOverrides::new();
            builder.add("my_task", "my_task", None, "overridden_role");
            let overrides = builder.build().unwrap();
            let manager = SchedulerManager {
                role_manager: None,
                role_overrides: overrides,
                profile_handle_cache: Default::default(),
            };

            // Set did_exec = true so custom role overrides are applied.
            current_task.thread_group().write().did_exec = true;
            current_task.set_command_name(starnix_task_command::TaskCommand::new(b"my_task"));

            let mut state = SchedulerState::default();
            state.policy = SchedulingPolicy::Normal;

            let role = manager.role_name_inner(current_task, state).expect("role_name");
            assert_eq!(role, "overridden_role");
        })
        .await;
    }
}
