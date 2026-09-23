// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::vfs::socket::AuditNetlinkClient;
use linux_uapi::{
    AUDIT_CONFIG_CHANGE, AUDIT_FAIL_PANIC, AUDIT_FAIL_PRINTK, AUDIT_FAIL_SILENT,
    AUDIT_FIRST_USER_MSG, AUDIT_FIRST_USER_MSG2, AUDIT_GET, AUDIT_LAST_USER_MSG,
    AUDIT_LAST_USER_MSG2, AUDIT_SET, AUDIT_STATUS_BACKLOG_LIMIT, AUDIT_STATUS_ENABLED,
    AUDIT_STATUS_FAILURE, AUDIT_STATUS_LOST, AUDIT_STATUS_PID, AUDIT_USER,
};
use starnix_lifecycle::AtomicCounter;
use starnix_logging::log_warn;
use starnix_sync::{AuditQueueLock, AuditSinkLock, LockDepGuard, LockDepMutex};
use starnix_uapi::errors::Errno;
use starnix_uapi::{audit_status, error, pid_t};
use std::collections::VecDeque;
use std::fmt::Display;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::SystemTime;
use zx::MonotonicDuration;

use crate::task::{ArgNameAndValue, CurrentTask, Kernel};
const DEFAULT_BACKLOG_LIMIT: u32 = 128;

/// Supported requests that manipulate the `AuditLogger`
pub enum AuditRequest {
    AuditGet,
    AuditSet,
    AuditUser,
}

impl TryFrom<u32> for AuditRequest {
    type Error = Errno;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            AUDIT_GET => Ok(Self::AuditGet),
            AUDIT_SET => Ok(Self::AuditSet),
            AUDIT_USER
            | AUDIT_FIRST_USER_MSG..=AUDIT_LAST_USER_MSG
            | AUDIT_FIRST_USER_MSG2..=AUDIT_LAST_USER_MSG2 => Ok(Self::AuditUser),
            _ => error!(ENOTSUP),
        }
    }
}

/// Possible modes of the audit framework.
#[derive(PartialEq)]
enum AuditMode {
    Disabled,
    Unspecified,
    Enabled,
}

/// The audit sink reference structure.
#[derive(Default)]
struct AuditNetlinkClientRef {
    /// Inner reference to the registered audit sink, if any.
    client: Option<Arc<AuditNetlinkClient>>,
    /// The PID of the registered audit sink.
    pid: pid_t,
    /// Deque for the audit messages, always present.
    messages: VecDeque<AuditMessage>,
}

/// Audit status structure defining the behaviour of the logger.
struct AuditConfig {
    /// The audit mode set by kernel command line.
    audit_mode: AuditMode,
    /// The maximum number of audit messages that can be stored by the logger.
    backlog_limit: AtomicU32,
    /// Action to take in case of audit failure.
    fail_action: AtomicU8,
    /// Socket to which the logger writes audit messages.
    audit_sink: LockDepMutex<AuditNetlinkClientRef, AuditSinkLock>,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            audit_mode: AuditMode::Unspecified,
            backlog_limit: AtomicU32::new(DEFAULT_BACKLOG_LIMIT),
            fail_action: AtomicU8::new(AUDIT_FAIL_PRINTK as u8),
            audit_sink: Default::default(),
        }
    }
}

impl AuditConfig {
    pub fn new<'a>(cmdline_iter: impl Iterator<Item = ArgNameAndValue<'a>>) -> Self {
        let mut config = Self::default();
        // The logger may be disabled by the kernel command line.
        config.apply_kernel_cmdline(cmdline_iter);
        config
    }

    /// Function to apply the optional kernel command line arguments.
    fn apply_kernel_cmdline<'a>(
        &mut self,
        cmdline_iter: impl Iterator<Item = ArgNameAndValue<'a>>,
    ) {
        for arg in cmdline_iter {
            match arg {
                ArgNameAndValue { name: "audit", value: Some(value) } => match value {
                    "0" | "off" => self.audit_mode = AuditMode::Disabled,
                    // If the audit option is "1"/"on"/anything else, fully enable auditing.
                    _ => self.audit_mode = AuditMode::Enabled,
                },
                ArgNameAndValue { name: "audit_backlog_limit", value: Some(value) } => self
                    .backlog_limit
                    .store(value.parse().unwrap_or(DEFAULT_BACKLOG_LIMIT), Ordering::Release),
                _ => (),
            }
        }
    }
}

