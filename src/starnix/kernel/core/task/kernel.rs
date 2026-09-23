// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bpf::EbpfState;
use crate::device::remote_block_device::RemoteBlockDeviceRegistry;
use crate::device::{DeviceMode, DeviceRegistry};
use crate::execution::CrashReporter;
use crate::mm::{FutexTable, MappingSummary, MlockPinFlavor, SharedFutexKey};
use crate::power::{SuspendResumeManagerHandle, create_watcher_for_wake_events};
use crate::ptrace::StopState;
use crate::security::{self, AuditLogger};
use crate::task::container_namespace::ContainerNamespace;
use crate::task::limits::SystemLimits;
use crate::task::memory_attribution::MemoryAttributionManager;
use crate::task::net::NetstackDevices;
use crate::task::tracing::TracePerformanceEventManager;
use crate::task::{
    AbstractUnixSocketNamespace, AbstractVsockSocketNamespace, CurrentTask, DelayedReleaser,
    IpTables, KernelCgroups, KernelStats, KernelThreads, PidTable, SchedulerManager, Syslog, Task,
    ThreadGroup, UtsNamespace, UtsNamespaceHandle,
};
use crate::time::{HrTimerManager, HrTimerManagerHandle};
use crate::vdso::vdso_loader::Vdso;
use crate::vfs::fs_args::MountParams;
use crate::vfs::socket::{
    GenericMessage, GenericNetlink, NetlinkAccessControl, NetlinkContextImpl,
    NetlinkToClientSender, SocketAddress, SocketTokensStore,
};
use crate::vfs::{CacheConfig, FileOps, FsNodeHandle, FsString, Mounts, NamespaceNode};
use bstr::{BString, ByteSlice};
use devicetree::types::Devicetree;
use expando::Expando;
use fidl::endpoints::{ClientEnd, DiscoverableProtocolMarker, ProtocolMarker, create_endpoints};
use fidl_fuchsia_component_runner::{ComponentControllerControlHandle, ComponentStopInfo};
use fidl_fuchsia_feedback::CrashReporterProxy;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_memory_attribution as fattribution;
use fidl_fuchsia_net_power as fnet_power;
use fidl_fuchsia_net_resources as fnet_resources;
use fidl_fuchsia_time_external::AdjustSynchronousProxy;
use fuchsia_async as fasync;
use fuchsia_inspect::ArrayProperty;
use fuchsia_rcu::RcuReadScope;
use futures::FutureExt;
use netlink::interfaces::InterfacesHandler;
use netlink::{NETLINK_LOG_TAG, Netlink};
use once_cell::sync::OnceCell;
use smallvec::SmallVec;
use starnix_lifecycle::AtomicCounter;
use starnix_logging::{SyscallLogFilter, log_debug, log_error, log_info, log_warn};
use starnix_sync::{
    ComponentControllerLock, KernelSwapFiles, LockDepGuard, LockDepMutex, MountsLevel, RwSeqLock,
    RwSeqLockGuard, SyscallLogFiltersLock,
};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::{Errno, errno};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{VMADDR_CID_HOST, from_status_like_fdio};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use zx::CpuFeatureFlags;

/// Kernel features are specified in the component manifest of the starnix container
/// or explicitly provided to the kernel constructor in tests.
#[derive(Debug, Default, Clone)]
pub struct KernelFeatures {
    pub bpf_v2: bool,

    /// Whether the kernel supports the S_ISUID and S_ISGID bits.
    ///
    /// For example, these bits are used by `sudo`.
    ///
    /// Enabling this feature is potentially a security risk because they allow privilege
    /// escalation.
    pub enable_suid: bool,

    /// Whether io_uring is enabled.
    ///
    /// TODO(https://fxbug.dev/297431387): Enabled by default once the feature is completed.
    pub io_uring: bool,

    /// Whether the kernel should return an error to userspace, rather than panicking, if `reboot()`
    /// is requested but cannot be enacted because the kernel lacks the relevant capabilities.
    pub error_on_failed_reboot: bool,

    /// The default seclabel that is applied to components that are run in this kernel.
    ///
    /// Components can override this by setting the `seclabel` field in their program block.
    pub default_seclabel: Option<String>,

    /// Whether the kernel is being used to run the SELinux Test Suite.
    ///
    /// TODO: https://fxbug.dev/388077431 - remove this once we no longer need workarounds for the
    /// SELinux Test Suite.
    pub selinux_test_suite: bool,

    /// The default mount options to use when mounting directories from a component's namespace.
    ///
    /// The key is the path in the component's namespace, and the value is the mount options
    /// string.
    pub default_ns_mount_options: Option<HashMap<String, String>>,

    /// The default uid that is applied to components that are run in this kernel.
    ///
    /// Components can override this by setting the `uid` field in their program block.
    pub default_uid: u32,

    /// mlock() never prefaults pages.
    pub mlock_always_onfault: bool,

    /// Implementation of mlock() to use for this kernel instance.
    pub mlock_pin_flavor: MlockPinFlavor,

    /// Whether excessive crash reports should be throttled.
    pub crash_report_throttling: bool,

    /// Whether or not to serve wifi support to Android.
    pub wifi: bool,

    /// The number of bytes to cache in pages for reading zx::MapInfo from VMARs.
    pub cached_zx_map_info_bytes: u32,

    /// The size of the Dirent LRU cache.
    pub dirent_cache_size: u32,

    /// Whether to expose a stub '/dev/ion' node, as a temporary workaround for compatibility.
    // TODO(https://fxbug.dev/485370648) remove when unnecessary
    pub fake_ion: bool,
}

impl KernelFeatures {
    /// Returns the `MountParams` to use when mounting the specified path from a component's
    /// namespace.  This mechanism is also used to specified options for mounts created via
    /// container features, by specifying a pseudo-path e.g. "#container".
    pub fn ns_mount_options(&self, ns_path: &str) -> Result<MountParams, Errno> {
        if let Some(all_options) = &self.default_ns_mount_options {
            if let Some(options) = all_options.get(ns_path) {
                return MountParams::parse(options.as_bytes().into());
            }
        }
        Ok(MountParams::default())
    }
}

/// Kernel command line argument structure
pub struct ArgNameAndValue<'a> {
    pub name: &'a str,
    pub value: Option<&'a str>,
}

type DeferredDropCallback = Box<dyn FnOnce() + Send + Sync>;

