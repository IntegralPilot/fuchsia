// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This file implements control group hierarchy.
//!
//! There is no support for actual resource constraints, or any operations outside of adding tasks
//! to a control group (for the duration of their lifetime).

use crate::signals::{SignalInfo, send_freeze_signal};
use crate::task::waiter::WaiterOptions;
use crate::task::{Kernel, Pid, ThreadGroup, WaitQueue, Waiter};
use crate::vfs::{FsStr, FsString, PathBuilder};
use starnix_logging::{CATEGORY_STARNIX, log_warn, track_stub};
use starnix_sync::{
    CgroupChildrenLock, CgroupPidTableLock, CgroupStateLock, CgroupV1Level, LockDepGuard,
    LockDepMutex, allow_subclass,
};
use starnix_uapi::errors::Errno;
use starnix_uapi::signals::SIGKILL;
use starnix_uapi::{errno, error, pid_t};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, btree_map, hash_map};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use crate::signals::KernelSignal;
use zx;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ControllerType {
    Cpu,
    Cpuacct,
    Cpuset,
    Memory,
    Freezer,
    Blkio,
}

impl ControllerType {
    pub const ALL: [Self; 6] =
        [Self::Cpu, Self::Cpuacct, Self::Cpuset, Self::Memory, Self::Freezer, Self::Blkio];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Cpuacct => "cpuacct",
            Self::Cpuset => "cpuset",
            Self::Memory => "memory",
            Self::Freezer => "freezer",
            Self::Blkio => "blkio",
        }
    }
}

impl std::str::FromStr for ControllerType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "cpu" => Ok(Self::Cpu),
            "cpuacct" => Ok(Self::Cpuacct),
            "cpuset" => Ok(Self::Cpuset),
            "memory" => Ok(Self::Memory),
            "freezer" => Ok(Self::Freezer),
            "blkio" => Ok(Self::Blkio),
            _ => Err(()),
        }
    }
}

impl std::fmt::Display for ControllerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CgroupV1Key {
    pub controllers: BTreeSet<ControllerType>,
    pub name: Option<String>,
}

#[derive(Default, Debug)]
pub struct CgroupV1State {
    // TODO(https://fxbug.dev/401298305): Support removing cgroup hierarchies when they are
    // unmounted and no longer have any tasks or child cgroups. Currently, they are kept alive
    // indefinitely by these Arc references.
    /// Maps controller to its active hierarchy.
    pub controllers: HashMap<ControllerType, Arc<CgroupRoot>>,
    /// Maps named hierarchies.
    pub named: HashMap<String, Arc<CgroupRoot>>,
    /// List of all unique hierarchies, naturally sorted.
    pub hierarchies: BTreeMap<CgroupV1Key, Arc<CgroupRoot>>,
    /// The next ID to assign to a cgroup v1 hierarchy.
    next_hierarchy_id: u32,
}

/// All cgroups of the kernel. There is a single cgroup v2 hierarchy, and one-or-more cgroup v1
/// hierarchies.
#[derive(Debug)]
pub struct KernelCgroups {
    /// The single cgroup v2 hierarchy.
    pub cgroup2: Arc<CgroupRoot>,
    /// The cgroup v1 hierarchies state, protected by a lockdep mutex.
    pub cgroup1: LockDepMutex<CgroupV1State, CgroupV1Level>,
}

impl KernelCgroups {
    /// Returns a locked `CgroupPidTable`, which guarantees that processes would not move in this
    /// cgroup hierarchy until the lock is freed.
    ///
    /// Note: Mutex dependency graph:
    ///
    /// `KernelPidTable` -> `CgroupRootPidTable` -> `CgroupState` -> `ThreadGroupState`
    pub fn lock_cgroup2_pid_table(&self) -> LockDepGuard<'_, CgroupPidTable> {
        self.cgroup2.pid_table.lock()
    }

    pub fn get_or_create_cgroup1(
        &self,
        controllers: &BTreeSet<ControllerType>,
        name: Option<&str>,
    ) -> Result<Arc<CgroupRoot>, Errno> {
        let mut cgroup1 = self.cgroup1.lock();

        let key = CgroupV1Key { controllers: controllers.clone(), name: name.map(String::from) };

        if let Some(root) = cgroup1.hierarchies.get(&key) {
            return Ok(root.clone());
        }

        for c in controllers {
            if cgroup1.controllers.contains_key(c) {
                return error!(EBUSY);
            }
        }

        if let Some(n) = name {
            if cgroup1.named.contains_key(n) {
                return error!(EBUSY);
            }
        }

        cgroup1.next_hierarchy_id += 1;
        let hierarchy_id = cgroup1.next_hierarchy_id;
        let root = CgroupRoot::new(hierarchy_id, controllers.clone());
        cgroup1.hierarchies.insert(key, root.clone());
        for c in controllers {
            cgroup1.controllers.insert(*c, root.clone());
        }
        if let Some(n) = name {
            cgroup1.named.insert(n.to_string(), root.clone());
        }

        Ok(root)
    }

    pub fn get_cgroup1(&self, controller: ControllerType, pid: &Pid) -> Option<Weak<Cgroup>> {
        let cgroup1 = self.cgroup1.lock();
        let root = cgroup1.controllers.get(&controller)?;
        root.get_cgroup(pid)
    }
}