/// Audit logging structure.
pub struct AuditLogger {
    /// Audit status structure.
    configuration: AuditConfig,
    /// The number of audit messages lost due to writing errors.
    lost_audit_messages: AtomicU32,
    /// Monotonic counter for audit serial numbers
    serial_counter: AtomicCounter<u64>,
    /// Audit message deque containing (audit type, audit string) up to `backlog_limit` messages.
    /// TODO: https://fxbug.dev/438677236 - confirm single queue behaviour is valid.
    audit_queue: LockDepMutex<VecDeque<AuditMessage>, AuditQueueLock>,
}

impl AuditLogger {
    pub fn new(kernel: &Kernel) -> Self {
        Self {
            configuration: AuditConfig::new(kernel.cmdline_args_iter()),
            lost_audit_messages: Default::default(),
            serial_counter: Default::default(),
            audit_queue: Default::default(),
        }
    }

    pub fn is_disabled(&self) -> bool {
        self.configuration.audit_mode == AuditMode::Disabled
    }

    /// Audit logging function that adds an audit message to the queue.
    ///
    /// The `audit_formatter` function is called only if the auditing is enabled.
    pub fn audit_log<M: Display, T: FnOnce() -> M>(&self, audit_type: u16, audit_formatter: T) {
        if self.configuration.audit_mode == AuditMode::Disabled {
            return;
        }
        self.add_audit_to_backlog(audit_type, audit_formatter);
    }

    /// Called by the `NetlinkAuditClient` to pull the next audit log from the backlog.
    pub fn read_audit_log(&self, client: &Arc<AuditNetlinkClient>) -> Option<AuditMessage> {
        let mut client_guard = self.configuration.audit_sink.lock();
        let Some(current_client) = client_guard.client.as_ref() else {
            return None;
        };
        // Check if the current client is reading the backlog.
        if !Arc::ptr_eq(&current_client, client) {
            return None;
        }
        client_guard.messages.pop_front()
    }

    /// Function to detach the `AuditNetlinkClient` from the `AuditLogger` if
    /// the provided client matches the one registered.
    pub fn detach_client(&self, client: &Arc<AuditNetlinkClient>) {
        let mut client_guard = self.configuration.audit_sink.lock();
        if client_guard
            .client
            .as_ref()
            .is_some_and(|current_client| Arc::ptr_eq(client, &current_client))
        {
            let pid = client_guard.pid;
            client_guard.client = None;
            client_guard.pid = 0;
            client_guard.messages.clear();
            drop(client_guard);
            self.audit_log(AUDIT_CONFIG_CHANGE as u16, || format!("audit sink detached pid={pid}"));
        }
    }

    /// Applies the specified changes to the audit logger settings.
    pub fn set_status(
        &self,
        current_task: &CurrentTask,
        status: audit_status,
        client: &Arc<AuditNetlinkClient>,
    ) -> Result<(), Errno> {
        // Dummy check for enable/disable request. This should be used again if other
        // subsystems will use the audit logger.
        if status.mask & AUDIT_STATUS_ENABLED != 0 && status.enabled > 1 {
            return error!(EINVAL);
        }
        if status.mask & AUDIT_STATUS_BACKLOG_LIMIT != 0 {
            self.configuration.backlog_limit.store(status.backlog_limit, Ordering::Release);
        }
        if status.mask & AUDIT_STATUS_FAILURE != 0 {
            self.configuration.fail_action.store(status.failure as u8, Ordering::Release);
        }
        if status.mask & AUDIT_STATUS_LOST != 0 {
            self.lost_audit_messages.store(0, Ordering::Release);
        }
        if status.mask & AUDIT_STATUS_PID != 0 {
            self.update_client(current_task.get_pid(), status.pid as pid_t, client)?;
        }
        Ok(())
    }

    /// Retrieve the `AuditConfig` as `audit_status` struct.
    pub fn get_status(&self) -> audit_status {
        let pid = self.configuration.audit_sink.lock().pid as u32;
        let backlog = self.audit_queue.lock().len() as u32;
        audit_status {
            mask: Default::default(),
            enabled: Default::default(),
            failure: self.configuration.fail_action.load(Ordering::Acquire) as u32,
            pid,
            rate_limit: u32::MAX,
            backlog_limit: self.configuration.backlog_limit.load(Ordering::Acquire),
            lost: self.lost_audit_messages.load(Ordering::Acquire),
            backlog,
            __bindgen_anon_1: Default::default(),
            backlog_wait_time: Default::default(),
            backlog_wait_time_actual: Default::default(),
        }
    }

