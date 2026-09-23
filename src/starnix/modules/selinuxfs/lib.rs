// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]

use seq_lock::{SeqLock, SeqLockable, WriteSize};

use selinux::policy::parser::PolicyData;
use selinux::policy::{AccessDecision, AccessVector, POLICYDB_VERSION_MAX, PolicyId};
use selinux::{
    ClassId, InitialSid, PolicyCap, SeLinuxStatus, SeLinuxStatusPublisher, SecurityId,
    SecurityPermission, SecurityServer,
};
use starnix_core::device::mem::DevNull;
use starnix_core::mm::memory::MemoryObject;
use starnix_core::security;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::buffers::{InputBuffer, OutputBuffer};
use starnix_core::vfs::pseudo::simple_directory::{SimpleDirectory, SimpleDirectoryMutator};
use starnix_core::vfs::pseudo::simple_file::{
    BytesFile, BytesFileOps, SimpleFileNode, parse_unsigned_file,
};
use starnix_core::vfs::pseudo::vec_directory::{VecDirectory, VecDirectoryEntry};
use starnix_core::vfs::{
    CacheMode, DirEntry, DirectoryEntryType, DirentSink, FileObject, FileOps, FileSystem,
    FileSystemHandle, FileSystemOps, FileSystemOptions, FsNode, FsNodeHandle, FsNodeInfo,
    FsNodeOps, FsStr, FsString, MemoryRegularNode, NamespaceNode, emit_dotdot,
    fileops_impl_directory, fileops_impl_noop_sync, fileops_impl_seekable,
    fileops_impl_unbounded_seek, fs_node_impl_dir_readonly, fs_node_impl_not_dir,
};
use starnix_logging::{
    __track_stub_inner, BugRef, impossible_error, log_error, log_info, track_stub,
};
use starnix_sync::{LockDepMutex, SeLinuxFsContextSidLock};
use starnix_types::vfs::default_statfs;
use starnix_uapi::auth::FsCred;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{AUDIT_AVC, SELINUX_MAGIC, errno, error, statfs};
use std::borrow::Cow;
use std::num::NonZeroU64;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::{Arc, OnceLock, Weak};
use strum::VariantArray as _;
use zerocopy::{Immutable, IntoBytes};

/// The version of the SELinux "status" file this implementation implements.
const SELINUX_STATUS_VERSION: u32 = 1;

/// Header of the C-style struct exposed via the /sys/fs/selinux/status file,
/// to userspace. Defined here (instead of imported through bindgen) as selinux
/// headers are not exposed through  kernel uapi headers.
#[derive(IntoBytes, Copy, Clone, Immutable)]
#[repr(C, align(4))]
struct SeLinuxStatusHeader {
    /// Version number of this structure (1).
    version: u32,
}

impl Default for SeLinuxStatusHeader {
    fn default() -> Self {
        Self { version: SELINUX_STATUS_VERSION }
    }
}

/// Value part of the C-style struct exposed via the /sys/fs/selinux/status file,
/// to userspace. Defined here (instead of imported through bindgen) as selinux
/// headers are not exposed through  kernel uapi headers.
#[derive(IntoBytes, Copy, Clone, Default, Immutable)]
#[repr(C, align(4))]
struct SeLinuxStatusValue {
    /// `0` means permissive mode, `1` means enforcing mode.
    enforcing: u32,
    /// The number of times the selinux policy has been reloaded.
    policyload: u32,
    /// `0` means allow and `1` means deny unknown object classes/permissions.
    deny_unknown: u32,
}

// SAFETY: `SeLinuxStatusValue` can be safely written to shared memory in 4-byte chunks
// because it is composed solely of u32s. It does not include an inline sequence lock.
unsafe impl SeqLockable for SeLinuxStatusValue {
    const WRITE_SIZE: WriteSize = WriteSize::Four;
    const HAS_INLINE_SEQUENCE: bool = false;
    const VMO_NAME: &'static [u8] = b"starnix:selinux";
}

type StatusSeqLock = SeqLock<SeLinuxStatusHeader, SeLinuxStatusValue>;

struct StatusPublisher(StatusSeqLock);

impl StatusPublisher {
    pub fn new_default() -> Result<Self, zx::Status> {
        let seq_lock = StatusSeqLock::new_default()?;
        Ok(StatusPublisher(seq_lock))
    }
}

impl SeLinuxStatusPublisher for StatusPublisher {
    fn set_status(&mut self, policy_status: SeLinuxStatus) {
        self.0.set_value(SeLinuxStatusValue {
            enforcing: policy_status.is_enforcing as u32,
            policyload: policy_status.change_count,
            deny_unknown: policy_status.deny_unknown as u32,
        })
    }
}

struct SeLinuxFs;
impl FileSystemOps for SeLinuxFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(SELINUX_MAGIC))
    }
    fn name(&self) -> &'static FsStr {
        "selinuxfs".into()
    }
}

