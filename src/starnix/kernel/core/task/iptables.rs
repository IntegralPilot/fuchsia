// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bpf::fs::resolve_pinned_bpf_object;
use crate::bpf::program::{Program, ProgramHandle};
use crate::security;
use crate::task::CurrentTask;
use crate::vfs::socket::iptables_utils::{
    self, Ip, IpTableParseError, IptReplaceContext, TableId, string_to_ascii_buffer,
};
use crate::vfs::socket::{SockOptValue, SocketDomain, SocketHandle, SocketType};
use bstr::BString;
use fidl_fuchsia_ebpf as febpf;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_ext::sync::Controller;
use fidl_fuchsia_net_filter_ext::{
    Change, CommitError, ControllerId, PushChangesError, RegisterEbpfProgramError,
};
use fuchsia_component::client::connect_to_protocol_sync;
use itertools::Itertools;
use starnix_logging::{log_error, log_warn, track_stub};
use starnix_sync::{KernelIpTables, LockDepRwLock};
use starnix_uapi::auth::CAP_NET_ADMIN;
use starnix_uapi::errors::Errno;
use starnix_uapi::iptables_flags::NfIpHooks;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{
    IP6T_SO_GET_ENTRIES, IP6T_SO_GET_INFO, IP6T_SO_GET_REVISION_MATCH, IP6T_SO_GET_REVISION_TARGET,
    IPT_SO_GET_ENTRIES, IPT_SO_GET_INFO, IPT_SO_GET_REVISION_MATCH, IPT_SO_GET_REVISION_TARGET,
    IPT_SO_SET_ADD_COUNTERS, IPT_SO_SET_REPLACE, SOL_IP, SOL_IPV6, errno, error, ip6t_entry,
    ip6t_get_entries, ip6t_getinfo, ipt_entry, ipt_get_entries, ipt_getinfo,
    nf_inet_hooks_NF_INET_NUMHOOKS, xt_counters, xt_counters_info,
    xt_entry_target__bindgen_ty_1__bindgen_ty_1 as xt_entry_target, xt_error_target,
    xt_get_revision, xt_standard_target,
};
use static_assertions::const_assert_eq;
use std::collections::HashMap;
use std::mem::size_of;
use std::ops::{Deref as _, Index, IndexMut};
use zerocopy::{FromBytes, IntoBytes};

const NAMESPACE_ID_PREFIX: &str = "starnix";

const IPT_ENTRY_SIZE: u16 = size_of::<ipt_entry>() as u16;
const IP6T_ENTRY_SIZE: u16 = size_of::<ip6t_entry>() as u16;
const STANDARD_TARGET_SIZE: u16 = size_of::<xt_standard_target>() as u16;
const ERROR_TARGET_SIZE: u16 = size_of::<xt_error_target>() as u16;

// The following arrays denote where built-in chains are defined for each table. This makes it easy
// to calculate `hook_entry` and `underflow` for tables where built-in chains only have a policy and
// no other rules by scaling by the size of a standard entry.
//
// The indices correspond to [PREROUTING, INPUT, FORWARD, OUTPUT, POSTROUTING]. The first built-in
// chain has value 0, second chain has value 1, and so on. Confusingly, built-in chains that do not
// exist on a table are also denoted as 0, but this is how Linux expects these values.
const FILTER_HOOKS: [u32; 5] = [0, 0, 1, 2, 0];
const NAT_HOOKS: [u32; 5] = [0, 1, 0, 2, 3];
const MANGLE_HOOKS: [u32; 5] = [0, 1, 2, 3, 4];
const RAW_HOOKS: [u32; 5] = [0, 0, 0, 1, 0];

/// Stores information about IP packet filter rules. Used to return information for
/// IPT_SO_GET_INFO and IPT_SO_GET_ENTRIES.
#[derive(Debug, Default)]
struct IpTable {
    pub valid_hooks: u32,
    pub hook_entry: [u32; nf_inet_hooks_NF_INET_NUMHOOKS as usize],
    pub underflow: [u32; nf_inet_hooks_NF_INET_NUMHOOKS as usize],
    pub num_entries: u32,
    pub size: u32,
    pub entries: Vec<u8>,
    pub num_counters: u32,
    pub counters: Vec<xt_counters>,
}