impl Default for KernelCgroups {
    fn default() -> Self {
        Self {
            cgroup2: CgroupRoot::new(0, ControllerType::ALL.iter().copied().collect()),
            cgroup1: Default::default(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FreezerState {
    Thawed,
    Frozen,
}

impl Default for FreezerState {
    fn default() -> Self {
        FreezerState::Thawed
    }
}

impl std::fmt::Display for FreezerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FreezerState::Frozen => write!(f, "1"),
            FreezerState::Thawed => write!(f, "0"),
        }
    }
}

#[derive(Default)]
pub struct CgroupFreezerState {
    /// Cgroups's own freezer state as set by the `cgroup.freeze` file.
    pub self_freezer_state: FreezerState,
    /// Considers both the cgroup's self freezer state as set by the `cgroup.freeze` file and
    /// the freezer state of its ancestors. A cgroup is considered frozen if either itself or any
    /// of its ancestors is frozen.
    pub effective_freezer_state: FreezerState,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CpusetControllerState {
    pub cpus: Option<Vec<u32>>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FreezerControllerState {
    pub self_freezer_state: FreezerState,
    pub inherited_freezer_state: FreezerState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerState {
    Cpuset(CpusetControllerState),
    Freezer(FreezerControllerState),
}

pub trait FreezerOps: Send + Sync + 'static {
    /// Get the freezer `self state` and `effective state`.
    fn get_freezer_state(&self) -> CgroupFreezerState;

    /// Freeze all tasks in the cgroup.
    fn freeze(&self);

    /// Thaw all tasks in the cgroup.
    fn thaw(&self);
}

pub trait CpusetOps: Send + Sync + 'static {
    /// Returns the cpuset cpus of the cgroup.
    fn cpuset_cpus(&self) -> Vec<u32>;

    /// Sets the cpuset cpus of the cgroup.
    fn set_cpuset_cpus(&self, cpus: Vec<u32>);
}

/// Common operations of all cgroups.
pub trait CgroupOps: Send + Sync + 'static {
    /// Returns the unique ID of the cgroup. ID of root cgroup is 0.
    fn id(&self) -> u64;

    /// Add a process to a cgroup. Errors if the cgroup has been deleted.
    fn add_process(&self, thread_group: &ThreadGroup) -> Result<(), Errno>;

    /// Create a new sub-cgroup as a child of this cgroup. Errors if the cgroup is deleted, or a
    /// child with `name` already exists.
    fn new_child(&self, name: &FsStr) -> Result<CgroupHandle, Errno>;

    /// Gets all children of this cgroup.
    fn get_children(&self) -> Result<Vec<CgroupHandle>, Errno>;

    /// Gets the child with `name`, errors if not found.
    fn get_child(&self, name: &FsStr) -> Result<CgroupHandle, Errno>;

    /// Remove a child from this cgroup and return it, if found. Errors if cgroup is deleted, or a
    /// child with `name` is not found.
    fn remove_child(&self, name: &FsStr) -> Result<CgroupHandle, Errno>;

    /// Return all pids that belong to this cgroup.
    fn get_pids(&self, kernel: &Kernel) -> Vec<pid_t>;

    /// Kills all processes in the cgroup and its descendants.
    fn kill(&self);

    /// Whether the cgroup or any of its descendants have any processes.
    fn is_populated(&self) -> bool;

    /// Returns freezer ops if freezer controller is supported.
    fn freezer(&self) -> Option<&dyn FreezerOps>;