/// Implements the /sys/fs/selinux filesystem, as documented in the SELinux
/// Notebook at
/// https://github.com/SELinuxProject/selinux-notebook/blob/main/src/lsm_selinux.md#selinux-filesystem
impl SeLinuxFs {
    fn new_fs(
        current_task: &CurrentTask,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        // If SELinux is not enabled then the "selinuxfs" file system does not exist.
        let security_server = security::selinuxfs_get_admin_api(current_task)
            .ok_or_else(|| errno!(ENODEV, "selinuxfs"))?;

        let kernel = current_task.kernel();
        let fs = FileSystem::new(kernel, CacheMode::Permanent, SeLinuxFs, options)?;
        let root = SimpleDirectory::new();
        fs.create_root(fs.allocate_ino(), root.clone());
        let dir = SimpleDirectoryMutator::new(fs.clone(), root);

        // Read-only files & directories, exposing SELinux internal state.
        dir.subdir("avc", 0o555, |dir| {
            dir.entry(
                "cache_stats",
                AvcCacheStatsFile::new_node(security_server.clone()),
                mode!(IFREG, 0o444),
            );
        });
        dir.entry("checkreqprot", CheckReqProtApi::new_node(), mode!(IFREG, 0o644));
        dir.entry("class", ClassDirectory::new(security_server.clone()), mode!(IFDIR, 0o555));
        dir.entry(
            "deny_unknown",
            DenyUnknownFile::new_node(security_server.clone()),
            mode!(IFREG, 0o444),
        );
        dir.entry(
            "reject_unknown",
            RejectUnknownFile::new_node(security_server.clone()),
            mode!(IFREG, 0o444),
        );
        dir.subdir("initial_contexts", 0o555, |dir| {
            for initial_sid in InitialSid::all_variants() {
                dir.entry(
                    initial_sid.name(),
                    InitialContextFile::new_node(security_server.clone(), *initial_sid),
                    mode!(IFREG, 0o444),
                );
            }
        });
        dir.entry("mls", BytesFile::new_node(b"1".to_vec()), mode!(IFREG, 0o444));
        dir.entry("policy", PolicyFile::new_node(security_server.clone()), mode!(IFREG, 0o600));
        dir.subdir("policy_capabilities", 0o555, |dir| {
            for capability in PolicyCap::VARIANTS {
                dir.entry(
                    capability.name(),
                    PolicyCapFile::new_node(security_server.clone(), *capability),
                    mode!(IFREG, 0o444),
                );
            }
        });
        dir.entry(
            "policyvers",
            BytesFile::new_node(format!("{}", POLICYDB_VERSION_MAX).into_bytes()),
            mode!(IFREG, 0o444),
        );

        // The status file needs to be mmap-able, so use a VMO-backed file. When the selinux state
        // changes in the future, the way to update this data (and communicate updates with
        // userspace) is to use the ["seqlock"](https://en.wikipedia.org/wiki/Seqlock) technique.
        let status_holder = StatusPublisher::new_default().expect("selinuxfs status seqlock");
        let status_file = status_holder
            .0
            .get_readonly_vmo()
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .map_err(impossible_error)?;
        dir.entry(
            "status",
            MemoryRegularNode::from_memory(Arc::new(MemoryObject::from(status_file))),
            mode!(IFREG, 0o444),
        );
        security_server.set_status_publisher(Box::new(status_holder));

        // Write-only files used to configure and query SELinux.
        dir.entry(
            "access",
            AccessApi::new_node(security_server.clone(), current_task.kernel()),
            mode!(IFREG, 0o666),
        );
        dir.entry("context", ContextApi::new_node(security_server.clone()), mode!(IFREG, 0o666));
        dir.entry("create", CreateApi::new_node(security_server.clone()), mode!(IFREG, 0o666));
        dir.entry("member", MemberApi::new_node(), mode!(IFREG, 0o666));
        dir.entry("relabel", RelabelApi::new_node(), mode!(IFREG, 0o666));
        dir.entry("user", UserApi::new_node(), mode!(IFREG, 0o666));
        dir.entry("load", LoadApi::new_node(security_server.clone()), mode!(IFREG, 0o600));
        dir.entry(
            "commit_pending_bools",
            CommitBooleansApi::new_node(security_server.clone()),
            mode!(IFREG, 0o200),
        );

        // Read/write files allowing values to be queried or changed.
        dir.entry("booleans", BooleansDirectory::new(security_server.clone()), mode!(IFDIR, 0o555));
        // TODO(b/297313229): Get mode from the container.
        dir.entry("enforce", EnforceApi::new_node(security_server), mode!(IFREG, 0o644));

        // "/dev/null" equivalent used for file descriptors redirected by SELinux.
        let null_ops: Box<dyn FsNodeOps> = (NullFileNode).into();
        let mut info = FsNodeInfo::new(mode!(IFCHR, 0o666), FsCred::root());
        info.rdev = DeviceId::NULL;
        let null_fs_node = fs.create_node_and_allocate_node_id(null_ops, info);
        dir.node("null".into(), null_fs_node.clone());

        // Initialize selinux kernel state to store a copy of "/sys/fs/selinux/null" for use in
        // hooks that redirect file descriptors to null. This has the side-effect of applying the
        // policy-defined "devnull" SID to the `null_fs_node`.
        let null_ops: Box<dyn FileOps> = Box::new(DevNull);
        let null_flags = OpenFlags::empty();
        let null_name =
            NamespaceNode::new_anonymous(DirEntry::new(null_fs_node, None, "null".into()));
        let null_file_object = FileObject::new(current_task, null_ops, null_name, null_flags)
            .expect("create file object for just-created selinuxfs/null");
        security::selinuxfs_init_null(current_task, &null_file_object);

        Ok(fs)
    }
}

/// "load" API, accepting a binary policy in a single `write()` operation, which must be at seek
/// position zero.
struct LoadApi {
    security_server: Arc<SecurityServer>,
}

impl LoadApi {
    fn new_node(security_server: Arc<SecurityServer>) -> impl FsNodeOps {
        SeLinuxApi::new_node(move || Ok(Self { security_server: security_server.clone() }))
    }
}

impl SeLinuxApiOps for LoadApi {
    fn api_write_permission() -> SecurityPermission {
        SecurityPermission::LoadPolicy
    }
    fn api_write_with_task(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        log_info!("Loading {} byte policy", data.len());
        self.security_server.load_policy(data).map_err(|error| {
            log_error!("Policy load error: {}", error);
            errno!(EINVAL)
        })?;

        // Allow one-time initialization of state that requires a loaded policy.
        security::selinuxfs_policy_loaded(current_task);

        Ok(())
    }
}

