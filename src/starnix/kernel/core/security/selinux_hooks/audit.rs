// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::{CurrentTask, Task};
use crate::vfs::{
    DirEntry, DirEntryHandle, FileObject, FileSystem, FsNode, FsStr, NamespaceNode,
    PathWithReachability,
};
use bstr::BStr;
use fuchsia_rcu::RcuReadScope;
use hex;
use linux_uapi::AUDIT_AVC;
use selinux::permission_check::{PermissionCheck, PermissionCheckResult};
use selinux::{ClassPermission, KernelClass, KernelPermission, SecurityId};
use starnix_logging::CATEGORY_STARNIX_SECURITY;
use starnix_sync::{AuditDenyCountsLock, LockDepMutex};
use std::collections::HashMap;
use std::fmt::{Display, Error};
use std::num::NonZeroU32;
use std::sync::LazyLock;

/// Represents a unique auditable instance, for rate limiting purposes.
#[derive(Clone, Eq, Hash, PartialEq)]
struct AuditableInstance {
    source_sid: SecurityId,
    target_sid: SecurityId,
    class: KernelClass,
    bug: NonZeroU32,
}

/// Stores count of todo_deny logged per auditable instance.
static TODO_DENY_COUNTS: LazyLock<
    LockDepMutex<HashMap<AuditableInstance, u32>, AuditDenyCountsLock>,
> = LazyLock::new(|| Default::default());

/// Checks whether an audit log entry should still be emitted for this audit instance.
fn should_audit(
    source_sid: SecurityId,
    target_sid: SecurityId,
    class: KernelClass,
    bug: NonZeroU32,
) -> bool {
    // Audit-log the first few denials, but skip further denials to avoid logspamming.
    const MAX_TODO_AUDIT_DENIALS: u32 = 5;

    let mut counts = TODO_DENY_COUNTS.lock();
    let count = counts.entry(AuditableInstance { source_sid, target_sid, class, bug }).or_default();
    *count += 1;
    *count <= MAX_TODO_AUDIT_DENIALS
}

/// Container for a reference to kernel state from which to include details when emitting audit
/// logging.  [`Auditable`] instances are created from references to objects via `into()`, e.g:
///
///   fn my_lovely_hook(current_task: &CurrentTask, ...) {
///     let audit_context = current_task.into();
///     check_permission(..., audit_context)
///   }
///
/// Call-sites which need to include context from multiple sources into audit logs can do so by
/// creating an array of [`Auditable`] instances from those sources, and using `into()` to create
/// an [`Auditable`] from a reference to that array, e.g:
///
///   fn my_lovelier_hook(current_task: &CurrentTask,..., audit_context: Auditable<'_>) {
///     let audit_context = [audit_context, current_task.into()];
///     check_permission(..., (&audit_context).into())
///   }
///
/// [`Auditable`] instances are parameterized with the lifetime of the references they contain,
/// which will be automagically derived by Rust. Since they only consist of a type discriminator and
/// reference they are cheap to copy, avoiding the need to pass them by-reference if the same
/// context is to be applied to multiple permission checks.
#[derive(Debug, Clone, Copy)]
pub enum Auditable<'a> {
    // keep-sorted start
    AuditContext(&'a [Auditable<'a>]),
    Bug(u32),
    CurrentTask,
    DirEntry(&'a DirEntry),
    FileObject(&'a FileObject),
    FileSystem(&'a FileSystem),
    FsNode(&'a FsNode),
    IoctlCommand(u16),
    Location(&'a std::panic::Location<'a>),
    Name(&'a FsStr),
    NamespaceNode(&'a NamespaceNode),
    NlMsgtype(u16),
    None,
    SockOptArguments(u32, u32),
    Task(&'a Task),
    // keep-sorted end
}

impl Auditable<'_> {
    fn from_bug(bug_id: u32) -> Self {
        Auditable::Bug(bug_id)
    }
}

impl<'a> From<&'a CurrentTask> for Auditable<'a> {
    fn from(_value: &'a CurrentTask) -> Self {
        // This case is vestigal and will be removed.
        Auditable::CurrentTask
    }
}

impl<'a> From<&'a Task> for Auditable<'a> {
    fn from(value: &'a Task) -> Self {
        Auditable::Task(value)
    }
}

impl<'a> From<&'a DirEntry> for Auditable<'a> {
    fn from(value: &'a DirEntry) -> Self {
        Auditable::DirEntry(value)
    }
}

impl<'a> From<&'a DirEntryHandle> for Auditable<'a> {
    fn from(value: &'a DirEntryHandle) -> Self {
        Auditable::DirEntry(&*value)
    }
}

impl<'a> From<&'a FileObject> for Auditable<'a> {
    fn from(value: &'a FileObject) -> Self {
        Auditable::FileObject(value)
    }
}