    /// Returns cpuset ops if cpuset controller is supported.
    fn cpuset(&self) -> Option<&dyn CpusetOps>;
}

/// `CgroupPidTable` contains the mapping of `ThreadGroup` (by pid) to non-root cgroup.
/// If `pid` is valid but does not exist in the mapping, then it is assumed to be in the root cgroup.
#[derive(Debug, Default)]
pub struct CgroupPidTable(HashMap<Pid, Weak<Cgroup>>);
impl Deref for CgroupPidTable {
    type Target = HashMap<Pid, Weak<Cgroup>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for CgroupPidTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl CgroupPidTable {
    /// Add a newly created `ThreadGroup` to the same cgroup as its parent. Assumes that
    /// `ThreadGroup` does not have any `Task` associated with it.
    pub fn inherit_cgroup(&mut self, parent: &ThreadGroup, child: &ThreadGroup) {
        assert!(child.read().tasks_count() == 0, "threadgroup must be newly created");
        if let Some(weak_cgroup) = self.0.get(&parent.leader).cloned() {
            let Some(cgroup) = weak_cgroup.upgrade() else {
                log_warn!("ignored attempt to inherit a non-existant cgroup");
                return;
            };
            assert!(
                self.0.insert(child.leader.clone(), weak_cgroup).is_none(),
                "child pid should not exist when inheriting"
            );
            // Skip freezer propagation because the `ThreadGroup` is newly created and has no tasks.
            cgroup.state.lock().processes.insert(child.leader.clone());
        }
    }

    /// Creates a new `KernelSignal` for a new `Task`, if that `Task` is added to a frozen cgroup.
    pub fn maybe_create_freeze_signal(&self, pid: &Pid) -> Option<KernelSignal> {
        let Some(weak_cgroup) = self.0.get(pid) else {
            return None;
        };
        let Some(cgroup) = weak_cgroup.upgrade() else {
            return None;
        };
        let state = cgroup.state.lock();
        if state.get_effective_freezer_state() != FreezerState::Frozen {
            return None;
        }
        Some(KernelSignal::Freeze(state.create_freeze_waiter()))
    }

    /// Remove a `ThreadGroup` from the root cgroup pid table and from the cgroup it is in.
    pub fn remove_process(&mut self, pid: &Pid) {
        if let Some(entry) = self.remove(pid) {
            if let Some(cgroup) = entry.upgrade() {
                cgroup.state.lock().processes.remove(pid);
            }
        }
    }
}

/// `CgroupRoot` is the root of the cgroup hierarchy. The root cgroup is different from the rest of
/// the cgroups in a cgroup hierarchy (sub-cgroups of the root) in a few ways:
///
/// - The root contains all known processes on cgroup creation, and all new processes as they are
/// spawned. As such, the root cgroup reports processes belonging to it differently than its
/// sub-cgroups.
///
/// - The root does not contain resource controller interface files, as otherwise they would apply
/// to the whole system.
///
/// - The root does not own a `FsNode` as it is created and owned by the `FileSystem` instead.
#[derive(Debug)]
pub struct CgroupRoot {
    /// The ID of this hierarchy. 0 is reserved for cgroup v2.
    pub hierarchy_id: u32,

    /// Controllers supported by this hierarchy.
    pub controllers: BTreeSet<ControllerType>,

    /// Look up cgroup by pid. Must be locked before child states.
    pid_table: LockDepMutex<CgroupPidTable, CgroupPidTableLock>,

    /// Sub-cgroups of this cgroup.
    children: LockDepMutex<CgroupChildren, CgroupChildrenLock>,

    /// Weak reference to self, used when creating child cgroups.
    weak_self: Weak<CgroupRoot>,

    /// Used to generate IDs for descendent Cgroups.
    next_id: AtomicU64,
}

impl CgroupRoot {
    pub fn new(hierarchy_id: u32, controllers: BTreeSet<ControllerType>) -> Arc<CgroupRoot> {
        Arc::new_cyclic(|weak_self| Self {
            hierarchy_id,
            controllers,
            pid_table: Default::default(),
            children: Default::default(),
            weak_self: weak_self.clone(),
            next_id: AtomicU64::new(1),
        })
    }

    pub fn has_controller(&self, controller: ControllerType) -> bool {
        self.controllers.contains(&controller)
    }

    fn get_next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn get_cgroup(&self, pid: &Pid) -> Option<Weak<Cgroup>> {
        self.pid_table.lock().get(pid).cloned()
    }

    pub fn get_cgroup_inspect(&self) -> fuchsia_inspect::Inspector {
        let inspector = fuchsia_inspect::Inspector::default();
        let cgroups = inspector.root();
        cgroups.record_uint("pids", self.pid_table.lock().len() as u64);
        cgroups.record_uint("count", self.children.lock().count_descendants());
        inspector
    }
}

impl CgroupOps for CgroupRoot {
    fn id(&self) -> u64 {
        0
    }