/// "policy" file, which allows the currently-loaded binary policy, to be read as a normal file,
/// including supporting seek-aware reads.
struct PolicyFile {
    binary_policy: Option<PolicyData>,
}

impl PolicyFile {
    fn new_node(security_server: Arc<SecurityServer>) -> impl FsNodeOps {
        SimpleFileNode::new(move |_| {
            Ok(Self { binary_policy: security_server.get_binary_policy() })
        })
    }
}

impl FileOps for PolicyFile {
    fileops_impl_seekable!();
    fileops_impl_noop_sync!();

    fn open(&self, _file: &FileObject, current_task: &CurrentTask) -> Result<(), Errno> {
        security::selinuxfs_check_access(current_task, SecurityPermission::ReadPolicy)?;
        Ok(())
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        let policy = self.binary_policy.as_ref().ok_or_else(|| errno!(EINVAL))?;
        let policy_bytes: &[u8] = policy.deref();

        if offset >= policy_bytes.len() {
            return Ok(0);
        }

        data.write(&policy_bytes[offset..])
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        error!(EACCES)
    }
}

/// "enforce" API used to control whether SELinux is globally permissive, versus enforcing.
struct EnforceApi {
    security_server: Arc<SecurityServer>,
}

impl EnforceApi {
    fn new_node(security_server: Arc<SecurityServer>) -> impl FsNodeOps {
        SeLinuxApi::new_node(move || Ok(Self { security_server: security_server.clone() }))
    }
}

impl SeLinuxApiOps for EnforceApi {
    fn api_write_permission() -> SecurityPermission {
        SecurityPermission::SetEnforce
    }

    fn api_write(&self, data: Vec<u8>) -> Result<(), Errno> {
        // Callers may write any number of times to this API, so long as the `data` is valid.
        let enforce = parse_unsigned_file::<u32>(&data)? != 0;
        self.security_server.set_enforcing(enforce);
        Ok(())
    }

    fn api_read(&self) -> Result<Cow<'_, [u8]>, Errno> {
        Ok(self.security_server.is_enforcing().then_some(b"1").unwrap_or(b"0").into())
    }
}

/// "deny_unknown" file which exposes how classes & permissions not defined by the policy should
/// be allowed or denied.
struct DenyUnknownFile {
    security_server: Arc<SecurityServer>,
}

impl DenyUnknownFile {
    fn new_node(security_server: Arc<SecurityServer>) -> impl FsNodeOps {
        BytesFile::new_node(Self { security_server })
    }
}

impl BytesFileOps for DenyUnknownFile {
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        Ok(format!("{}", self.security_server.deny_unknown() as u32).into_bytes().into())
    }
}

/// "reject_unknown" file which exposes whether kernel classes & permissions not defined by the
/// policy would have prevented the policy being loaded.
struct RejectUnknownFile {
    security_server: Arc<SecurityServer>,
}

impl RejectUnknownFile {
    fn new_node(security_server: Arc<SecurityServer>) -> impl FsNodeOps {
        BytesFile::new_node(Self { security_server })
    }
}

impl BytesFileOps for RejectUnknownFile {
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        Ok(format!("{}", self.security_server.reject_unknown() as u32).into_bytes().into())
    }
}

/// "create" API used to determine the Security Context to associate with a new resource instance
/// based on source, target, and target class.
struct CreateApi {
    security_server: Arc<SecurityServer>,
    result: OnceLock<SecurityId>,
}

impl CreateApi {
    fn new_node(security_server: Arc<SecurityServer>) -> impl FsNodeOps {
        SeLinuxApi::new_node(move || {
            Ok(Self { security_server: security_server.clone(), result: OnceLock::new() })
        })
    }
}

impl SeLinuxApiOps for CreateApi {
    fn api_write_permission() -> SecurityPermission {
        SecurityPermission::ComputeCreate
    }

    fn api_write(&self, data: Vec<u8>) -> Result<(), Errno> {
        if self.result.get().is_some() {
            // The "create" API can be written-to at most once.
            return error!(EBUSY);
        }

        let data = str::from_utf8(&data).map_err(|_| errno!(EINVAL))?;

        // Requests consist of three mandatory space-separated elements.
        let mut parts = data.split_whitespace();

        // <scontext>: describes the subject that is creating the new object.
        let scontext = parts.next().ok_or_else(|| errno!(EINVAL))?;
        let scontext = self
            .security_server
            .security_context_to_sid(scontext.into())
            .map_err(|_| errno!(EINVAL))?;

        // <tcontext>: describes the target (e.g. parent directory) of the create operation.
        let tcontext = parts.next().ok_or_else(|| errno!(EINVAL))?;
        let tcontext = self
            .security_server
            .security_context_to_sid(tcontext.into())
            .map_err(|_| errno!(EINVAL))?;

        // <tclass>: the policy-specific Id of the created object's class, as a decimal integer.
        // Class Ids are obtained via lookups in the SELinuxFS "class" directory.
        let tclass = parts.next().ok_or_else(|| errno!(EINVAL))?;
        let tclass = u32::from_str(tclass).map_err(|_| errno!(EINVAL))?;
        let tclass = ClassId::from_u32(tclass).ok_or_else(|| errno!(EINVAL))?;

        // Optional <name>: the final element of the path of the newly-created object. This allows
        // filename-dependent transition rules to be applied to the computation.
        let tname = parts.next().map(str::as_bytes).unwrap_or(&[]);

        // There must be no further trailing arguments.
        if parts.next().is_some() {
            return error!(EINVAL);
        }

        let result = self
            .security_server
            .compute_create_sid_raw(scontext, tcontext, tclass, tname)
            .map_err(|_| errno!(EINVAL))?;
        self.result.set(result).map_err(|_| errno!(EINVAL))?;

        Ok(())
    }