/// A proof token representing the global lock over the namespace mount topology.
///
/// Functions that take `&MountsWriteToken` require the caller to hold the
/// `Kernel::mounts_lock` to ensure safe modification of the global mount tree.
pub struct MountsWriteToken {
    deferred_drops: RefCell<SmallVec<[DeferredDropCallback; 8]>>,
}

impl MountsWriteToken {
    fn new() -> Self {
        Self { deferred_drops: RefCell::new(SmallVec::new()) }
    }

    /// We cannot block while holding a [MountsWriteGuard] as it is a spinlock. If a Drop requires
    /// a blocking operation, use [Self::defer_drop] defer the blocking operation until
    /// after the [MountsWriteGuard] is released.
    pub fn defer_drop<T: Send + Sync + 'static>(&self, value: T) {
        self.deferred_drops.borrow_mut().push(Box::new(move || {
            drop(value);
        }));
    }

    /// Helper method to [Self::defer_drop] specifically for [Arc<T>]s.
    pub fn retain<T: Send + Sync + 'static>(&self, value: &Arc<T>) -> Arc<T> {
        self.defer_drop(value.clone());
        value.clone()
    }

    fn take_deferred_drops(&mut self) -> SmallVec<[DeferredDropCallback; 8]> {
        std::mem::take(self.deferred_drops.get_mut())
    }
}

/// A guard special cased for mount operations to defer destroying mounts until after all
/// modifications of the global mount tree are completed.
pub struct MountsWriteGuard<'a> {
    guard: Option<RwSeqLockGuard<'a, LockDepGuard<'a, MountsWriteToken>>>,
}

impl<'a> MountsWriteGuard<'a> {
    fn new(guard: RwSeqLockGuard<'a, LockDepGuard<'a, MountsWriteToken>>) -> Self {
        Self { guard: Some(guard) }
    }
}

impl<'a> Deref for MountsWriteGuard<'a> {
    type Target = MountsWriteToken;
    fn deref(&self) -> &Self::Target {
        self.guard.as_ref().unwrap()
    }
}

impl<'a> Drop for MountsWriteGuard<'a> {
    fn drop(&mut self) {
        if let Some(mut guard) = self.guard.take() {
            let drops = guard.take_deferred_drops();
            drop(guard);
            for callback in drops {
                callback();
            }
        }
    }
}

/// The shared, mutable state for the entire Starnix kernel.
///
/// The `Kernel` object holds all kernel threads, userspace tasks, and file system resources for a
/// single instance of the Starnix kernel. In production, there is one instance of this object for
/// the entire Starnix kernel. However, multiple instances of this object can be created in one
/// process during unit testing.
///
/// The structure of this object will likely need to evolve as we implement more namespacing and
/// isolation mechanisms, such as `namespaces(7)` and `pid_namespaces(7)`.
pub struct Kernel {
    /// Weak reference to self. Allows to not have to pass &Arc<Kernel> in apis.
    pub weak_self: Weak<Kernel>,

    /// The kernel threads running on behalf of this kernel.
    pub kthreads: KernelThreads,

    /// The features enabled for this kernel.
    pub features: KernelFeatures,

    /// The processes and threads running in this kernel, organized by pid_t.
    pub pids: PidTable,

    /// A weak reference to the init task (PID 1).
    pub init_task: OnceLock<Weak<Task>>,

    /// Shared manager of pid/tid to koid mappings for tracing and profiling clients.
    /// A direct field (rather than an expando entry) so the task-creation fast path is a
    /// single relaxed atomic load with no locks when nothing is recording.
    pub trace_event_manager: Arc<TracePerformanceEventManager>,

    /// Subsystem-specific properties that hang off the Kernel object.
    ///
    /// Instead of adding yet another property to the Kernel object, consider storing the property
    /// in an expando if that property is only used by one part of the system, such as a module.
    pub expando: Expando,

    /// The default namespace for abstract AF_UNIX sockets in this kernel.
    ///
    /// Rather than use this default namespace, abstract socket addresses
    /// should be looked up in the AbstractSocketNamespace on each Task
    /// object because some Task objects might have a non-default namespace.
    pub default_abstract_socket_namespace: Arc<AbstractUnixSocketNamespace>,

    /// The default namespace for abstract AF_VSOCK sockets in this kernel.
    pub default_abstract_vsock_namespace: Arc<AbstractVsockSocketNamespace>,

    /// The kernel command line. Shows up in /proc/cmdline.
    pub cmdline: BString,

    pub device_tree: Option<Devicetree>,

    // Global state held by the Linux Security Modules subsystem.
    pub security_state: security::KernelState,

    /// The registry of device drivers.
    pub device_registry: DeviceRegistry,

    /// Mapping of top-level namespace entries to an associated proxy.
    /// For example, "/svc" to the respective proxy. Only the namespace entries
    /// which were known at component startup will be available by the kernel.
    pub container_namespace: ContainerNamespace,

    /// The global lock for the mount tree.
    ///
    /// This lock protects against concurrent modifications to the mount topology. It uses
    /// an `RwSeqLock` to allow readers (like path walking traversing mount points) to get a
    /// consistent, lock-free snapshot of the RCU-protected mount table using `read_seq`.
    /// Writers must acquire the lock before mutating filesystems, moving mounts, or
    /// propagating peer groups. The returned `MountsWriteToken` is used as a proof token
    /// throughout the `namespace` module to statically enforce exclusive write access.
    pub mounts_lock: RwSeqLock<LockDepMutex<MountsWriteToken, MountsLevel>>,

    /// The registry of block devices backed by a remote fuchsia.io file.
    pub remote_block_device_registry: Arc<RemoteBlockDeviceRegistry>,

    /// The iptables used for filtering network packets.
    iptables: OnceLock<IpTables>,

    /// The futexes shared across processes.
    pub shared_futexes: Arc<FutexTable<SharedFutexKey>>,

    /// The default UTS namespace for all tasks.
    ///
    /// Because each task can have its own UTS namespace, you probably want to use
    /// the UTS namespace handle of the task, which may/may not point to this one.
    pub root_uts_ns: UtsNamespaceHandle,

    /// A struct containing a VMO with a vDSO implementation, if implemented for a given architecture, and possibly an offset for a sigreturn function.
    pub vdso: Vdso,