impl IpTable {
    fn accept_policy_v4() -> Vec<u8> {
        [
            ipt_entry {
                target_offset: IPT_ENTRY_SIZE,
                next_offset: IPT_ENTRY_SIZE + STANDARD_TARGET_SIZE,
                ..Default::default()
            }
            .as_bytes(),
            xt_entry_target { target_size: STANDARD_TARGET_SIZE, ..Default::default() }.as_bytes(),
            iptables_utils::VerdictWithPadding {
                verdict: iptables_utils::VERDICT_ACCEPT,
                ..Default::default()
            }
            .as_bytes(),
        ]
        .concat()
    }

    fn accept_policy_v6() -> Vec<u8> {
        [
            ip6t_entry {
                target_offset: IP6T_ENTRY_SIZE,
                next_offset: IP6T_ENTRY_SIZE + STANDARD_TARGET_SIZE,
                ..Default::default()
            }
            .as_bytes(),
            xt_entry_target { target_size: STANDARD_TARGET_SIZE, ..Default::default() }.as_bytes(),
            iptables_utils::VerdictWithPadding {
                verdict: iptables_utils::VERDICT_ACCEPT,
                ..Default::default()
            }
            .as_bytes(),
        ]
        .concat()
    }

    fn end_of_input_v4() -> Vec<u8> {
        [
            ipt_entry {
                target_offset: IPT_ENTRY_SIZE,
                next_offset: IPT_ENTRY_SIZE + ERROR_TARGET_SIZE,
                ..Default::default()
            }
            .as_bytes(),
            xt_entry_target {
                target_size: ERROR_TARGET_SIZE,
                name: string_to_ascii_buffer("ERROR").expect("convert \"ERROR\" to ASCII"),
                revision: 0,
            }
            .as_bytes(),
            iptables_utils::ErrorNameWithPadding {
                errorname: string_to_ascii_buffer("ERROR").expect("convert \"ERROR\" to ASCII"),
                ..Default::default()
            }
            .as_bytes(),
        ]
        .concat()
    }

    fn end_of_input_v6() -> Vec<u8> {
        [
            ip6t_entry {
                target_offset: IP6T_ENTRY_SIZE,
                next_offset: IP6T_ENTRY_SIZE + ERROR_TARGET_SIZE,
                ..Default::default()
            }
            .as_bytes(),
            xt_entry_target {
                target_size: ERROR_TARGET_SIZE,
                name: string_to_ascii_buffer("ERROR").expect("convert \"ERROR\" to ASCII"),
                revision: 0,
            }
            .as_bytes(),
            iptables_utils::ErrorNameWithPadding {
                errorname: string_to_ascii_buffer("ERROR").expect("convert \"ERROR\" to ASCII"),
                ..Default::default()
            }
            .as_bytes(),
        ]
        .concat()
    }

    fn default_ipv4_nat_table() -> Self {
        let hook_entry = NAT_HOOKS.map(|n| n * u32::from(IPT_ENTRY_SIZE + STANDARD_TARGET_SIZE));
        let accept_policy = Self::accept_policy_v4();
        let entries = [
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            Self::end_of_input_v4().as_slice(),
        ]
        .concat();
        Self {
            valid_hooks: NfIpHooks::NAT.bits(),
            hook_entry,
            underflow: hook_entry,
            num_entries: 5,
            size: entries.len() as u32,
            entries,
            ..Default::default()
        }
    }

    fn default_ipv6_nat_table() -> Self {
        let hook_entry = NAT_HOOKS.map(|n| n * u32::from(IP6T_ENTRY_SIZE + STANDARD_TARGET_SIZE));
        let accept_policy = Self::accept_policy_v6();
        let entries = [
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            Self::end_of_input_v6().as_slice(),
        ]
        .concat();

        Self {
            valid_hooks: NfIpHooks::NAT.bits(),
            hook_entry,
            underflow: hook_entry,
            num_entries: 5,
            size: entries.len() as u32,
            entries,
            ..Default::default()
        }
    }

    fn default_ipv4_filter_table() -> Self {
        let hook_entry = FILTER_HOOKS.map(|n| n * u32::from(IPT_ENTRY_SIZE + STANDARD_TARGET_SIZE));
        let accept_policy = Self::accept_policy_v4();
        let entries = [
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            Self::end_of_input_v4().as_slice(),
        ]
        .concat();

        Self {
            valid_hooks: NfIpHooks::FILTER.bits(),
            hook_entry,
            underflow: hook_entry,
            num_entries: 4,
            size: entries.len() as u32,
            entries,
            ..Default::default()
        }
    }