    fn api_read(&self) -> Result<Cow<'_, [u8]>, Errno> {
        let maybe_context = self
            .result
            .get()
            .map(|sid| self.security_server.sid_to_security_context_with_nul(*sid).unwrap());
        let context = maybe_context.unwrap_or_else(|| Vec::new());
        Ok(context.into())
    }
}

/// "member" API used to determine the Security Context to associate with a new resource instance
/// based on source, target, and target class and `type_member` rules.
struct MemberApi;

impl MemberApi {
    fn new_node() -> impl FsNodeOps {
        SeLinuxApi::new_node(|| Ok(Self {}))
    }
}

impl SeLinuxApiOps for MemberApi {
    fn api_write_permission() -> SecurityPermission {
        SecurityPermission::ComputeMember
    }
    fn api_write(&self, _data: Vec<u8>) -> Result<(), Errno> {
        track_stub!(TODO("https://fxbug.dev/399069170"), "selinux member");
        error!(ENOTSUP)
    }
    fn api_read(&self) -> Result<Cow<'_, [u8]>, Errno> {
        error!(ENOTSUP)
    }
}

/// "relabel" API used to determine the Security Context to associate with a new resource instance
/// based on source, target, and target class and `type_change` rules.
struct RelabelApi;

impl RelabelApi {
    fn new_node() -> impl FsNodeOps {
        SeLinuxApi::new_node(|| Ok(Self {}))
    }
}

impl SeLinuxApiOps for RelabelApi {
    fn api_write_permission() -> SecurityPermission {
        SecurityPermission::ComputeRelabel
    }
    fn api_write(&self, _data: Vec<u8>) -> Result<(), Errno> {
        track_stub!(TODO("https://fxbug.dev/399069766"), "selinux relabel");
        error!(ENOTSUP)
    }
    fn api_read(&self) -> Result<Cow<'_, [u8]>, Errno> {
        error!(ENOTSUP)
    }
}

/// "user" API used to perform a user decision.
struct UserApi;

impl UserApi {
    fn new_node() -> impl FsNodeOps {
        SeLinuxApi::new_node(|| Ok(Self {}))
    }
}

impl SeLinuxApiOps for UserApi {
    fn api_write_permission() -> SecurityPermission {
        SecurityPermission::ComputeUser
    }
    fn api_write(&self, _data: Vec<u8>) -> Result<(), Errno> {
        track_stub!(TODO("https://fxbug.dev/411433214"), "selinux user");
        error!(ENOTSUP)
    }
    fn api_read(&self) -> Result<Cow<'_, [u8]>, Errno> {
        error!(ENOTSUP)
    }
}

struct CheckReqProtApi;

impl CheckReqProtApi {
    fn new_node() -> impl FsNodeOps {
        SeLinuxApi::new_node(|| Ok(Self {}))
    }
}

impl SeLinuxApiOps for CheckReqProtApi {
    fn api_write_permission() -> SecurityPermission {
        SecurityPermission::SetCheckReqProt
    }

    fn api_write(&self, _data: Vec<u8>) -> Result<(), Errno> {
        // Linux v6.4 removed support for enabling "checkreqprot", rendering writes to the node a
        // no-op.
        error!(ENOTSUP)
    }

    fn api_read(&self) -> Result<Cow<'_, [u8]>, Errno> {
        Ok(b"0".into())
    }
}

/// "context" API which accepts a Security Context in a single `write()` operation, and validates
/// it against the loaded policy. If the Context is invalid then the `write()` returns `EINVAL`,
/// otherwise the Context may be read back from the file.
struct ContextApi {
    security_server: Arc<SecurityServer>,
    // Holds the SID representing the Security Context that the caller wrote to the file.
    context_sid: LockDepMutex<Option<SecurityId>, SeLinuxFsContextSidLock>,
}

impl ContextApi {
    fn new_node(security_server: Arc<SecurityServer>) -> impl FsNodeOps {
        SeLinuxApi::new_node(move || {
            Ok(Self { security_server: security_server.clone(), context_sid: Default::default() })
        })
    }
}

impl SeLinuxApiOps for ContextApi {
    fn api_write_permission() -> SecurityPermission {
        SecurityPermission::CheckContext
    }

    fn api_write(&self, data: Vec<u8>) -> Result<(), Errno> {
        // If this instance was already written-to then fail the operation.
        let mut context_sid = self.context_sid.lock();
        if context_sid.is_some() {
            return error!(EBUSY);
        }

        // Validate that the `data` describe valid user, role, type, etc by attempting to create
        // a SID from it.
        *context_sid = Some(
            self.security_server
                .security_context_to_sid(data.as_slice().into())
                .map_err(|_| errno!(EINVAL))?,
        );

        Ok(())
    }

    fn api_read(&self) -> Result<Cow<'_, [u8]>, Errno> {
        // Read returns the Security Context the caller previously wrote to the file, normalized
        // as a consequence of the Context->SID->Context round-trip. If no Context had been written
        // by the caller, then this API file behaves as though empty.
        // TODO: https://fxbug.dev/319629153 - If `write()` failed due to an invalid Context then
        // should `read()` also fail, or return an empty result?
        let maybe_sid = *self.context_sid.lock();
        let result = maybe_sid
            .and_then(|sid| self.security_server.sid_to_security_context_with_nul(sid))
            .unwrap_or_default();
        Ok(result.into())
    }
}

/// Implements an entry within the "initial_contexts" directory.
struct InitialContextFile {
    security_server: Arc<SecurityServer>,
    initial_sid: InitialSid,
}