    /// A struct containing a VMO with a arch32-vDSO implementation, if implemented for a given architecture.
    // TODO(https://fxbug.dev/380431743) This could be made less clunky -- maybe a Vec<Vdso> above or
    // something else
    pub vdso_arch32: Option<Vdso>,

    /// The table of devices installed on the netstack and their associated
    /// state local to this `Kernel`.
    pub netstack_devices: Arc<NetstackDevices>,

    /// Files that are currently available for swapping.
    /// Note: Starnix never actually swaps memory to these files. We just need to track them
    /// to pass conformance tests.
    pub swap_files: LockDepMutex<Vec<FsNodeHandle>, KernelSwapFiles>,

    /// The implementation of generic Netlink protocol families.
    generic_netlink: OnceLock<GenericNetlink<NetlinkToClientSender<GenericMessage>>>,

    /// The implementation of networking-related Netlink protocol families.
    network_netlink: OnceLock<Netlink<NetlinkContextImpl>>,

    /// Inspect instrumentation for this kernel instance.
    pub inspect_node: fuchsia_inspect::Node,

    /// The kinds of seccomp action that gets logged, stored as a bit vector.
    /// Each potential SeccompAction gets a bit in the vector, as specified by
    /// SeccompAction::logged_bit_offset.  If the bit is set, that means the
    /// action should be logged when it is taken, subject to the caveats
    /// described in seccomp(2).  The value of the bit vector is exposed to users
    /// in a text form in the file /proc/sys/kernel/seccomp/actions_logged.
    pub actions_logged: AtomicU16,

    /// The manager for suspend/resume.
    pub suspend_resume_manager: SuspendResumeManagerHandle,

    /// Unique IDs for new mounts and mount namespaces.
    pub next_mount_id: AtomicCounter<u64>,
    pub next_peer_group_id: AtomicCounter<u64>,
    pub next_namespace_id: AtomicCounter<u64>,

    /// Unique IDs for file objects.
    pub next_file_object_id: AtomicCounter<u64>,

    /// Controls which processes a process is allowed to ptrace.  See Documentation/security/Yama.txt
    pub ptrace_scope: AtomicU8,

    // The Fuchsia build version returned by `fuchsia.buildinfo.Provider`.
    pub build_version: OnceCell<String>,

    pub stats: Arc<KernelStats>,

    /// Resource limits that are exposed, for example, via sysctl.
    pub system_limits: SystemLimits,

    // The service to handle delayed releases. This is required for elements that requires to
    // execute some code when released and requires a known context (both in term of lock context,
    // as well as `CurrentTask`).
    pub delayed_releaser: DelayedReleaser,

    /// Manages task priorities.
    pub scheduler: SchedulerManager,

    /// The syslog manager.
    pub syslog: Syslog,

    /// All mounts.
    pub mounts: Mounts,

    /// The manager for creating and managing high-resolution timers.
    pub hrtimer_manager: HrTimerManagerHandle,

    /// The manager for monitoring and reporting resources used by the kernel.
    pub memory_attribution_manager: MemoryAttributionManager,

    /// Handler for crashing Linux processes.
    pub crash_reporter: CrashReporter,

    /// Whether this kernel is shutting down. When shutting down, new processes may not be spawned.
    shutting_down: AtomicBool,

    /// True to disable syslog access to unprivileged callers.  This also controls whether read
    /// access to /dev/kmsg requires privileged capabilities.
    pub restrict_dmesg: AtomicBool,

    /// Determines whether unprivileged BPF is permitted, or can be re-enabled.
    ///   0 - Unprivileged BPF is permitted.
    ///   1 - Unprivileged BPF is not permitted, and cannot be enabled.
    ///   2 - Unprivileged BPF is not permitted, but can be enabled by a privileged task.
    pub disable_unprivileged_bpf: AtomicU8,

    /// Control handle to the running container's ComponentController.
    pub container_control_handle:
        LockDepMutex<Option<ComponentControllerControlHandle>, ComponentControllerLock>,

    /// eBPF state: loaded programs, eBPF maps, etc.
    pub ebpf_state: EbpfState,

    /// Cgroups of the kernel.
    pub cgroups: KernelCgroups,

    /// Used to communicate requests to adjust system time from within a Starnix
    /// container. Used from syscalls.
    pub time_adjustment_proxy: Option<AdjustSynchronousProxy>,

    /// A token for the wake group we have registered with the netstack to
    /// receive wakeup notifications on incoming data while suspended.
    ///
    /// A value of `None` means we were unable to communicate with the netstack
    /// to create the wake group. In that case, we assume something has gone
    /// wrong with the netstack (failed to start, crash, etc) and won't retry.
    pub netstack_wake_group: OnceLock<Option<fnet_resources::WakeGroupToken>>,

    /// Used to store tokens for sockets, particularly per-uid sharing domain sockets.
    pub socket_tokens_store: SocketTokensStore,

    /// Hardware capabilities to push onto stack when loading an ELF binary.
    pub hwcaps: HwCaps,

    /// Filters for syscall logging. Processes with names matching these filters will have syscalls
    /// logged at INFO level.
    pub syscall_log_filters: LockDepMutex<Vec<SyscallLogFilter>, SyscallLogFiltersLock>,
}

/// Hardware capabilities.
#[derive(Debug, Clone, Copy, Default)]
pub struct HwCap {
    /// The value for `AT_HWCAP`.
    pub hwcap: u32,
    /// The value for `AT_HWCAP2`.
    pub hwcap2: u32,
}

/// Hardware capabilities for both 32-bit and 64-bit ELF binaries.
#[derive(Debug, Clone, Copy, Default)]
pub struct HwCaps {
    /// For 32-bit binaries.
    #[cfg(target_arch = "aarch64")]
    pub arch32: HwCap,
    /// For 64-bit binaries.
    pub arch64: HwCap,
}

/// An implementation of [`InterfacesHandler`].
///
/// This holds a `Weak<Kernel>` because it is held within a [`Netlink`] which
/// is itself held within an `Arc<Kernel>`. Holding an `Arc<T>` within an
/// `Arc<T>` prevents the `Arc`'s ref count from ever reaching 0, causing a
/// leak.
struct InterfacesHandlerImpl(Weak<Kernel>);

impl InterfacesHandlerImpl {
    fn kernel(&self) -> Option<Arc<Kernel>> {
        self.0.upgrade()
    }
}