    fn add_process(&self, thread_group: &ThreadGroup) -> Result<(), Errno> {
        let mut pid_table = self.pid_table.lock();
        // If the process is currently in a child cgroup, we must remove it from that cgroup's
        // tracking. If it's not in the pid table, it is already implicitly in the root cgroup,
        // so adding it to root is a no-op.
        if let Some(entry) = pid_table.remove(&thread_group.leader) {
            if let Some(cgroup) = entry.upgrade() {
                cgroup.state.lock().remove_process(thread_group)?;
            }
        }

        let tasks = thread_group.read().tasks();
        if self.has_controller(ControllerType::Cpuset) {
            for task in &tasks {
                task.write().cpuset_path = "/".to_string();
            }
        }

        // Re-evaluate roles for all threads in the thread group.
        for task in tasks {
            if let Err(e) = task.sync_scheduler_state_to_role() {
                log_warn!("Failed to set thread role for task {}: {:?}", task.tid, e);
            }
        }

        Ok(())
    }

    fn new_child(&self, name: &FsStr) -> Result<CgroupHandle, Errno> {
        let id = self.get_next_id();
        let new_child = Cgroup::new(id, name, &self.weak_self, None);
        let mut children = self.children.lock();
        children.insert_child(name.into(), new_child)
    }

    fn get_child(&self, name: &FsStr) -> Result<CgroupHandle, Errno> {
        let children = self.children.lock();
        children.get_child(name).ok_or_else(|| errno!(ENOENT))
    }

    fn remove_child(&self, name: &FsStr) -> Result<CgroupHandle, Errno> {
        let mut children = self.children.lock();
        children.remove_child(name)
    }

    fn get_children(&self) -> Result<Vec<CgroupHandle>, Errno> {
        let children = self.children.lock();
        Ok(children.get_children())
    }

    fn get_pids(&self, kernel: &Kernel) -> Vec<pid_t> {
        let controlled_pids: HashSet<pid_t> = self
            .pid_table
            .lock()
            .keys()
            .filter_map(|v| v.get_thread_group().ok().map(|tg| tg.leader.id))
            .collect();
        let kernel_pids = kernel.pids.process_ids();
        kernel_pids.into_iter().filter(|pid| !controlled_pids.contains(pid)).collect()
    }

    fn kill(&self) {
        unreachable!("Root cgroup cannot kill its processes.");
    }

    fn is_populated(&self) -> bool {
        false
    }

    fn freezer(&self) -> Option<&dyn FreezerOps> {
        None
    }

    fn cpuset(&self) -> Option<&dyn CpusetOps> {
        if self.controllers.contains(&ControllerType::Cpuset) { Some(self) } else { None }
    }
}

impl CpusetOps for CgroupRoot {
    fn cpuset_cpus(&self) -> Vec<u32> {
        (0..zx::system_get_num_cpus()).collect()
    }

    fn set_cpuset_cpus(&self, _cpus: Vec<u32>) {}
}

#[derive(Debug, Default)]
struct CgroupChildren(BTreeMap<FsString, CgroupHandle>);
impl CgroupChildren {
    fn insert_child(&mut self, name: FsString, child: CgroupHandle) -> Result<CgroupHandle, Errno> {
        let btree_map::Entry::Vacant(child_entry) = self.0.entry(name) else {
            return error!(EEXIST);
        };
        Ok(child_entry.insert(child).clone())
    }

    fn remove_child(&mut self, name: &FsStr) -> Result<CgroupHandle, Errno> {
        let btree_map::Entry::Occupied(child_entry) = self.0.entry(name.into()) else {
            return error!(ENOENT);
        };
        let child = child_entry.get();

        // This allow_subclass is safe because the lock is being acquired
        // in a strictly top-down traversal of the Cgroup tree (from parent
        // to child), so no lock ordering cycles can be formed.
        let _token = allow_subclass();
        let mut child_state = child.state.lock();
        assert!(!child_state.deleted, "child cannot be deleted");

        child_state.update_processes();
        if !child_state.processes.is_empty() {
            return error!(EBUSY);
        }
        if !child_state.children.is_empty() {
            return error!(EBUSY);
        }

        child_state.deleted = true;
        drop(child_state);

        Ok(child_entry.remove())
    }

    fn get_child(&self, name: &FsStr) -> Option<CgroupHandle> {
        self.0.get(name).cloned()
    }

    fn get_children(&self) -> Vec<CgroupHandle> {
        self.0.values().cloned().collect()
    }

    fn count_descendants(&self) -> u64 {
        self.0
            .values()
            .map(|child| {
                1 + {
                    // This allow_subclass is safe because the lock is being acquired
                    // in a strictly top-down traversal of the Cgroup tree (from parent
                    // to child), so no lock ordering cycles can be formed.
                    let _token = allow_subclass();
                    child.count_descendants()
                }
            })
            .sum()
    }
}

impl Deref for CgroupChildren {
    type Target = BTreeMap<FsString, CgroupHandle>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Default)]
struct CgroupState {
    /// Subgroups of this control group.
    children: CgroupChildren,