    fn default_ipv6_filter_table() -> Self {
        let hook_entry =
            FILTER_HOOKS.map(|n| n * u32::from(IP6T_ENTRY_SIZE + STANDARD_TARGET_SIZE));
        let accept_policy = Self::accept_policy_v6();
        let entries = [
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            Self::end_of_input_v6().as_slice(),
        ]
        .concat();

        Self {
            valid_hooks: NfIpHooks::FILTER.bits(),
            hook_entry,
            underflow: hook_entry,
            num_entries: 4,
            size: entries.len() as u32,
            entries,
            ..Default::default()
        }
    }

    fn default_ipv4_mangle_table() -> Self {
        let hook_entry = MANGLE_HOOKS.map(|n| n * u32::from(IPT_ENTRY_SIZE + STANDARD_TARGET_SIZE));
        let accept_policy = Self::accept_policy_v4();
        let entries = [
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            Self::end_of_input_v4().as_slice(),
        ]
        .concat();

        Self {
            valid_hooks: NfIpHooks::MANGLE.bits(),
            hook_entry,
            underflow: hook_entry,
            num_entries: 6,
            size: entries.len() as u32,
            entries,
            ..Default::default()
        }
    }

    fn default_ipv6_mangle_table() -> Self {
        let hook_entry =
            MANGLE_HOOKS.map(|n| n * u32::from(IP6T_ENTRY_SIZE + STANDARD_TARGET_SIZE));
        let accept_policy = Self::accept_policy_v6();
        let entries = [
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            Self::end_of_input_v6().as_slice(),
        ]
        .concat();

        Self {
            valid_hooks: NfIpHooks::MANGLE.bits(),
            hook_entry,
            underflow: hook_entry,
            num_entries: 6,
            size: entries.len() as u32,
            entries,
            ..Default::default()
        }
    }

    fn default_ipv4_raw_table() -> Self {
        let hook_entry = RAW_HOOKS.map(|n| n * u32::from(IPT_ENTRY_SIZE + STANDARD_TARGET_SIZE));
        let accept_policy = Self::accept_policy_v4();
        let entries = [
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            Self::end_of_input_v4().as_slice(),
        ]
        .concat();

        Self {
            valid_hooks: NfIpHooks::RAW.bits(),
            hook_entry,
            underflow: hook_entry,
            num_entries: 3,
            size: entries.len() as u32,
            entries,
            ..Default::default()
        }
    }

    fn default_ipv6_raw_table() -> Self {
        let hook_entry = RAW_HOOKS.map(|n| n * u32::from(IP6T_ENTRY_SIZE + STANDARD_TARGET_SIZE));
        let accept_policy = Self::accept_policy_v6();
        let entries = [
            accept_policy.as_slice(),
            accept_policy.as_slice(),
            Self::end_of_input_v6().as_slice(),
        ]
        .concat();

        Self {
            valid_hooks: NfIpHooks::RAW.bits(),
            hook_entry,
            underflow: hook_entry,
            num_entries: 3,
            size: entries.len() as u32,
            entries,
            ..Default::default()
        }
    }
}

type IpTablesArray = [IpTable; iptables_utils::NUM_TABLES];

impl Index<TableId> for IpTablesArray {
    type Output = IpTable;

    fn index(&self, index: TableId) -> &Self::Output {
        &self[index as usize]
    }
}

impl IndexMut<TableId> for IpTablesArray {
    fn index_mut(&mut self, index: TableId) -> &mut Self::Output {
        &mut self[index as usize]
    }
}

fn default_ipv4_tables() -> IpTablesArray {
    const_assert_eq!(TableId::Filter as usize, 0);
    const_assert_eq!(TableId::Mangle as usize, 1);
    const_assert_eq!(TableId::Nat as usize, 2);
    const_assert_eq!(TableId::Raw as usize, 3);
    [
        IpTable::default_ipv4_filter_table(),
        IpTable::default_ipv4_mangle_table(),
        IpTable::default_ipv4_nat_table(),
        IpTable::default_ipv4_raw_table(),
    ]
}