impl InterfacesHandler for InterfacesHandlerImpl {
    fn handle_new_link(&mut self, name: &str, interface_id: NonZeroU64) {
        if let Some(kernel) = self.kernel() {
            kernel.netstack_devices.add_device(&kernel, name.into(), interface_id);
        }
    }

    fn handle_deleted_link(&mut self, name: &str) {
        if let Some(kernel) = self.kernel() {
            kernel.netstack_devices.remove_device(&kernel, name.into());
        }
    }

    fn handle_idle_event(&mut self) {
        let Some(kernel) = self.kernel() else {
            log_error!("kernel went away while netlink is initializing");
            return;
        };
        let (initialized, wq) = &kernel.netstack_devices.initialized_and_wq;
        if initialized.swap(true, Ordering::SeqCst) {
            log_error!("netlink initial devices should only be reported once");
            return;
        }
        wq.notify_all()
    }
}

impl Kernel {
    pub fn new(
        cmdline: BString,
        features: KernelFeatures,
        system_limits: SystemLimits,
        container_namespace: ContainerNamespace,
        scheduler: SchedulerManager,
        crash_reporter_proxy: Option<CrashReporterProxy>,
        inspect_node: fuchsia_inspect::Node,
        security_state: security::KernelState,
        time_adjustment_proxy: Option<AdjustSynchronousProxy>,
        device_tree: Option<Devicetree>,
    ) -> Result<Arc<Kernel>, zx::Status> {
        let unix_address_maker =
            Box::new(|x: FsString| -> SocketAddress { SocketAddress::Unix(x) });
        let vsock_address_maker = Box::new(|x: u32| -> SocketAddress {
            SocketAddress::Vsock { port: x, cid: VMADDR_CID_HOST }
        });

        let crash_reporter = CrashReporter::new(
            &inspect_node,
            crash_reporter_proxy,
            zx::Duration::from_minutes(8),
            features.crash_report_throttling,
        );
        let hrtimer_manager = HrTimerManager::new(&inspect_node);

        let cpu_feature_flags =
            zx::system_get_feature_flags::<CpuFeatureFlags>().unwrap_or_else(|e| {
                log_debug!("CPU feature flags are only supported on ARM64: {}, reporting 0", e);
                CpuFeatureFlags::empty()
            });
        let hwcaps = HwCaps::from_cpu_feature_flags(cpu_feature_flags);

        let this = Arc::new_cyclic(|kernel| Kernel {
            weak_self: kernel.clone(),
            kthreads: KernelThreads::new(kernel.clone()),
            features,
            pids: Default::default(),
            init_task: OnceLock::new(),
            trace_event_manager: Arc::new(TracePerformanceEventManager::new(kernel.clone())),
            expando: Default::default(),
            default_abstract_socket_namespace: AbstractUnixSocketNamespace::new(unix_address_maker),
            default_abstract_vsock_namespace: AbstractVsockSocketNamespace::new(
                vsock_address_maker,
            ),
            cmdline,
            device_tree,
            security_state,
            device_registry: Default::default(),
            container_namespace,
            mounts_lock: RwSeqLock::new(MountsWriteToken::new().into()),
            remote_block_device_registry: Default::default(),
            iptables: OnceLock::new(),
            shared_futexes: Arc::<FutexTable<SharedFutexKey>>::default(),
            root_uts_ns: Arc::new(UtsNamespace::default().into()),
            vdso: Vdso::new(),
            vdso_arch32: Vdso::new_arch32(),
            netstack_devices: Arc::default(),
            swap_files: Default::default(),
            generic_netlink: OnceLock::new(),
            network_netlink: OnceLock::new(),
            inspect_node,
            actions_logged: AtomicU16::new(0),
            suspend_resume_manager: Default::default(),
            next_mount_id: AtomicCounter::<u64>::new(1),
            next_peer_group_id: AtomicCounter::<u64>::new(1),
            next_namespace_id: AtomicCounter::<u64>::new(1),
            next_file_object_id: Default::default(),
            system_limits,
            ptrace_scope: AtomicU8::new(0), // Disable YAMA checks by default.
            restrict_dmesg: AtomicBool::new(false),
            disable_unprivileged_bpf: AtomicU8::new(0), // Enable unprivileged BPF by default.
            build_version: OnceCell::new(),
            stats: Arc::new(KernelStats::default()),
            delayed_releaser: Default::default(),
            scheduler,
            syslog: Default::default(),
            mounts: Mounts::new(),
            hrtimer_manager,
            memory_attribution_manager: MemoryAttributionManager::new(kernel.clone()),
            crash_reporter,
            shutting_down: AtomicBool::new(false),
            container_control_handle: Default::default(),
            ebpf_state: Default::default(),
            cgroups: Default::default(),
            time_adjustment_proxy,
            netstack_wake_group: OnceLock::new(),
            socket_tokens_store: Default::default(),
            hwcaps,
            syscall_log_filters: Default::default(),
        });

        // Initialize the device registry before registering any devices.
        //
        // We will create sysfs recursively within this function.
        this.device_registry.objects.init(&this);

        // Make a copy of this Arc for the inspect lazy node to use but don't create an Arc cycle
        // because the inspect node that owns this reference is owned by the kernel.
        let kernel = Arc::downgrade(&this);
        this.inspect_node.record_lazy_child("thread_groups", move || {
            if let Some(kernel) = kernel.upgrade() {
                let inspector = kernel.get_thread_groups_inspect();
                async move { Ok(inspector) }.boxed()
            } else {
                async move { Err(anyhow::format_err!("kernel was dropped")) }.boxed()
            }
        });

        let kernel = Arc::downgrade(&this);
        this.inspect_node.record_lazy_child("cgroupv2", move || {
            if let Some(kernel) = kernel.upgrade() {
                async move { Ok(kernel.cgroups.cgroup2.get_cgroup_inspect()) }.boxed()
            } else {
                async move { Err(anyhow::format_err!("kernel was dropped")) }.boxed()
            }
        });

        Ok(this)
    }

    /// Returns the init task for this kernel.
    pub fn get_init_task(&self) -> Result<Arc<Task>, Errno> {
        self.init_task.get().and_then(|t| t.upgrade()).ok_or_else(|| errno!(EINVAL))
    }