    /// The tasks that are part of this control group.
    processes: HashSet<Pid>,

    /// If true, can no longer add children or tasks.
    deleted: bool,

    /// Wait queue to thaw all blocked tasks in this cgroup.
    wait_queue: WaitQueue,

    /// Controller-specific state.
    controllers: HashMap<ControllerType, ControllerState>,
}

impl CgroupState {
    fn freezer(&self) -> Option<&FreezerControllerState> {
        match self.controllers.get(&ControllerType::Freezer) {
            Some(ControllerState::Freezer(state)) => Some(state),
            _ => None,
        }
    }

    fn freezer_mut(&mut self) -> Option<&mut FreezerControllerState> {
        match self.controllers.get_mut(&ControllerType::Freezer) {
            Some(ControllerState::Freezer(state)) => Some(state),
            _ => None,
        }
    }

    fn cpuset(&self) -> Option<&CpusetControllerState> {
        match self.controllers.get(&ControllerType::Cpuset) {
            Some(ControllerState::Cpuset(state)) => Some(state),
            _ => None,
        }
    }

    fn cpuset_mut(&mut self) -> Option<&mut CpusetControllerState> {
        match self.controllers.get_mut(&ControllerType::Cpuset) {
            Some(ControllerState::Cpuset(state)) => Some(state),
            _ => None,
        }
    }

    /// Creates a new Waiter that subscribes to the Cgroup's freezer WaitQueue. This `Waiter` can be
    /// sent as a part of a `KernelSignal::Freeze` to freeze a `Task`.
    fn create_freeze_waiter(&self) -> Waiter {
        let waiter = Waiter::with_options(WaiterOptions::IGNORE_SIGNALS);
        self.wait_queue.wait_async(&waiter);
        waiter
    }

    // Goes through `processes` and remove processes that are no longer alive.
    fn update_processes(&mut self) {
        self.processes.retain(|thread_group| {
            let Ok(thread_group) = thread_group.get_thread_group() else {
                return false;
            };
            let running = thread_group.read().is_running();
            running
        });
    }

    fn freeze_thread_group(&self, thread_group: &ThreadGroup) {
        let tasks = thread_group.read().tasks();
        for task in tasks {
            send_freeze_signal(&task, self.create_freeze_waiter())
                .expect("sending freeze signal should not fail");
        }
    }

    fn thaw_thread_group(&self, thread_group: &ThreadGroup) {
        let tasks = thread_group.read().tasks();
        for task in tasks {
            task.write().thaw();
            task.interrupt();
        }
    }

    fn get_effective_freezer_state(&self) -> FreezerState {
        if let Some(freezer) = self.freezer() {
            std::cmp::max(freezer.self_freezer_state, freezer.inherited_freezer_state)
        } else {
            FreezerState::Thawed
        }
    }

    fn add_process(&mut self, thread_group: &ThreadGroup) -> Result<(), Errno> {
        if self.deleted {
            return error!(ENOENT);
        }
        self.processes.insert(thread_group.leader.clone());

        if self.get_effective_freezer_state() == FreezerState::Frozen {
            self.freeze_thread_group(&thread_group);
        }
        Ok(())
    }

    fn remove_process(&mut self, thread_group: &ThreadGroup) -> Result<(), Errno> {
        if self.deleted {
            return error!(ENOENT);
        }
        self.processes.remove(&thread_group.leader);

        if self.get_effective_freezer_state() == FreezerState::Frozen {
            self.thaw_thread_group(thread_group);
        }
        Ok(())
    }

    fn propagate_freeze(&mut self, inherited_freezer_state: FreezerState) {
        let prev_effective_freezer_state = self.get_effective_freezer_state();
        if let Some(freezer) = self.freezer_mut() {
            freezer.inherited_freezer_state = inherited_freezer_state;
        }
        if prev_effective_freezer_state == FreezerState::Frozen {
            return;
        }

        for pid in self.processes.iter() {
            let Ok(thread_group) = pid.get_thread_group() else {
                continue;
            };
            self.freeze_thread_group(&thread_group);
        }

        // Freeze all children cgroups while holding self state lock
        for child in self.children.get_children() {
            // This allow_subclass is safe because the lock is being acquired
            // in a strictly top-down traversal of the Cgroup tree (from parent
            // to child), so no lock ordering cycles can be formed.
            let _token = allow_subclass();
            child.state.lock().propagate_freeze(FreezerState::Frozen);
        }
    }