fn default_ipv6_tables() -> IpTablesArray {
    [
        IpTable::default_ipv6_filter_table(),
        IpTable::default_ipv6_mangle_table(),
        IpTable::default_ipv6_nat_table(),
        IpTable::default_ipv6_raw_table(),
    ]
}

/// Defines a set of table.
#[derive(Default, PartialEq, Eq, Copy, Clone)]
struct IpTableSet(u64);

impl IpTableSet {
    fn element_index(ip: Ip, table: TableId) -> usize {
        (ip as usize) * iptables_utils::NUM_TABLES + table as usize
    }

    fn add(&mut self, ip: Ip, table: TableId) {
        self.0 |= 1 << Self::element_index(ip, table);
    }

    fn remove(&mut self, ip: Ip, table: TableId) {
        self.0 &= !(1 << Self::element_index(ip, table));
    }

    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

// `EbpfProgramState` keeps track of where an eBPF program is currently
// installed. This ensures that we keep at least one reference to the program
// while it is being used by a filter rule.
struct EbpfProgramState {
    #[allow(dead_code)]
    program: ProgramHandle,
    tables: IpTableSet,
}

#[derive(Default)]
struct LazyController {
    controller: Option<Controller>,
}

impl LazyController {
    fn get(&mut self) -> Result<&mut Controller, Errno> {
        if self.controller.is_none() {
            let control_proxy =
                connect_to_protocol_sync::<fnet_filter::ControlMarker>().map_err(|e| {
                    log_error!("failed to connect to fuchsia.net.filter.Control: {e}");
                    errno!(EIO)
                })?;
            self.controller = Some(
                Controller::new(
                    &control_proxy,
                    &ControllerId(NAMESPACE_ID_PREFIX.to_string()),
                    zx::MonotonicInstant::INFINITE,
                )
                .map_err(|e| {
                    log_error!("failed to create filter controller: {e}");
                    errno!(EIO)
                })?,
            );
        }
        Ok(self.controller.as_mut().expect("just ensured this is Some"))
    }
}

struct IpTablesState {
    ipv4: IpTablesArray,
    ipv6: IpTablesArray,

    /// Controller to configure net filtering state.
    controller: LazyController,

    ebpf_programs: HashMap<febpf::ProgramId, EbpfProgramState>,
}

impl IpTablesState {
    fn send_changes_to_net_filter(
        &mut self,
        changes: impl Iterator<Item = Change>,
    ) -> Result<(), Errno> {
        let controller = self.controller.get()?;
        for chunk in &changes.chunks(fnet_filter::MAX_BATCH_SIZE as usize) {
            match controller.push_changes(chunk.collect(), zx::MonotonicInstant::INFINITE) {
                Ok(()) => {}
                Err(
                    e @ (PushChangesError::CallMethod(_)
                    | PushChangesError::TooManyChanges
                    | PushChangesError::FidlConversion(_)),
                ) => {
                    log_warn!(
                        "IpTables: failed to call \
                        fuchsia.net.filter.NamespaceController/PushChanges: {e}"
                    );
                    return error!(ECOMM);
                }
                Err(e @ PushChangesError::ErrorOnChange(_)) => {
                    log_warn!(
                        "IpTables: fuchsia.net.filter.NamespaceController/PushChanges \
                        returned error: {e}"
                    );
                    return error!(EINVAL);
                }
            }
        }

        match controller.commit_idempotent(zx::MonotonicInstant::INFINITE) {
            Ok(()) => Ok(()),
            Err(e @ (CommitError::CallMethod(_) | CommitError::FidlConversion(_))) => {
                log_warn!(
                    "IpTables: failed to call \
                    fuchsia.net.filter.NamespaceController/Commit: {e}"
                );
                error!(ECOMM)
            }
            Err(
                e @ (CommitError::RuleWithInvalidMatcher(_)
                | CommitError::RuleWithInvalidAction(_)
                | CommitError::TransparentProxyWithInvalidMatcher(_)
                | CommitError::CyclicalRoutineGraph(_)
                | CommitError::RedirectWithInvalidMatcher(_)
                | CommitError::MasqueradeWithInvalidMatcher(_)
                | CommitError::RejectWithInvalidMatcher(_)
                | CommitError::ErrorOnChange(_)),
            ) => {
                log_warn!(
                    "IpTables: fuchsia.net.filter.NamespaceController/Commit \
                    returned error: {e}"
                );
                error!(EINVAL)
            }
        }
    }