    /// Shuts down userspace and the kernel in an orderly fashion, eventually terminating the root
    /// kernel process.
    pub fn shut_down(self: &Arc<Self>) {
        // Run shutdown code on a kthread in the main process so that it can be the last process
        // alive.
        self.kthreads.spawn_future(
            {
                let kernel = self.clone();
                move || async move {
                    kernel.run_shutdown().await;
                }
            },
            "run_shutdown",
        );
    }

    /// Starts shutting down the Starnix kernel and any running container. Only one thread can drive
    /// shutdown at a time. This function will return immediately if shut down is already under way.
    ///
    /// Shutdown happens in several phases:
    ///
    /// 1. Disable launching new processes
    /// 2. Shut down individual ThreadGroups until only the init and system tasks remain
    /// 3. Repeat the above for the init task
    /// 4. Clean up kernel-internal structures that can hold processes alive
    /// 5. Ensure this process is the only one running in the kernel job.
    /// 6. Unmounts the kernel's mounts' FileSystems.
    /// 7. Tell CF the container component has stopped
    /// 8. Exit this process
    ///
    /// If a ThreadGroup does not shut down on its own (including after SIGKILL), that phase of
    /// shutdown will hang. To gracefully shut down any further we need the other kernel processes
    /// to do controlled exits that properly release access to shared state. If our orderly shutdown
    /// does hang, eventually CF will kill the container component which will lead to the job of
    /// this process being killed and shutdown will still complete.
    async fn run_shutdown(&self) {
        const INIT_PID: i32 = 1;
        const SYSTEM_TASK_PID: i32 = 2;

        // Step 1: Prevent new processes from being created once they observe this update. We don't
        // want the thread driving shutdown to be racing with other threads creating new processes.
        if self
            .shutting_down
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            log_info!("Additional thread tried to initiate shutdown while already in-progress.");
            return;
        }

        log_info!("Shutting down Starnix kernel.");

        // Step 2: Shut down thread groups in a loop until init and the system task are all that
        // remain.
        loop {
            let tgs = self
                .pids
                .get_thread_groups(&RcuReadScope::new())
                .filter(|tg| tg.leader.id != SYSTEM_TASK_PID && tg.leader.id != INIT_PID)
                .collect::<Vec<_>>();
            if tgs.is_empty() {
                log_info!("pid table is empty except init and system task");
                break;
            }

            log_info!(tgs:?; "shutting down thread groups");
            let mut tasks = vec![];
            for tg in tgs {
                let task = fasync::Task::local(ThreadGroup::shut_down(Arc::downgrade(&tg)));
                tasks.push(task);
            }
            futures::future::join_all(tasks).await;
        }

        // Step 3: Terminate the init process.
        let maybe_init = self.get_init_task().ok().map(|t| Arc::downgrade(&t.thread_group));
        if let Some(init) = maybe_init {
            log_info!("shutting down init");
            ThreadGroup::shut_down(init).await;
        } else {
            log_info!("init already terminated");
        }

        // Step 4: Clean up any structures that can keep non-Linux processes live in our job.
        log_info!("cleaning up pinned memory");
        self.expando.remove::<crate::mm::InfoCacheShadowProcess>();
        self.expando.remove::<crate::mm::MlockShadowProcess>();

        // Step 5: Make sure this is the only process running in the job. We already should have
        // cleared up all processes other than the system task at this point, but wait on any that
        // might be around for good measure.
        //
        // Use unwrap liberally since we're shutting down anyway and errors will still tear down the
        // kernel.
        let kernel_job = fuchsia_runtime::job_default();
        assert_eq!(kernel_job.children().unwrap(), &[], "starnix does not create any child jobs");
        let own_koid = fuchsia_runtime::process_self().koid().unwrap();

        log_info!("waiting for this to be the only process in the job");
        loop {
            let mut remaining_processes = kernel_job
                .processes()
                .unwrap()
                .into_iter()
                // Don't wait for ourselves to exit.
                .filter(|pid| pid != &own_koid)
                .peekable();
            if remaining_processes.peek().is_none() {
                log_info!("No stray Zircon processes.");
                break;
            }

            let mut terminated_signals = vec![];
            for pid in remaining_processes {
                let handle = match kernel_job
                    .get_child(&pid, zx::Rights::BASIC | zx::Rights::PROPERTY | zx::Rights::DESTROY)
                {
                    Ok(h) => h,
                    Err(e) => {
                        log_info!(pid:?, e:?; "failed to get child process from job");
                        continue;
                    }
                };
                log_info!(
                    pid:?,
                    name:? = handle.get_name();
                    "waiting on process terminated signal"
                );
                terminated_signals
                    .push(fuchsia_async::OnSignals::new(handle, zx::Signals::PROCESS_TERMINATED));
            }
            log_info!("waiting on process terminated signals");
            futures::future::join_all(terminated_signals).await;
        }

        // Step 6: Forcibly unmounts the mounts' FileSystems.
        log_info!("clearing mounts");
        self.mounts.clear();

        // Step 7: Tell CF the container stopped.
        log_info!("all non-root processes killed, notifying CF container is stopped");
        if let Some(control_handle) = self.container_control_handle.lock().take() {
            log_info!("Notifying CF that the container has stopped.");
            control_handle
                .send_on_stop(ComponentStopInfo {
                    termination_status: Some(zx::sys::ZX_OK),
                    exit_code: Some(0),
                    ..ComponentStopInfo::default()
                })
                .unwrap();
            control_handle.shutdown_with_epitaph(Ok(()));
        } else {
            log_warn!("Shutdown invoked without a container controller control handle.");
        }

        // Step 8: exiting this process.
        log_info!("All tasks killed, exiting Starnix kernel root process.");
        // Normally a Rust program exits its process by calling `std::process::exit()` which goes
        // through libc to exit the program. This runs drop impls on any thread-local variables
        // which can cause issues during Starnix shutdown when we haven't yet integrated every
        // subsystem with the shutdown flow. While those issues are indicative of underlying
        // problems, we can't solve them without finishing the implementation of graceful shutdown.
        // Instead, ask Zircon to exit our process directly, bypassing any libc atexit handlers.
        // TODO(https://fxbug.dev/295073633) return from main instead of avoiding atexit handlers
        zx::Process::exit(0);
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    pub fn allow_unprivileged_bpf(&self) -> bool {
        self.disable_unprivileged_bpf.load(Ordering::Relaxed) == 0
    }