    fn propagate_thaw(&mut self, inherited_freezer_state: FreezerState) {
        if let Some(freezer) = self.freezer_mut() {
            freezer.inherited_freezer_state = inherited_freezer_state;
        }
        if self.get_effective_freezer_state() == FreezerState::Thawed {
            self.wait_queue.notify_all();
            for child in self.children.get_children() {
                // This allow_subclass is safe because the lock is being acquired
                // in a strictly top-down traversal of the Cgroup tree (from parent
                // to child), so no lock ordering cycles can be formed.
                let _token = allow_subclass();
                child.state.lock().propagate_thaw(FreezerState::Thawed);
            }
        }
    }

    fn propagate_kill(&self) {
        for pid in self.processes.iter() {
            let Ok(thread_group) = pid.get_thread_group() else {
                continue;
            };
            thread_group.write().send_signal(SignalInfo::kernel(SIGKILL));
        }

        // Recursively lock and kill children cgroups' processes.
        for child in self.children.get_children() {
            // This allow_subclass is safe because the lock is being acquired
            // in a strictly top-down traversal of the Cgroup tree (from parent
            // to child), so no lock ordering cycles can be formed.
            let _token = allow_subclass();
            child.state.lock().propagate_kill();
        }
    }
}

/// `Cgroup` is a non-root cgroup in a cgroup hierarchy, and can have other `Cgroup`s as children.
#[derive(Debug)]
pub struct Cgroup {
    root: Weak<CgroupRoot>,

    /// ID of the cgroup.
    id: u64,

    /// Name of the cgroup.
    name: FsString,

    /// Weak reference to its parent cgroup, `None` if direct descendent of the root cgroup.
    /// This field is useful in implementing features that only apply to non-root cgroups.
    parent: Option<Weak<Cgroup>>,

    /// Internal state of the Cgroup.
    state: LockDepMutex<CgroupState, CgroupStateLock>,

    weak_self: Weak<Cgroup>,
}
pub type CgroupHandle = Arc<Cgroup>;

/// Returns the path from the root to this `cgroup`.
pub fn path_from_root(weak_cgroup: Option<Weak<Cgroup>>) -> Result<FsString, Errno> {
    let cgroup = match weak_cgroup {
        Some(weak_cgroup) => Weak::upgrade(&weak_cgroup).ok_or_else(|| errno!(ENODEV))?,
        None => return Ok("/".into()),
    };
    let mut path = PathBuilder::new();
    let mut current = Some(cgroup);
    while let Some(cgroup) = current {
        path.prepend_element(cgroup.name());
        current = cgroup.parent()?;
    }
    Ok(path.build_absolute())
}

impl Cgroup {
    pub fn new(
        id: u64,
        name: &FsStr,
        root: &Weak<CgroupRoot>,
        parent: Option<Weak<Cgroup>>,
    ) -> CgroupHandle {
        let root_shared = root.upgrade().expect("root must exist");
        let mut controllers = HashMap::new();
        for controller in &root_shared.controllers {
            match controller {
                ControllerType::Cpuset => {
                    controllers.insert(
                        *controller,
                        ControllerState::Cpuset(CpusetControllerState::default()),
                    );
                }
                ControllerType::Freezer => {
                    controllers.insert(
                        *controller,
                        ControllerState::Freezer(FreezerControllerState::default()),
                    );
                }
                _ => {}
            }
        }

        Arc::new_cyclic(|weak| Self {
            id,
            root: root.clone(),
            name: name.to_owned(),
            parent,
            state: LockDepMutex::new(CgroupState {
                children: Default::default(),
                processes: Default::default(),
                deleted: false,
                wait_queue: Default::default(),
                controllers,
            }),
            weak_self: weak.clone(),
        })
    }

    pub fn name(&self) -> &FsStr {
        self.name.as_ref()
    }

    fn root(&self) -> Result<Arc<CgroupRoot>, Errno> {
        self.root.upgrade().ok_or_else(|| errno!(ENODEV))
    }

    /// Returns the upgraded parent cgroup, or `Ok(None)` if cgroup is a direct desendent of root.
    /// Errors if parent node is no longer around.
    fn parent(&self) -> Result<Option<CgroupHandle>, Errno> {
        self.parent.as_ref().map(|weak| weak.upgrade().ok_or_else(|| errno!(ENODEV))).transpose()
    }

    fn count_descendants(&self) -> u64 {
        self.state.lock().children.count_descendants()
    }

    fn is_controller_supported(&self, controller: ControllerType) -> bool {
        self.root().map(|r| r.controllers.contains(&controller)).unwrap_or(false)
    }