    /// Registers eBPF programs with the controller and updates the state for the specified table.
    fn register_ebpf_programs(
        &mut self,
        ip: Ip,
        table_id: TableId,
        ebpf_programs: HashMap<febpf::ProgramId, ProgramHandle>,
    ) -> Result<(), Errno> {
        // Register new programs with the controller.
        let controller = self.controller.get()?;
        for (id, program_handle) in &ebpf_programs {
            if !self.ebpf_programs.contains_key(id) {
                let program: &Program = program_handle.deref();
                match controller.register_ebpf_program(
                    program.fidl_handle(),
                    program.try_into()?,
                    zx::MonotonicInstant::INFINITE,
                ) {
                    Ok(_) => {}
                    Err(RegisterEbpfProgramError::AlreadyRegistered) => {}
                    Err(e) => {
                        log_warn!("IpTables: failed to register eBPF program: {e}");
                        return error!(EINVAL);
                    }
                }
            }
        }

        // Cleanup programs used in the previous version of the table.
        self.ebpf_programs.retain(|_id, program_state| {
            program_state.tables.remove(ip, table_id);

            !program_state.tables.is_empty()
        });

        // Update the state for the new programs.
        for (id, program_handle) in &ebpf_programs {
            let entry = self.ebpf_programs.entry(*id).or_insert_with(|| EbpfProgramState {
                program: program_handle.clone(),
                tables: IpTableSet::default(),
            });
            entry.tables.add(ip, table_id);
        }

        Ok(())
    }

    fn replace_table(
        &mut self,
        ip: Ip,
        ebpf_programs: HashMap<febpf::ProgramId, ProgramHandle>,
        table: iptables_utils::IpTable,
    ) -> Result<(), Errno> {
        let entries = table.parser.entries_bytes().to_vec();
        let replace_info = table.parser.replace_info.clone();
        let iptable_entry = IpTable {
            num_entries: replace_info.num_entries as u32,
            size: replace_info.size as u32,
            entries,
            num_counters: replace_info.num_counters,
            valid_hooks: replace_info.valid_hooks.bits(),
            hook_entry: replace_info.hook_entry,
            underflow: replace_info.underflow,
            counters: vec![],
        };

        let table_id = replace_info.table_id;
        self.register_ebpf_programs(ip, table_id, ebpf_programs)?;

        self.send_changes_to_net_filter(table.into_changes())?;

        match ip {
            Ip::V4 => self.ipv4[table_id] = iptable_entry,
            Ip::V6 => self.ipv6[table_id] = iptable_entry,
        }

        Ok(())
    }
}

/// Stores [`IpTable`]s associated with each protocol.
pub struct IpTables {
    state: LockDepRwLock<IpTablesState, KernelIpTables>,
}

impl IpTables {
    pub fn new() -> Self {
        // Install default chains and policies on supported tables. These chains are expected to be
        // present on the system before `iptables` client is ran.
        // TODO(https://fxbug.dev/354766238): Propagated default rules to fuchsia.net.filter.
        Self {
            state: LockDepRwLock::new(IpTablesState {
                ipv4: default_ipv4_tables(),
                ipv6: default_ipv6_tables(),
                controller: LazyController::default(),
                ebpf_programs: HashMap::new(),
            }),
        }
    }

    /// Returns `true` if the sockopt can be handled by [`IpTables`].
    pub fn can_handle_getsockopt(level: u32, optname: u32) -> bool {
        matches!(
            (level, optname),
            (
                SOL_IP,
                IPT_SO_GET_INFO
                    | IPT_SO_GET_ENTRIES
                    | IPT_SO_GET_REVISION_MATCH
                    | IPT_SO_GET_REVISION_TARGET,
            ) | (
                SOL_IPV6,
                IP6T_SO_GET_INFO
                    | IP6T_SO_GET_ENTRIES
                    | IP6T_SO_GET_REVISION_MATCH
                    | IP6T_SO_GET_REVISION_TARGET,
            )
        )
    }

    /// Returns `true` if the sockopt can be handled by [`IpTables`].
    pub fn can_handle_setsockopt(level: u32, optname: u32) -> bool {
        matches!(
            (level, optname),
            (SOL_IP | SOL_IPV6, IPT_SO_SET_REPLACE | IPT_SO_SET_ADD_COUNTERS)
        )
    }