    /// Opens a device file (driver) identified by `dev`.
    pub fn open_device(
        &self,
        current_task: &CurrentTask,
        node: &NamespaceNode,
        flags: OpenFlags,
        dev: DeviceId,
        mode: DeviceMode,
    ) -> Result<Box<dyn FileOps>, Errno> {
        self.device_registry.open_device(current_task, node, flags, dev, mode)
    }

    /// Return a reference to the Audit Framework
    ///
    /// This function follows the lazy initialization pattern.
    pub fn audit_logger(&self) -> Arc<AuditLogger> {
        self.expando.get_or_init(|| AuditLogger::new(self))
    }

    /// Return a reference to the GenericNetlink implementation.
    ///
    /// This function follows the lazy initialization pattern, where the first
    /// call will instantiate the Generic Netlink server in a separate kthread.
    pub fn generic_netlink(&self) -> &GenericNetlink<NetlinkToClientSender<GenericMessage>> {
        self.generic_netlink.get_or_init(|| {
            let (generic_netlink, worker_params) = GenericNetlink::new();
            let enable_nl80211 = self.features.wifi;
            self.kthreads.spawn_future(
                move || async move {
                    crate::vfs::socket::run_generic_netlink_worker(worker_params, enable_nl80211)
                        .await;
                    log_error!("Generic Netlink future unexpectedly exited");
                },
                "generic_netlink_worker",
            );
            generic_netlink
        })
    }

    /// Return a reference to the [`netlink::Netlink`] implementation.
    ///
    /// This function follows the lazy initialization pattern, where the first
    /// call will instantiate the Netlink implementation.
    pub fn network_netlink(self: &Arc<Self>) -> &Netlink<NetlinkContextImpl> {
        self.network_netlink.get_or_init(|| {
            let (network_netlink, worker_params) =
                Netlink::new(InterfacesHandlerImpl(self.weak_self.clone()));

            let kernel = self.clone();
            self.kthreads.spawn_future(
                move || async move {
                    netlink::run_netlink_worker(
                        worker_params,
                        NetlinkAccessControl::new(kernel.kthreads.system_task()),
                    )
                    .await;
                    log_error!(tag = NETLINK_LOG_TAG; "Netlink async worker unexpectedly exited");
                },
                "network_netlink_worker",
            );
            network_netlink
        })
    }

    /// Return a reference to the token representing our wake group registered
    /// with the netstack.
    ///
    /// This function lazily initializes the wake group with the netstack and
    /// the wake group as a wake source with the Starnix runner.
    pub(crate) fn netstack_wake_group(&self) -> Option<&fnet_resources::WakeGroupToken> {
        self.netstack_wake_group
            .get_or_init(|| {
                // The signal the netstack raises when it wants the container to
                // wake up.
                const GROUP_WAKEUP_SIGNAL: zx::Signals =
                    zx::Signals::from_bits(fnet_power::GROUP_WAKEUP_SIGNAL).unwrap();

                let provider = fuchsia_component::client::connect_to_protocol_sync::<
                    fnet_power::WakeGroupProviderMarker,
                >()
                .expect("connect to WakeGroupProvider");

                let (wake_watcher_waiter, wake_watcher_signaller) = zx::EventPair::create();
                create_watcher_for_wake_events(wake_watcher_signaller);

                let token = match provider.create_wake_group(
                    &fnet_power::WakeGroupOptions {
                        debug_name: Some(String::from("starnix")),
                        ..Default::default()
                    },
                    wake_watcher_waiter,
                    zx::Instant::INFINITE,
                ) {
                    Ok(fnet_power::CreateWakeGroupResponse { token, .. }) => token,
                    Err(e) => {
                        log_error!("failed to create wake group with the netstack: {e}");
                        return None;
                    }
                };

                let token = token.expect("netstack provides a wake group token");
                let wake_source = token
                    .token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("duplicate handle to token");
                self.suspend_resume_manager
                    .add_external_wake_source(
                        wake_source.into(),
                        GROUP_WAKEUP_SIGNAL,
                        "netstack-wake-group".into(),
                    )
                    .expect("add wake group as wake source");

                Some(token)
            })
            .as_ref()
    }

    pub fn iptables(&self) -> &IpTables {
        self.iptables.get_or_init(|| IpTables::new())
    }

    /// Returns a Proxy to the service used by the container at `filename`.
    #[allow(unused)]
    pub fn connect_to_named_protocol_at_container_svc<P: ProtocolMarker>(
        &self,
        filename: &str,
    ) -> Result<ClientEnd<P>, Errno> {
        match self.container_namespace.get_namespace_channel("/svc") {
            Ok(channel) => {
                let (client_end, server_end) = create_endpoints::<P>();
                fdio::service_connect_at(channel.as_ref(), filename, server_end.into_channel())
                    .map_err(|status| from_status_like_fdio!(status))?;
                Ok(client_end)
            }
            Err(err) => {
                log_error!("Unable to get /svc namespace channel! {}", err);
                Err(errno!(ENOENT))
            }
        }
    }

    /// Returns a Proxy to the service `P` used by the container.
    pub fn connect_to_protocol_at_container_svc<P: DiscoverableProtocolMarker>(
        &self,
    ) -> Result<ClientEnd<P>, Errno> {
        self.connect_to_named_protocol_at_container_svc::<P>(P::PROTOCOL_NAME)
    }

    pub fn add_syscall_log_filter(&self, name: &str) {
        let filter = SyscallLogFilter::new(name.to_string());
        {
            let mut filters = self.syscall_log_filters.lock();
            if filters.contains(&filter) {
                return;
            }
            filters.push(filter);
        }
        for headers in self.pids.get_thread_groups(&RcuReadScope::new()) {
            headers.sync_syscall_log_level();
        }
    }

    pub fn clear_syscall_log_filters(&self) {
        {
            let mut filters = self.syscall_log_filters.lock();
            if filters.is_empty() {
                return;
            }
            filters.clear();
        }
        for headers in self.pids.get_thread_groups(&RcuReadScope::new()) {
            headers.sync_syscall_log_level();
        }
    }