impl InitialContextFile {
    fn new_node(security_server: Arc<SecurityServer>, initial_sid: InitialSid) -> impl FsNodeOps {
        BytesFile::new_node(Self { security_server, initial_sid })
    }
}

impl BytesFileOps for InitialContextFile {
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let sid = self.initial_sid.into();
        if let Some(context) = self.security_server.sid_to_security_context_with_nul(sid) {
            Ok(context.into())
        } else {
            // Looking up an initial SID can only fail if no policy is loaded, in
            // which case the file contains the name of the initial SID, rather
            // than a Security Context value.
            Ok(self.initial_sid.name().as_bytes().into())
        }
    }
}

/// An entry in the "policy_capabilities" directory. There is one entry for each policy capability
/// supported by the kernel implementation, with the content indicating whether the capability is
/// enabled or disabled by the loaded policy.
struct PolicyCapFile {
    security_server: Arc<SecurityServer>,
    policy_cap: PolicyCap,
}

impl PolicyCapFile {
    fn new_node(security_server: Arc<SecurityServer>, initial_sid: PolicyCap) -> impl FsNodeOps {
        BytesFile::new_node(Self { security_server, policy_cap: initial_sid })
    }
}

impl BytesFileOps for PolicyCapFile {
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        if self.security_server.is_policycap_enabled(self.policy_cap) {
            Ok(b"1".into())
        } else {
            Ok(b"0".into())
        }
    }
}

/// Extends a calculated `AccessDecision` with an additional permission set describing which
/// permissions were actually `decided` - all other permissions in the `AccessDecision` structure
/// should be assumed to be un-`decided`. This allows the "access" API to return partial results, to
/// force userspace to re-query the API if any un-`decided` permission is later requested.
struct AccessDecisionAndDecided {
    decision: AccessDecision,
    decided: AccessVector,
}

struct AccessApi {
    security_server: Arc<SecurityServer>,
    result: OnceLock<AccessDecisionAndDecided>,

    // Required to support audit-logging of requests granted via `todo_deny` exceptions.
    kernel: Weak<Kernel>,
}

impl AccessApi {
    fn new_node(security_server: Arc<SecurityServer>, kernel: &Arc<Kernel>) -> impl FsNodeOps {
        let kernel = Arc::downgrade(kernel);
        SeLinuxApi::new_node(move || {
            Ok(Self {
                security_server: security_server.clone(),
                result: OnceLock::default(),
                kernel: kernel.clone(),
            })
        })
    }
}

impl SeLinuxApiOps for AccessApi {
    fn api_write_permission() -> SecurityPermission {
        SecurityPermission::ComputeAv
    }

    fn api_write(&self, data: Vec<u8>) -> Result<(), Errno> {
        if self.result.get().is_some() {
            // The "access" API can be written-to at most once.
            return error!(EBUSY);
        }

        let data = str::from_utf8(&data).map_err(|_| errno!(EINVAL))?;

        // Requests consist of three mandatory space-separated elements, and one optional element.
        let mut parts = data.split_whitespace();

        // <scontext>: describes the subject acting on the class.
        let scontext_str = parts.next().ok_or_else(|| errno!(EINVAL))?;
        let scontext = self
            .security_server
            .security_context_to_sid(scontext_str.into())
            .map_err(|_| errno!(EINVAL))?;

        // <tcontext>: describes the target (e.g. parent directory) of the operation.
        let tcontext_str = parts.next().ok_or_else(|| errno!(EINVAL))?;
        let tcontext = self
            .security_server
            .security_context_to_sid(tcontext_str.into())
            .map_err(|_| errno!(EINVAL))?;

        // <tclass>: the policy-specific Id of the target class, as a decimal integer.
        // Class Ids are obtained via lookups in the SELinuxFS "class" directory.
        let tclass = parts.next().ok_or_else(|| errno!(EINVAL))?;
        let tclass_id = u32::from_str(tclass).map_err(|_| errno!(EINVAL))?;
        let tclass = ClassId::from_u32(tclass_id).ok_or_else(|| errno!(EINVAL))?.into();

        // <request>: the set of permissions that the caller requests.
        let requested = if let Some(requested) = parts.next() {
            AccessVector::from_str(requested).map_err(|_| errno!(EINVAL))?
        } else {
            AccessVector::ALL
        };

        // This API does not appear to treat trailing arguments as invalid.

        // Perform the access decision calculation.
        let mut decision =
            self.security_server.compute_access_decision_raw(scontext, tcontext, tclass);

        // `compute_access_decision()` returns an `AccessDecision` with results calculated for all
        // permissions defined by policy, so by default the "access" API reports all permissions as
        // having been `decided`.
        let mut decided = AccessVector::ALL;

        // If there is a `todo_bug` associated with the decision then grant all permissions and
        // make a best-effort attempt to emit a log for missing permissions.
        let Some(kernel) = self.kernel.upgrade() else {
            return error!(EINVAL);
        };
        if let Some(todo_bug) = decision.todo_bug {
            let denied = AccessVector::ALL - decision.allow;
            let audited_denied = denied & decision.auditdeny;

            let requested_has_audited_denial = audited_denied & requested != AccessVector::NONE;

            if requested_has_audited_denial {
                // One or more requested permissions would be denied, and the denial audit-logged,
                // so emit a track-stub report and a description of the request and result.
                // Leave all permissions `decided`, so that only the first such failure is audited.
                __track_stub_inner(
                    BugRef::from(NonZeroU64::new(todo_bug.get() as u64).unwrap()),
                    "Enforce SELinuxFS access API",
                    None,
                    std::panic::Location::caller(),
                );
                let audit_message = format!(
                    "avc: todo_deny {{ ACCESS_API }} bug={todo_bug} scontext={scontext_str:?} tcontext={tcontext_str:?} tclass={tclass_id} requested={requested:?}",
                );
                kernel.audit_logger().audit_log(AUDIT_AVC as u16, || audit_message);
            } else {
                // All requested permissions were granted. To allow "todo_deny" logs and track-stub
                // tracking of permissions that would otherwise be denied & audited, remove those
                // permissions from the `decided` set, to signal that the userspace AVC should re-
                // query rather than using these cached results.
                decided -= audited_denied;
            }

            // Grant all permissions in the returned result, so that clients that ignore the
            // `decided` set will still have the allowance applied to all permissions.
            decision.allow = AccessVector::ALL;
        }

        self.result
            .set(AccessDecisionAndDecided { decision, decided })
            .map_err(|_| errno!(EINVAL))?;

        Ok(())
    }