    pub fn getsockopt(
        &self,
        current_task: &CurrentTask,
        socket: &SocketHandle,
        optname: u32,
        mut optval: Vec<u8>,
    ) -> Result<Vec<u8>, Errno> {
        security::check_task_capable(current_task, CAP_NET_ADMIN)?;

        if optval.is_empty() {
            return error!(EINVAL);
        }
        if socket.socket_type != SocketType::Raw {
            return error!(ENOPROTOOPT);
        }

        match optname {
            // Returns information about the table specified by `optval`.
            IPT_SO_GET_INFO => {
                if socket.domain == SocketDomain::Inet {
                    let (mut info, _) =
                        ipt_getinfo::read_from_prefix(&*optval).map_err(|_| errno!(EINVAL))?;
                    let Ok(table_id) = TableId::try_from(&info.name) else {
                        return error!(EINVAL);
                    };
                    let state = self.state.read();
                    let table = &state.ipv4[table_id];
                    info.valid_hooks = table.valid_hooks;
                    info.hook_entry = table.hook_entry;
                    info.underflow = table.underflow;
                    info.num_entries = table.num_entries;
                    info.size = table.size;
                    Ok(info.as_bytes().to_vec())
                } else {
                    let (mut info, _) =
                        ip6t_getinfo::read_from_prefix(&*optval).map_err(|_| errno!(EINVAL))?;
                    let Ok(table_id) = TableId::try_from(&info.name) else {
                        return error!(EINVAL);
                    };
                    let state = self.state.read();
                    let table = &state.ipv6[table_id];
                    info.valid_hooks = table.valid_hooks;
                    info.hook_entry = table.hook_entry;
                    info.underflow = table.underflow;
                    info.num_entries = table.num_entries;
                    info.size = table.size;
                    Ok(info.as_bytes().to_vec())
                }
            }

            // Returns the entries of the table specified by `optval`.
            IPT_SO_GET_ENTRIES => {
                if socket.domain == SocketDomain::Inet {
                    let (get_entries, _) =
                        ipt_get_entries::read_from_prefix(&*optval).map_err(|_| errno!(EINVAL))?;
                    let Ok(table_id) = TableId::try_from(&get_entries.name) else {
                        return error!(EINVAL);
                    };
                    let mut entry_bytes = self.state.read().ipv4[table_id].entries.clone();

                    if entry_bytes.len() > get_entries.size as usize {
                        log_warn!("Entries are longer than expected so truncating.");
                        entry_bytes.truncate(get_entries.size as usize);
                    }

                    optval.truncate(std::mem::size_of::<ipt_get_entries>());
                    optval.append(&mut entry_bytes);
                } else {
                    let (get_entries, _) =
                        ip6t_get_entries::read_from_prefix(&*optval).map_err(|_| errno!(EINVAL))?;
                    let Ok(table_id) = TableId::try_from(&get_entries.name) else {
                        return error!(EINVAL);
                    };
                    let mut entry_bytes = self.state.read().ipv6[table_id].entries.clone();

                    if entry_bytes.len() > get_entries.size as usize {
                        log_warn!("Entries are longer than expected so truncating.");
                        entry_bytes.truncate(get_entries.size as usize);
                    }

                    optval.truncate(std::mem::size_of::<ip6t_get_entries>());
                    optval.append(&mut entry_bytes);
                }
                Ok(optval)
            }

            // Returns the revision match. Currently stubbed to return a max version number.
            IPT_SO_GET_REVISION_MATCH | IP6T_SO_GET_REVISION_MATCH => {
                let (mut revision, _) =
                    xt_get_revision::read_from_prefix(&*optval).map_err(|_| errno!(EINVAL))?;
                revision.revision = u8::MAX;
                Ok(revision.as_bytes().to_vec())
            }

            // Returns the revision target. Currently stubbed to return a max version number.
            IPT_SO_GET_REVISION_TARGET | IP6T_SO_GET_REVISION_TARGET => {
                let (mut revision, _) =
                    xt_get_revision::read_from_prefix(&*optval).map_err(|_| errno!(EINVAL))?;
                revision.revision = u8::MAX;
                Ok(revision.as_bytes().to_vec())
            }

            _ => {
                track_stub!(TODO("https://fxbug.dev/322875228"), "optname for network sockets");
                Ok(vec![])
            }
        }
    }