impl<'a> From<&'a FsNode> for Auditable<'a> {
    fn from(value: &'a FsNode) -> Self {
        Auditable::FsNode(value)
    }
}

impl<'a> From<&'a FileSystem> for Auditable<'a> {
    fn from(value: &'a FileSystem) -> Self {
        Auditable::FileSystem(value)
    }
}

impl<'a> From<&'a std::panic::Location<'a>> for Auditable<'a> {
    fn from(value: &'a std::panic::Location<'a>) -> Self {
        Auditable::Location(value)
    }
}

impl<'a> From<&'a NamespaceNode> for Auditable<'a> {
    fn from(value: &'a NamespaceNode) -> Self {
        Auditable::NamespaceNode(value)
    }
}

impl<'a, const N: usize> From<&'a [Auditable<'a>; N]> for Auditable<'a> {
    fn from(value: &'a [Auditable<'a>; N]) -> Self {
        Auditable::AuditContext(value)
    }
}

/// Emits an audit log entry with the supplied details. See the SELinux Project's "AVC Audit Events"
/// description (at https://selinuxproject.org/page/NB_AL) for details of the format and fields in
/// audit logs.
///
/// The supplied `permission_check` is used to serialize the `source_sid` and `target_sid` into
/// their string forms.
///
/// If the `result` has a `todo_bug` then the audit entry's decision will be "todo_deny", instead of
/// the standard "granted" or "denied" decisions, to indicate that the check failed, but was granted
/// nonetheless, via the todo-deny exceptions configuration.
///
/// Callers must supply an [`Auditable`] with context for the check (e.g. the calling task, target
/// file object or filesystem node, etc.).
pub(super) fn audit_decision(
    current_task: &CurrentTask,
    permission_check: &PermissionCheck<'_>,
    result: PermissionCheckResult,
    source_sid: SecurityId,
    target_sid: SecurityId,
    permission: KernelPermission,
    audit_data: Auditable<'_>,
) {
    fuchsia_trace::instant!(
        CATEGORY_STARNIX_SECURITY,
        match (result.granted, result.todo_bug) {
            (true, None) => c"audit.granted",
            (true, Some(_)) => c"audit.todo_deny",
            _ => c"audit.denied",
        },
        fuchsia_trace::Scope::Thread
    );

    let decision = if let Some(todo_bug) = result.todo_bug {
        // If `todo_bug` is set then this check is being granted to accommodate errata, rather than
        // the denial being enforced.

        // Re-using the `track_stub!()` internals to track the denial, and determine whether
        // too many denial audit logs have already been emit for this case.
        if !should_audit(source_sid, target_sid, permission.class(), todo_bug) {
            return;
        }

        // The first few of each `todo_bug` are logged as "todo_deny", and the denial tracked.
        "todo_deny"
    } else {
        if result.granted { "granted" } else { "denied" }
    };

    // If there is an associated bug then add it to the audit context.
    let audit_data_with_bug =
        [Auditable::from_bug(result.todo_bug.map(NonZeroU32::get).unwrap_or(0)), audit_data];
    let audit_data =
        if result.todo_bug.is_some() { (&audit_data_with_bug).into() } else { audit_data };

    let audit_logger = current_task.kernel().audit_logger();
    audit_logger.audit_log(
        AUDIT_AVC as u16,
        || {
            let tclass = permission.class().name();
            let permission_name = permission.name();

            // The source and target SIDs are by definition allocated to Security Contexts, so there is no
            // need to handle `sid_to_security_context()` failure.
            let security_server = permission_check.security_server();
            let scontext = security_server.sid_to_security_context(source_sid).unwrap();
            let scontext = BStr::new(&scontext);
            let tcontext = security_server.sid_to_security_context(target_sid).unwrap();
            let tcontext = BStr::new(&tcontext);

            // Gather details about the calling task.
            let pid = current_task.get_pid();
            let command = current_task.command();

            let is_permissive = result.permissive as u8;

            format!("avc: {decision} {{ {permission_name} }} for pid={pid} comm=\"{command}\"{audit_data} scontext={scontext} tcontext={tcontext} tclass={tclass} permissive={is_permissive}")
        }
    );
}