    fn api_read(&self) -> Result<Cow<'_, [u8]>, Errno> {
        let Some(AccessDecisionAndDecided { decision, decided }) = self.result.get() else {
            return Ok(Vec::new().into());
        };

        let allowed = decision.allow;
        let auditallow = decision.auditallow;
        let auditdeny = decision.auditdeny;
        let flags = decision.flags;

        // TODO: https://fxbug.dev/361551536 - `seqno` should reflect the policy revision from
        // which the result was calculated, to allow the client to re-try if the policy changed.
        const SEQNO: u32 = 1;

        // Result format is: allowed decided auditallow auditdeny seqno flags
        // Everything but seqno must be in hexadecimal format and represents a bits field.
        let result =
            format!("{allowed:x} {decided:x} {auditallow:x} {auditdeny:x} {SEQNO} {flags:x}");
        Ok(result.into_bytes().into())
    }
}

struct NullFileNode;

impl FsNodeOps for NullFileNode {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(DevNull))
    }
}

#[derive(Clone)]
struct BooleansDirectory {
    security_server: Arc<SecurityServer>,
}

impl BooleansDirectory {
    fn new(security_server: Arc<SecurityServer>) -> Self {
        Self { security_server }
    }
}

impl FsNodeOps for BooleansDirectory {
    fs_node_impl_dir_readonly!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(self.clone()))
    }

    fn lookup(
        &self,
        node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        let utf8_name = String::from_utf8(name.to_vec()).map_err(|_| errno!(ENOENT))?;
        if self.security_server.conditional_booleans().contains(&utf8_name) {
            Ok(node.fs().create_node_and_allocate_node_id(
                BooleanFile::new_node(self.security_server.clone(), utf8_name),
                FsNodeInfo::new(mode!(IFREG, 0o644), current_task.current_fscred()),
            ))
        } else {
            error!(ENOENT)
        }
    }
}

impl FileOps for BooleansDirectory {
    fileops_impl_directory!();
    fileops_impl_noop_sync!();
    fileops_impl_unbounded_seek!();

    fn readdir(
        &self,
        file: &FileObject,
        _current_task: &CurrentTask,
        sink: &mut dyn DirentSink,
    ) -> Result<(), Errno> {
        emit_dotdot(file, sink)?;

        // `emit_dotdot()` provides the first two directory entries, so that the entries for
        // the conditional booleans start from offset 2.
        let iter_offset = sink.offset() - 2;
        for name in self.security_server.conditional_booleans().iter().skip(iter_offset as usize) {
            sink.add(
                file.fs.allocate_ino(),
                /* next offset = */ sink.offset() + 1,
                DirectoryEntryType::REG,
                FsString::from(name.as_bytes()).as_ref(),
            )?;
        }

        Ok(())
    }
}

struct BooleanFile {
    security_server: Arc<SecurityServer>,
    name: String,
}

impl BooleanFile {
    fn new_node(security_server: Arc<SecurityServer>, name: String) -> impl FsNodeOps {
        BytesFile::new_node(BooleanFile { security_server, name })
    }
}

impl BytesFileOps for BooleanFile {
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let value = parse_unsigned_file::<u32>(&data)? != 0;
        self.security_server.set_pending_boolean(&self.name, value).map_err(|_| errno!(EIO))
    }

    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        // Each boolean has a current active value, and a pending value that
        // will become active if "commit_pending_booleans" is written to.
        // e.g. "1 0" will be read if a boolean is True but will become False.
        let (active, pending) =
            self.security_server.get_boolean(&self.name).map_err(|_| errno!(EIO))?;
        Ok(format!("{} {}", active as u32, pending as u32).into_bytes().into())
    }
}

struct CommitBooleansApi {
    security_server: Arc<SecurityServer>,
}

impl CommitBooleansApi {
    fn new_node(security_server: Arc<SecurityServer>) -> impl FsNodeOps {
        SeLinuxApi::new_node(move || {
            Ok(CommitBooleansApi { security_server: security_server.clone() })
        })
    }
}

impl SeLinuxApiOps for CommitBooleansApi {
    fn api_write_permission() -> SecurityPermission {
        SecurityPermission::SetBool
    }

    fn api_write(&self, data: Vec<u8>) -> Result<(), Errno> {
        // "commit_pending_booleans" expects a numeric argument, which is
        // interpreted as a boolean, with the pending booleans committed if the
        // value is true (i.e. non-zero).
        let commit = parse_unsigned_file::<u32>(&data)? != 0;

        if commit {
            self.security_server.commit_pending_booleans();
        }
        Ok(())
    }
}

struct ClassDirectory {
    security_server: Arc<SecurityServer>,
}

impl ClassDirectory {
    fn new(security_server: Arc<SecurityServer>) -> Self {
        Self { security_server }
    }
}

impl FsNodeOps for ClassDirectory {
    fs_node_impl_dir_readonly!();