    pub fn setsockopt(
        &self,
        current_task: &CurrentTask,
        socket: &SocketHandle,
        optname: u32,
        optval: SockOptValue,
    ) -> Result<(), Errno> {
        security::check_task_capable(current_task, CAP_NET_ADMIN)?;

        let mut bytes = optval.to_vec(current_task)?;
        match optname {
            // Replaces the [`IpTable`] specified by `user_opt`.
            IPT_SO_SET_REPLACE => {
                // TODO(https://fxbug.dev/407842082): The following logic needs to be fixed.
                if socket.domain == SocketDomain::Inet {
                    self.replace_ipv4_table(current_task, bytes)
                } else {
                    self.replace_ipv6_table(current_task, bytes)
                }
            }

            // Sets the counters of the [`IpTable`] specified by `user_opt`.
            IPT_SO_SET_ADD_COUNTERS => {
                let (counters_info, _) =
                    xt_counters_info::read_from_prefix(&*bytes).map_err(|_| errno!(EINVAL))?;

                let Ok(table_id) = TableId::try_from(&counters_info.name) else {
                    return error!(EINVAL);
                };

                let mut state = self.state.write();
                let entry: &mut IpTable = match socket.domain {
                    SocketDomain::Inet => &mut state.ipv4[table_id],
                    _ => &mut state.ipv6[table_id],
                };

                entry.num_counters = counters_info.num_counters;
                let mut counters = vec![];
                bytes = bytes.split_off(std::mem::size_of::<xt_counters_info>());
                for chunk in bytes.chunks(std::mem::size_of::<xt_counters>()) {
                    counters
                        .push(xt_counters::read_from_prefix(chunk).map_err(|_| errno!(EINVAL))?.0);
                }
                entry.counters = counters;
                Ok(())
            }

            _ => Ok(()),
        }
    }

    fn replace_ipv4_table(&self, current_task: &CurrentTask, bytes: Vec<u8>) -> Result<(), Errno> {
        let mut ebpf_state = IpTablesEbpfState::new(current_task);
        let table =
            iptables_utils::IpTable::from_ipt_replace(&mut ebpf_state, bytes).map_err(|e| {
                log_warn!("Iptables: encountered error while parsing rules: {e}");
                errno!(EINVAL)
            })?;
        let ebpf_programs = ebpf_state.take_programs();
        self.state.write().replace_table(Ip::V4, ebpf_programs, table)?;

        Ok(())
    }

    fn replace_ipv6_table(&self, current_task: &CurrentTask, bytes: Vec<u8>) -> Result<(), Errno> {
        let mut ebpf_state = IpTablesEbpfState::new(current_task);
        let table =
            iptables_utils::IpTable::from_ip6t_replace(&mut ebpf_state, bytes).map_err(|e| {
                log_warn!("Iptables: encountered error while parsing rules: {e}");
                errno!(EINVAL)
            })?;
        let ebpf_programs = ebpf_state.take_programs();
        self.state.write().replace_table(Ip::V6, ebpf_programs, table)?;

        Ok(())
    }
}

struct IpTablesEbpfState<'a> {
    current_task: &'a CurrentTask,
    ebpf_programs: HashMap<febpf::ProgramId, ProgramHandle>,
}

impl<'a> IpTablesEbpfState<'a> {
    fn new(current_task: &'a CurrentTask) -> Self {
        Self { current_task, ebpf_programs: HashMap::default() }
    }

    fn take_programs(self) -> HashMap<febpf::ProgramId, ProgramHandle> {
        self.ebpf_programs
    }
}

impl<'a> IptReplaceContext for IpTablesEbpfState<'a> {
    fn resolve_ebpf_socket_filter(
        &mut self,
        path: &BString,
    ) -> Result<febpf::ProgramId, IpTableParseError> {
        let program =
            resolve_pinned_bpf_object(self.current_task, path.as_ref(), OpenFlags::RDONLY)
                .and_then(|handle| handle.into_program())
                .map_err(|e| {
                    log_warn!(
                        "Failed to resolve eBPF program path {} for iptable matcher: {:?}",
                        path,
                        e
                    );
                    IpTableParseError::InvalidEbpfProgramPath { path: path.clone() }
                })?;

        let id = program.fidl_id();
        self.ebpf_programs.insert(id, program);
        Ok(id)
    }
}