impl Display for Auditable<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), Error> {
        match self {
            Auditable::AuditContext(audit_context) => {
                for item in *audit_context {
                    item.fmt(f)?;
                }
                Ok(())
            }
            Auditable::Bug(bug_id) => {
                write!(f, " bug={}", bug_id)
            }
            Auditable::CurrentTask => Ok(()),
            Auditable::DirEntry(entry) => {
                let scope = RcuReadScope::new();
                write!(f, " name={}", hex_escape(entry.local_name(&scope)))
            }
            Auditable::FileObject(file) => {
                write!(f, " path={}", hex_escape(&file.name.path_escaping_chroot()))
            }
            Auditable::FileSystem(fs) => {
                write!(f, " dev={}", hex_escape(&fs.options.source))
            }
            Auditable::FsNode(node) => {
                write!(f, " ino={}", node.ino)
            }
            Auditable::IoctlCommand(ioctl) => {
                write!(f, " ioctlcmd={:#x}", ioctl)
            }
            Auditable::NlMsgtype(message_type) => {
                write!(f, " nl-msgtype={}", message_type)
            }
            Auditable::Location(location) => {
                write!(f, " caller={:?}", location)
            }
            Auditable::Name(name) => {
                write!(f, " name={}", hex_escape(name))
            }
            Auditable::NamespaceNode(node) => {
                let PathWithReachability::Reachable(path) = node.path_from_root(None) else {
                    return Ok(());
                };
                write!(f, " path={}", hex_escape(&path))
            }
            Auditable::SockOptArguments(level, optname) => {
                write!(f, " level={}, optname={}", level, optname)
            }
            Auditable::None => Ok(()),
            Auditable::Task(task) => {
                write!(f, " pid={}, comm={}", task.get_pid(), task.command())
            }
        }
    }
}

struct EscapedString<'a> {
    value: &'a [u8],
}

impl<'a> Display for EscapedString<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), Error> {
        // SELinux escapes strings containing spaces or control characters, to prevent userspace
        // being able to construct names that confuse audit-log parsing tooling.
        // Additionally enforcing that strings are valid UTF-8 encoded allows non-UTF-8 strings to
        // be expressed losslessy (via hex escaping) rather than being formatted lossily by `bstr`.
        let maybe_utf8 = str::from_utf8(self.value).ok();
        if let Some(utf8) = maybe_utf8 {
            if utf8.find(|c| c <= ' ').is_none() {
                return write!(f, "\"{}\"", BStr::new(self.value));
            }
        }
        hex::encode_upper(self.value).fmt(f)
    }
}

fn hex_escape<'a>(value: &'a [u8]) -> EscapedString<'a> {
    EscapedString { value }
}