    /// Returns the set of classes under the "class" directory.
    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(VecDirectory::new_file(
            self.security_server
                .class_names()
                .map_err(|_| errno!(ENOENT))?
                .iter()
                .map(|class_name| VecDirectoryEntry {
                    entry_type: DirectoryEntryType::DIR,
                    name: class_name.clone().into(),
                    inode: None,
                })
                .collect(),
        ))
    }

    fn lookup(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        let id: u32 = self
            .security_server
            .class_id_by_name(&name.to_string())
            .map_err(|_| errno!(EINVAL))?
            .into();

        let fs = node.fs();
        let dir = SimpleDirectory::new();
        dir.edit(&fs, |dir| {
            let index_bytes = format!("{}", id).into_bytes();
            dir.entry("index", BytesFile::new_node(index_bytes), mode!(IFREG, 0o444));
            dir.entry(
                "perms",
                PermsDirectory::new(self.security_server.clone(), name.to_string()),
                mode!(IFDIR, 0o555),
            );
        });
        Ok(dir.into_node(&fs, 0o555))
    }
}

/// Represents the perms/ directory under each class entry of the SeLinuxClassDirectory.
struct PermsDirectory {
    security_server: Arc<SecurityServer>,
    class_name: String,
}

impl PermsDirectory {
    fn new(security_server: Arc<SecurityServer>, class_name: String) -> Self {
        Self { security_server, class_name }
    }
}

impl FsNodeOps for PermsDirectory {
    fs_node_impl_dir_readonly!();

    /// Lists all available permissions for the corresponding class.
    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(VecDirectory::new_file(
            self.security_server
                .class_permissions_by_name(&self.class_name)
                .map_err(|_| errno!(ENOENT))?
                .iter()
                .map(|(_permission_id, permission_name)| VecDirectoryEntry {
                    entry_type: DirectoryEntryType::DIR,
                    name: permission_name.clone().into(),
                    inode: None,
                })
                .collect(),
        ))
    }

    fn lookup(
        &self,
        node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        let found_permission_id = self
            .security_server
            .class_permissions_by_name(&(self.class_name))
            .map_err(|_| errno!(ENOENT))?
            .iter()
            .find(|(_permission_id, permission_name)| permission_name == name)
            .ok_or_else(|| errno!(ENOENT))?
            .0;

        Ok(node.fs().create_node_and_allocate_node_id(
            BytesFile::new_node(format!("{}", found_permission_id).into_bytes()),
            FsNodeInfo::new(mode!(IFREG, 0o444), current_task.current_fscred()),
        ))
    }
}

/// Exposes AVC cache statistics from the SELinux security server to userspace.
struct AvcCacheStatsFile {
    security_server: Arc<SecurityServer>,
}

impl AvcCacheStatsFile {
    fn new_node(security_server: Arc<SecurityServer>) -> impl FsNodeOps {
        BytesFile::new_node(Self { security_server })
    }
}

impl BytesFileOps for AvcCacheStatsFile {
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let stats = self.security_server.avc_cache_stats();
        Ok(format!(
            "lookups hits misses allocations reclaims frees\n{} {} {} {} {} {}\n",
            stats.lookups, stats.hits, stats.misses, stats.allocs, stats.reclaims, stats.frees
        )
        .into_bytes()
        .into())
    }
}

/// File node implementation tailored to the behaviour of the APIs exposed to userspace via the
/// SELinux filesystem. These API files share some unusual behaviours:
///
/// (1) Seek Position:
/// API files in the SELinux filesystem do have persistent seek offsets, but asymmetric behaviour
/// for read and write operations:
/// - Read operations respect the file offset, and increment it.
/// - Write operations do not increment the file offset. This is important for APIs such as
///   "create", which are used by `write()`ing a query and then `read()`ing the resulting value,
///   since otherwise the `read()` would start from the end of the `write()`.
///
/// API files do not handle non-zero offset `write()`s consistently. Some, (e.g. "context"), ignore
/// the offset, while others (e.g. "load") will fail with `EINVAL` if it is non-zero.
///
/// (2) Single vs Multi-Request:
/// Most API files may be `read()` from any number of times, but only support a single `write()`
/// operation. Attempting to `write()` a second time will return `EBUSY`.
///
/// (3) Error Handling:
/// Once an operation on an API file has failed, all subsequent operations on that file will
/// also fail, with the same error code.  e.g. Attempting multiple `write()` operations will
/// return `EBUSY` from the second and subsequent calls, but subsequent calls to `read()`,
/// `seek()` etc will also return `EBUSY`.
///
/// This helper currently implements asymmetric seek behaviour, and permission checks on write
/// operations.
struct SeLinuxApi<T: SeLinuxApiOps + Sync + Send + 'static> {
    ops: T,
}

impl<T: SeLinuxApiOps + Sync + Send + 'static> SeLinuxApi<T> {
    /// Returns a new `SeLinuxApi` file node that will use `create_ops` to create a new `SeLinuxApiOps`
    /// instance every time a caller opens the file.
    fn new_node<F>(create_ops: F) -> impl FsNodeOps
    where
        F: Fn() -> Result<T, Errno> + Send + Sync + 'static,
    {
        SimpleFileNode::new(move |_| create_ops().map(|ops| SeLinuxApi { ops }))
    }
}

/// Trait implemented for each SELinux API file (e.g. "create", "load") to define its behaviour.
trait SeLinuxApiOps {
    /// Returns the "security" class permission that is required in order to write to the API file.
    fn api_write_permission() -> SecurityPermission;

    /// Returns true if writes ignore the seek offset, rather than requiring it to be zero.
    fn api_write_ignores_offset() -> bool {
        false
    }