    fn cpuset_path(&self, root: &CgroupRoot) -> Option<String> {
        if !root.has_controller(ControllerType::Cpuset) {
            return None;
        }
        let bytes = path_from_root(Some(self.weak_self.clone())).ok()?;
        std::str::from_utf8(&bytes).ok().map(|s| s.to_string())
    }
}

impl CgroupOps for Cgroup {
    fn id(&self) -> u64 {
        self.id
    }

    fn add_process(&self, thread_group: &ThreadGroup) -> Result<(), Errno> {
        let root = self.root()?;
        let mut pid_table = root.pid_table.lock();
        match pid_table.entry(thread_group.leader.clone()) {
            hash_map::Entry::Occupied(mut entry) => {
                // Check if thread_group is already in the current cgroup. Linux does not return an error if
                // it already exists.
                if std::ptr::eq(self, entry.get().as_ptr()) {
                    return Ok(());
                }

                // If thread_group is in another cgroup, we need to remove it first.
                track_stub!(TODO("https://fxbug.dev/383374687"), "check permissions");
                if let Some(other_cgroup) = entry.get().upgrade() {
                    other_cgroup.state.lock().remove_process(thread_group)?;
                }

                self.state.lock().add_process(thread_group)?;
                entry.insert(self.weak_self.clone());
            }
            hash_map::Entry::Vacant(entry) => {
                self.state.lock().add_process(thread_group)?;
                entry.insert(self.weak_self.clone());
            }
        }

        let tasks = thread_group.read().tasks();
        if let Some(cpuset_path) = self.cpuset_path(&root) {
            for task in &tasks {
                task.write().cpuset_path = cpuset_path.clone();
            }
        }

        // Re-evaluate roles for all threads in the thread group.
        for task in tasks {
            if let Err(e) = task.sync_scheduler_state_to_role() {
                log_warn!("Failed to set thread role for task {}: {:?}", task.tid, e);
            }
        }

        Ok(())
    }

    fn new_child(&self, name: &FsStr) -> Result<CgroupHandle, Errno> {
        let id = self.root()?.get_next_id();
        let new_child = Cgroup::new(id, name, &self.root, Some(self.weak_self.clone()));
        let mut state = self.state.lock();
        if state.deleted {
            return error!(ENOENT);
        }
        // New child should inherit the effective freezer state of the current cgroup.
        // This allow_subclass is safe because the lock is being acquired
        // in a strictly top-down traversal of the Cgroup tree (from parent
        // to child), so no lock ordering cycles can be formed.
        let effective_freezer = state.get_effective_freezer_state();
        let _token = allow_subclass();
        if let Some(freezer) = new_child.state.lock().freezer_mut() {
            freezer.inherited_freezer_state = effective_freezer;
        }
        state.children.insert_child(name.into(), new_child)
    }

    fn get_child(&self, name: &FsStr) -> Result<CgroupHandle, Errno> {
        let state = self.state.lock();
        state.children.get_child(name).ok_or_else(|| errno!(ENOENT))
    }

    fn remove_child(&self, name: &FsStr) -> Result<CgroupHandle, Errno> {
        let mut state = self.state.lock();
        if state.deleted {
            return error!(ENOENT);
        }
        state.children.remove_child(name)
    }

    fn get_children(&self) -> Result<Vec<CgroupHandle>, Errno> {
        let state = self.state.lock();
        if state.deleted {
            return error!(ENOENT);
        }
        Ok(state.children.get_children())
    }

    fn get_pids(&self, _kernel: &Kernel) -> Vec<pid_t> {
        let mut state = self.state.lock();
        state.update_processes();
        state
            .processes
            .iter()
            .filter_map(|v| v.get_thread_group().ok().map(|tg| tg.leader.id))
            .collect()
    }

    fn kill(&self) {
        fuchsia_trace::duration!(CATEGORY_STARNIX, "CgroupKill");
        let state = self.state.lock();
        state.propagate_kill();
    }

    fn is_populated(&self) -> bool {
        let mut state = self.state.lock();
        if state.deleted {
            return false;
        }
        state.update_processes();
        if !state.processes.is_empty() {
            return true;
        }

        state.children.get_children().into_iter().any(|child| {
            // This allow_subclass is safe because the lock is being acquired
            // in a strictly top-down traversal of the Cgroup tree (from parent
            // to child), so no lock ordering cycles can be formed.
            let _token = allow_subclass();
            child.is_populated()
        })
    }

    fn freezer(&self) -> Option<&dyn FreezerOps> {
        if self.is_controller_supported(ControllerType::Freezer) { Some(self) } else { None }
    }