    fn get_thread_groups_inspect(&self) -> fuchsia_inspect::Inspector {
        let inspector = fuchsia_inspect::Inspector::default();

        let thread_groups = inspector.root();
        let mut mm_summary = MappingSummary::default();
        let mut mms_summarized = HashSet::new();

        for thread_group in self.pids.get_thread_groups(&RcuReadScope::new()) {
            // Avoid holding the state lock while summarizing.
            let (ppid, tasks) = {
                let tg = thread_group.read();
                (tg.get_ppid() as i64, tg.tasks())
            };

            let tg_node = thread_groups.create_child(format!("{}", thread_group.leader));
            if let Ok(koid) = thread_group.process.koid() {
                tg_node.record_int("koid", koid.raw_koid() as i64);
            }
            tg_node.record_int("pid", thread_group.leader.id as i64);
            tg_node.record_int("ppid", ppid);
            tg_node.record_bool("stopped", thread_group.load_stopped() == StopState::GroupStopped);

            let tasks_node = tg_node.create_child("tasks");
            for task in tasks {
                if let Ok(mm) = task.mm() {
                    if mms_summarized.insert(Arc::as_ptr(&mm) as usize) {
                        mm.summarize(&mut mm_summary);
                    }
                }
                let set_properties = |node: &fuchsia_inspect::Node| {
                    node.record_string("command", task.command().to_string());

                    let scheduler_state = task.read().scheduler_state;
                    if !scheduler_state.is_default() {
                        node.record_child("sched", |node| {
                            node.record_string(
                                "role_name",
                                self.scheduler
                                    .role_name(&task)
                                    .map(|n| Cow::Borrowed(n))
                                    .unwrap_or_else(|e| Cow::Owned(e.to_string())),
                            );
                            node.record_string("state", format!("{scheduler_state:?}"));
                        });
                    }
                };
                if task.tid == thread_group.leader {
                    let mut argv = task.read_argv(256).unwrap_or_default();

                    // Any runtime that overwrites argv is likely to leave a lot of trailing
                    // nulls, no need to print those in inspect.
                    argv.retain(|arg| !arg.is_empty());

                    let inspect_argv = tg_node.create_string_array("argv", argv.len());
                    for (i, arg) in argv.iter().enumerate() {
                        inspect_argv.set(i, arg.to_string());
                    }
                    tg_node.record(inspect_argv);

                    set_properties(&tg_node);
                } else {
                    tasks_node.record_child(task.tid.to_string(), |task_node| {
                        set_properties(task_node);
                    });
                };
            }
            tg_node.record(tasks_node);
            thread_groups.record(tg_node);
        }

        thread_groups.record_child("memory_managers", |node| mm_summary.record(node));

        inspector
    }

    pub fn new_memory_attribution_observer(
        &self,
        control_handle: fattribution::ProviderControlHandle,
    ) -> attribution_server::Observer {
        self.memory_attribution_manager.new_observer(control_handle)
    }

    /// Opens and returns a directory proxy from the container's namespace, at
    /// the requested path, using the provided flags. This method will open the
    /// closest existing path from the namespace hierarchy, and then attempt
    /// initialize an open on the remaining subdirectory path, using the given open_flags.
    ///
    /// For example, given the parameter provided is `/path/to/foo/bar` and there
    /// are namespace entries already for `/path/to/foo` and `/path/to`. The entry
    /// for /path/to/foo will be opened, and then the /bar will attempt to be opened
    /// underneath that directory with the given open_flags. The returned value
    /// will be the proxy to the parent (/path/to/foo) and the string to the child
    /// path (/bar). The caller of this method can expect /bar to be initialized.
    pub fn open_ns_dir(
        &self,
        path: &str,
        open_flags: fio::Flags,
    ) -> Result<(fio::DirectorySynchronousProxy, String), Errno> {
        let ns_path = PathBuf::from(path);
        match self.container_namespace.find_closest_channel(&ns_path) {
            Ok((root_channel, remaining_subdir)) => {
                let (_, server_end) = create_endpoints::<fio::DirectoryMarker>();
                fdio::open_at(
                    &root_channel,
                    &remaining_subdir,
                    open_flags,
                    server_end.into_channel(),
                )
                .map_err(|e| {
                    log_error!("Failed to intialize the subdirs: {}", e);
                    errno!(EIO)
                })?;

                Ok((fio::DirectorySynchronousProxy::new(root_channel), remaining_subdir))
            }
            Err(err) => {
                log_error!(
                    "Unable to find a channel for {}. Received error: {}",
                    ns_path.display(),
                    err
                );
                Err(errno!(ENOENT))
            }
        }
    }

    /// Returns an iterator of the command line arguments.
    pub fn cmdline_args_iter(&self) -> impl Iterator<Item = ArgNameAndValue<'_>> {
        parse_cmdline(self.cmdline.to_str().unwrap_or_default()).filter_map(|arg| {
            arg.split_once('=')
                .map(|(name, value)| ArgNameAndValue { name: name, value: Some(value) })
                .or(Some(ArgNameAndValue { name: arg, value: None }))
        })
    }

    /// Returns the container-configured CacheConfig.
    pub fn fs_cache_config(&self) -> CacheConfig {
        CacheConfig { capacity: self.features.dirent_cache_size as usize }
    }

    pub fn mounts_lock(&self) -> MountsWriteGuard<'_> {
        MountsWriteGuard::new(self.mounts_lock.lock())
    }
}

pub fn parse_cmdline(cmdline: &str) -> impl Iterator<Item = &str> {
    let mut args = Vec::new();
    let mut arg_start: Option<usize> = None;
    let mut in_quotes = false;
    let mut previous_char = ' ';

    for (i, c) in cmdline.char_indices() {
        if let Some(start) = arg_start {
            match c {
                ' ' if !in_quotes => {
                    args.push(&cmdline[start..i]);
                    arg_start = None;
                }
                '"' if previous_char != '\\' => {
                    in_quotes = !in_quotes;
                }
                _ => {}
            }
        } else if c != ' ' {
            arg_start = Some(i);
            if c == '"' {
                in_quotes = true;
            }
        }
        previous_char = c;
    }
    if let Some(start) = arg_start {
        args.push(&cmdline[start..]);
    }
    args.into_iter()
}

impl std::fmt::Debug for Kernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kernel").finish()
    }
}