    /// Processes a request written to an API file.
    fn api_write(&self, _data: Vec<u8>) -> Result<(), Errno> {
        error!(EINVAL)
    }

    /// Returns the complete contents of this API file.
    fn api_read(&self) -> Result<Cow<'_, [u8]>, Errno> {
        error!(EINVAL)
    }

    /// Variant of `api_write()` that additionally receives the `current_task`.
    fn api_write_with_task(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        self.api_write(data)
    }
}

impl<T: SeLinuxApiOps + Sync + Send + 'static> FileOps for SeLinuxApi<T> {
    fileops_impl_seekable!();
    fileops_impl_noop_sync!();

    fn writes_update_seek_offset(&self) -> bool {
        false
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        let response = self.ops.api_read()?;
        data.write(&response[offset..])
    }

    fn write(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        if offset != 0 && !T::api_write_ignores_offset() {
            return error!(EINVAL);
        }
        security::selinuxfs_check_access(current_task, T::api_write_permission())?;
        let data = data.read_all()?;
        let data_len = data.len();
        self.ops.api_write_with_task(current_task, data)?;
        Ok(data_len)
    }
}

/// Returns the "selinuxfs" file system, used by the system userspace to administer SELinux.
pub fn selinux_fs(
    current_task: &CurrentTask,
    options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    struct SeLinuxFsHandle(FileSystemHandle);

    Ok(current_task
        .kernel()
        .expando
        .get_or_try_init(|| Ok(SeLinuxFsHandle(SeLinuxFs::new_fs(current_task, options)?)))?
        .0
        .clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_runtime;
    use selinux::SecurityServer;
    use zerocopy::{FromBytes, KnownLayout};

    #[fuchsia::test]
    fn status_vmo_has_correct_size_and_rights() {
        // The current version of the "status" file contains five packed
        // u32 values.
        const STATUS_T_SIZE: usize = size_of::<u32>() * 5;

        let status_holder = StatusPublisher::new_default().unwrap();
        let status_vmo = status_holder.0.get_readonly_vmo();

        // Verify the content and actual size of the structure are as expected.
        let content_size = status_vmo.get_content_size().unwrap() as usize;
        assert_eq!(content_size, STATUS_T_SIZE);
        let actual_size = status_vmo.get_size().unwrap() as usize;
        assert!(actual_size >= STATUS_T_SIZE);

        // Ensure the returned handle is read-only and non-resizable.
        let rights = status_vmo.basic_info().unwrap().rights;
        assert_eq!((rights & zx::Rights::MAP), zx::Rights::MAP);
        assert_eq!((rights & zx::Rights::READ), zx::Rights::READ);
        assert_eq!((rights & zx::Rights::GET_PROPERTY), zx::Rights::GET_PROPERTY);
        assert_eq!((rights & zx::Rights::WRITE), zx::Rights::NONE);
        assert_eq!((rights & zx::Rights::RESIZE), zx::Rights::NONE);
    }

    #[derive(KnownLayout, FromBytes)]
    #[repr(C, align(4))]
    struct TestSeLinuxStatusT {
        version: u32,
        sequence: u32,
        enforcing: u32,
        policyload: u32,
        deny_unknown: u32,
    }

    fn with_status_t<R>(
        status_vmo: &Arc<zx::Vmo>,
        do_test: impl FnOnce(&TestSeLinuxStatusT) -> R,
    ) -> R {
        let flags = zx::VmarFlags::PERM_READ
            | zx::VmarFlags::ALLOW_FAULTS
            | zx::VmarFlags::REQUIRE_NON_RESIZABLE;
        let map_addr = fuchsia_runtime::vmar_root_self()
            .map(0, status_vmo, 0, size_of::<TestSeLinuxStatusT>(), flags)
            .unwrap();
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let mapped_status = unsafe { &mut *(map_addr as *mut TestSeLinuxStatusT) };
        let result = do_test(mapped_status);
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            fuchsia_runtime::vmar_root_self()
                .unmap(map_addr, size_of::<TestSeLinuxStatusT>())
                .unwrap()
        };
        result
    }

    #[fuchsia::test]
    fn status_file_layout() {
        let security_server = SecurityServer::new_default();
        let status_holder = StatusPublisher::new_default().unwrap();
        let status_vmo = status_holder.0.get_readonly_vmo();
        security_server.set_status_publisher(Box::new(status_holder));
        security_server.set_enforcing(false);
        let mut seq_no: u32 = 0;
        with_status_t(&status_vmo, |status| {
            assert_eq!(status.version, SELINUX_STATUS_VERSION);
            assert_eq!(status.enforcing, 0);
            seq_no = status.sequence;
            assert_eq!(seq_no % 2, 0);
        });
        security_server.set_enforcing(true);
        with_status_t(&status_vmo, |status| {
            assert_eq!(status.version, SELINUX_STATUS_VERSION);
            assert_eq!(status.enforcing, 1);
            assert_ne!(status.sequence, seq_no);
            seq_no = status.sequence;
            assert_eq!(seq_no % 2, 0);
        });
    }

    #[fuchsia::test]
    fn status_accurate_directly_following_set_status_publisher() {
        let security_server = SecurityServer::new_default();
        let status_holder = StatusPublisher::new_default().unwrap();
        let status_vmo = status_holder.0.get_readonly_vmo();

        // Ensure a change in status-visible security server state is made before invoking
        // `set_status_publisher()`.
        assert_eq!(false, security_server.is_enforcing());
        security_server.set_enforcing(true);

        security_server.set_status_publisher(Box::new(status_holder));
        with_status_t(&status_vmo, |status| {
            // Ensure latest `enforcing` state is reported immediately following
            // `set_status_publisher()`.
            assert_eq!(status.enforcing, 1);
        });
    }
}