    fn cpuset(&self) -> Option<&dyn CpusetOps> {
        if self.is_controller_supported(ControllerType::Cpuset) { Some(self) } else { None }
    }
}

impl FreezerOps for Cgroup {
    fn get_freezer_state(&self) -> CgroupFreezerState {
        let state = self.state.lock();
        let self_freezer_state = state.freezer().map(|f| f.self_freezer_state).unwrap_or_default();
        CgroupFreezerState {
            self_freezer_state,
            effective_freezer_state: state.get_effective_freezer_state(),
        }
    }

    fn freeze(&self) {
        fuchsia_trace::duration!(CATEGORY_STARNIX, "CgroupFreeze");
        let mut state = self.state.lock();
        let inherited_freezer_state =
            state.freezer().map(|f| f.inherited_freezer_state).unwrap_or_default();
        state.propagate_freeze(inherited_freezer_state);
        if let Some(freezer) = state.freezer_mut() {
            freezer.self_freezer_state = FreezerState::Frozen;
        }
    }

    fn thaw(&self) {
        fuchsia_trace::duration!(CATEGORY_STARNIX, "CgroupThaw");
        let mut state = self.state.lock();
        if let Some(freezer) = state.freezer_mut() {
            freezer.self_freezer_state = FreezerState::Thawed;
        }
        let inherited_freezer_state =
            state.freezer().map(|f| f.inherited_freezer_state).unwrap_or_default();
        state.propagate_thaw(inherited_freezer_state);
    }
}

impl CpusetOps for Cgroup {
    fn cpuset_cpus(&self) -> Vec<u32> {
        self.state
            .lock()
            .cpuset()
            .and_then(|c| c.cpus.clone())
            .unwrap_or_else(|| (0..zx::system_get_num_cpus()).collect())
    }

    fn set_cpuset_cpus(&self, cpus: Vec<u32>) {
        // TODO(b/322255433): Translate the cpuset logic into Zircon
        let mut state = self.state.lock();
        if let Some(cpuset) = state.cpuset_mut() {
            cpuset.cpus = Some(cpus);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing::spawn_kernel_and_run;
    use assert_matches::assert_matches;
    use starnix_uapi::signals::SIGCHLD;
    use starnix_uapi::{CLONE_SIGHAND, CLONE_THREAD, CLONE_VM};

    #[::fuchsia::test]
    async fn cgroup_path_from_root() {
        spawn_kernel_and_run(async |_| {
            let root = CgroupRoot::new(0, BTreeSet::new());

            let test_cgroup =
                root.new_child("test".into()).expect("new_child on root cgroup succeeds");
            let child_cgroup = test_cgroup
                .new_child("child".into())
                .expect("new_child on non-root cgroup succeeds");

            assert_eq!(path_from_root(Some(Arc::downgrade(&test_cgroup))), Ok("/test".into()));
            assert_eq!(
                path_from_root(Some(Arc::downgrade(&child_cgroup))),
                Ok("/test/child".into())
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn cgroup_clone_task_in_frozen_cgroup() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let root = &kernel.cgroups.cgroup2;
            let cgroup = root.new_child("test".into()).expect("new_child on root cgroup succeeds");

            let process = current_task.clone_task_for_test(0, Some(SIGCHLD));
            cgroup.add_process(process.thread_group()).expect("add process to cgroup");
            cgroup.freeze();
            assert_eq!(cgroup.get_pids(&kernel).first(), Some(process.get_pid()).as_ref());
            assert_eq!(root.get_cgroup(&process.pid).unwrap().as_ptr(), Arc::as_ptr(&cgroup));

            let thread = process.clone_task_for_test(
                (CLONE_THREAD | CLONE_SIGHAND | CLONE_VM) as u64,
                Some(SIGCHLD),
            );

            let thread_state = thread.read();
            let kernel_signals = thread_state.kernel_signals_for_test();
            assert_matches!(kernel_signals.front(), Some(KernelSignal::Freeze(_)));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn cgroup_tg_release_removes_pid() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let root = &kernel.cgroups.cgroup2;
            let cgroup = root.new_child("test".into()).expect("new_child on root cgroup succeeds");

            let process = current_task.clone_task_for_test(0, Some(SIGCHLD));
            cgroup.add_process(process.thread_group()).expect("add process to cgroup");

            assert_eq!(root.get_cgroup(&process.pid).unwrap().as_ptr(), Arc::as_ptr(&cgroup));

            // Drop the process to release it.
            drop(process);

            // Verify that the process is removed from the cgroup pid table.
            assert!(root.pid_table.lock().is_empty());
        })
        .await;
    }
}