// TODO(https://fxbug.dev/380427153): move arch dependent code to `kernel/core/arch/*`.
#[cfg(target_arch = "aarch64")]
fn arm32_hwcap(cpu_feature_flags: CpuFeatureFlags) -> HwCap {
    use starnix_uapi::arch32;
    const COMPAT_ARM32_ELF_HWCAP: u32 = arch32::HWCAP_HALF
        | arch32::HWCAP_THUMB
        | arch32::HWCAP_FAST_MULT
        | arch32::HWCAP_EDSP
        | arch32::HWCAP_TLS
        | arch32::HWCAP_IDIV // == IDIVA | IDIVT.
        | arch32::HWCAP_LPAE
        | arch32::HWCAP_EVTSTRM;

    let mut hwcap = COMPAT_ARM32_ELF_HWCAP;
    let mut hwcap2 = 0;
    for feature in cpu_feature_flags.iter() {
        match feature {
            CpuFeatureFlags::ARM64_FEATURE_ISA_ASIMD => hwcap |= arch32::HWCAP_NEON,
            CpuFeatureFlags::ARM64_FEATURE_ISA_AES => hwcap2 |= arch32::HWCAP2_AES,
            CpuFeatureFlags::ARM64_FEATURE_ISA_PMULL => hwcap2 |= arch32::HWCAP2_PMULL,
            CpuFeatureFlags::ARM64_FEATURE_ISA_SHA1 => hwcap2 |= arch32::HWCAP2_SHA1,
            CpuFeatureFlags::ARM64_FEATURE_ISA_SHA256 => hwcap2 |= arch32::HWCAP2_SHA2,
            CpuFeatureFlags::ARM64_FEATURE_ISA_CRC32 => hwcap2 |= arch32::HWCAP2_CRC32,
            CpuFeatureFlags::ARM64_FEATURE_ISA_I8MM => hwcap |= arch32::HWCAP_I8MM,
            CpuFeatureFlags::ARM64_FEATURE_ISA_FHM => hwcap |= arch32::HWCAP_ASIMDFHM,
            CpuFeatureFlags::ARM64_FEATURE_ISA_DP => hwcap |= arch32::HWCAP_ASIMDDP,
            CpuFeatureFlags::ARM64_FEATURE_ISA_FP => {
                hwcap |= arch32::HWCAP_VFP | arch32::HWCAP_VFPv3 | arch32::HWCAP_VFPv4
            }
            _ => {}
        }
    }
    HwCap { hwcap, hwcap2 }
}

#[cfg(target_arch = "aarch64")]
fn arm64_hwcap(cpu_feature_flags: CpuFeatureFlags) -> HwCap {
    // See https://docs.kernel.org/arch/arm64/elf_hwcaps.html for details.
    use starnix_uapi;
    let mut hwcap = 0;
    let mut hwcap2 = 0;

    for feature in cpu_feature_flags.iter() {
        match feature {
            CpuFeatureFlags::ARM64_FEATURE_ISA_FP => hwcap |= starnix_uapi::HWCAP_FP,
            CpuFeatureFlags::ARM64_FEATURE_ISA_ASIMD => hwcap |= starnix_uapi::HWCAP_ASIMD,
            CpuFeatureFlags::ARM64_FEATURE_ISA_AES => hwcap |= starnix_uapi::HWCAP_AES,
            CpuFeatureFlags::ARM64_FEATURE_ISA_PMULL => hwcap |= starnix_uapi::HWCAP_PMULL,
            CpuFeatureFlags::ARM64_FEATURE_ISA_SHA1 => hwcap |= starnix_uapi::HWCAP_SHA1,
            CpuFeatureFlags::ARM64_FEATURE_ISA_SHA256 => hwcap |= starnix_uapi::HWCAP_SHA2,
            CpuFeatureFlags::ARM64_FEATURE_ISA_CRC32 => hwcap |= starnix_uapi::HWCAP_CRC32,
            CpuFeatureFlags::ARM64_FEATURE_ISA_I8MM => hwcap2 |= starnix_uapi::HWCAP2_I8MM,
            CpuFeatureFlags::ARM64_FEATURE_ISA_FHM => hwcap |= starnix_uapi::HWCAP_ASIMDFHM,
            CpuFeatureFlags::ARM64_FEATURE_ISA_DP => hwcap |= starnix_uapi::HWCAP_ASIMDDP,
            CpuFeatureFlags::ARM64_FEATURE_ISA_SM3 => hwcap |= starnix_uapi::HWCAP_SM3,
            CpuFeatureFlags::ARM64_FEATURE_ISA_SM4 => hwcap |= starnix_uapi::HWCAP_SM4,
            CpuFeatureFlags::ARM64_FEATURE_ISA_SHA3 => hwcap |= starnix_uapi::HWCAP_SHA3,
            CpuFeatureFlags::ARM64_FEATURE_ISA_SHA512 => hwcap |= starnix_uapi::HWCAP_SHA512,
            CpuFeatureFlags::ARM64_FEATURE_ISA_ATOMICS => hwcap |= starnix_uapi::HWCAP_ATOMICS,
            CpuFeatureFlags::ARM64_FEATURE_ISA_RDM => hwcap |= starnix_uapi::HWCAP_ASIMDRDM,
            CpuFeatureFlags::ARM64_FEATURE_ISA_TS => hwcap |= starnix_uapi::HWCAP_FLAGM,
            CpuFeatureFlags::ARM64_FEATURE_ISA_DPB => hwcap |= starnix_uapi::HWCAP_DCPOP,
            CpuFeatureFlags::ARM64_FEATURE_ISA_RNDR => hwcap2 |= starnix_uapi::HWCAP2_RNG,
            _ => {}
        }
    }
    HwCap { hwcap, hwcap2 }
}

impl HwCaps {
    #[cfg(target_arch = "aarch64")]
    pub fn from_cpu_feature_flags(cpu_feature_flags: CpuFeatureFlags) -> Self {
        Self { arch32: arm32_hwcap(cpu_feature_flags), arch64: arm64_hwcap(cpu_feature_flags) }
    }

    #[cfg(not(target_arch = "aarch64"))]
    pub fn from_cpu_feature_flags(_cpu_feature_flags: CpuFeatureFlags) -> Self {
        Self { arch64: HwCap::default() }
    }
}

#[cfg(test)]
mod test {
    use super::parse_cmdline;

    #[test]
    fn test_parse_cmdline() {
        let cmdline =
            r#"first second=third "fourth fifth" sixth="seventh eighth" "ninth\" tenth" eleventh"#;
        let expected = vec![
            "first",
            "second=third",
            "\"fourth fifth\"",
            "sixth=\"seventh eighth\"",
            "\"ninth\\\" tenth\"",
            "eleventh",
        ];
        assert_eq!(parse_cmdline(cmdline).collect::<Vec<_>>(), expected);
    }
}