    /// Retrieve the number of audit messages in the backlog.
    pub fn get_backlog_count(&self, client: &Arc<AuditNetlinkClient>) -> usize {
        let client_guard = self.configuration.audit_sink.lock();
        if let Some(current_client) = &client_guard.client {
            if Arc::ptr_eq(&current_client, client) {
                return client_guard.messages.len();
            }
        }
        0
    }

    /// Function to update the attached `client` and its PID
    fn update_client(
        &self,
        pid: pid_t,
        request_pid: pid_t,
        client: &Arc<AuditNetlinkClient>,
    ) -> Result<(), Errno> {
        if request_pid == 0 {
            let client_ref = {
                let client_guard = self.configuration.audit_sink.lock();
                // If there is no audit client registered and unregister is requested, return without error.
                if client_guard.pid == 0 {
                    return Ok(());
                } else if pid != client_guard.pid {
                    return error!(EPERM);
                }
                client_guard.client.clone()
            };
            client_ref.inspect(|client_ref| self.detach_client(&client_ref));
            return Ok(());
        }
        if pid != request_pid {
            return error!(EINVAL);
        }

        let mut client_guard = self.configuration.audit_sink.lock();
        if client_guard.client.is_some() {
            return error!(EEXIST);
        }
        client_guard.client = Some(client.clone());
        client_guard.pid = pid;
        drop(client_guard);
        self.audit_log(AUDIT_CONFIG_CHANGE as u16, || format!("new audit sink attached pid={pid}"));
        Ok(())
    }

    /// Add an audit message to the backlog if it is enabled.
    fn add_audit_to_backlog<M: Display, T: FnOnce() -> M>(
        &self,
        audit_type: u16,
        audit_formatter: T,
    ) {
        // At this point, we know that the audit framework is not disabled until reboot.
        let audit_message = self.prepend_audit_metadata(audit_formatter);

        let mut client_guard = self.configuration.audit_sink.lock();
        // If there is no audit sink and the auditing is partially enabled, print and return
        // without pushing the message to the backlog.
        if client_guard.client.is_none() {
            log_warn!("audit: type={audit_type} msg={audit_message}");
        }

        if client_guard.client.is_some() || self.configuration.audit_mode == AuditMode::Enabled {
            self.push_back_audit(audit_type, audit_message, &mut client_guard);
            let client = client_guard.client.clone();
            drop(client_guard);
            if let Some(client) = client {
                client.notify();
            }
        }
    }

    /// Push the audit message in the backlog after checking its limit.
    fn push_back_audit(
        &self,
        audit_type: u16,
        audit_message: String,
        client_guard: &mut LockDepGuard<'_, AuditNetlinkClientRef>,
    ) {
        // TODO: https://fxbug.dev/440090442 - implement backlog waiting.
        if self.check_backlog(client_guard.messages.len() as u32) {
            return;
        }
        client_guard.messages.push_back(AuditMessage { audit_type, message: audit_message.into() });
    }

    /// Function to check the backlog size against the backlog limit.
    /// If the limit is set to 0, ignore the check.
    ///
    /// Return true if the limit is reached, false otherwise.
    fn check_backlog(&self, backlog_size: u32) -> bool {
        let limit = self.configuration.backlog_limit.load(Ordering::Acquire);
        if limit != 0 && backlog_size >= limit {
            let lost = self.lost_audit_messages.fetch_add(1, Ordering::Release) + 1;
            log_warn!("audit_lost={lost} backlog_limit={limit}");
            // If the backlog is full, use failure-to-log action.
            match self.configuration.fail_action.load(Ordering::Acquire) as u32 {
                AUDIT_FAIL_PANIC => panic!("backlog limit exceeded"),
                AUDIT_FAIL_PRINTK => log_warn!("backlog limit exceeded"),
                AUDIT_FAIL_SILENT | _ => (),
            }
            return true;
        }
        false
    }

    /// Function to prepend an audit message with a timestamp and serial number.
    fn prepend_audit_metadata<M: Display, T: FnOnce() -> M>(&self, audit: T) -> String {
        let epoch_time = MonotonicDuration::from(
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default(),
        )
        .into_millis();

        format!(
            "audit({}.{}:{}): {}",
            epoch_time / 1000,
            epoch_time % 1000,
            self.serial_counter.next(),
            audit()
        )
    }
}

/// Audit message structure.
pub struct AuditMessage {
    /// The type of the audit message (e.g., AUDIT_AVC).
    pub audit_type: u16,
    /// The message to be audit-logged.
    pub message: Vec<u8>,
}
