// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file contains translation between fuchsia.net.filter data structures and Linux
// iptables structures.

use bstr::BString;
use itertools::Itertools;
use starnix_logging::track_stub;
use starnix_uapi::iptables_flags::{
    IptIpFlags, IptIpFlagsV4, IptIpFlagsV6, IptIpInverseFlags, NfIpHooks, NfNatRangeFlags,
    XtTcpInverseFlags, XtUdpInverseFlags,
};
use starnix_uapi::{
    IPPROTO_ICMP, IPPROTO_ICMPV6, IPPROTO_IP, IPPROTO_TCP, IPPROTO_UDP, IPT_RETURN, NF_ACCEPT,
    NF_DROP, NF_IP_FORWARD, NF_IP_LOCAL_IN, NF_IP_LOCAL_OUT, NF_IP_NUMHOOKS, NF_IP_POST_ROUTING,
    NF_IP_PRE_ROUTING, NF_QUEUE, c_char, c_int, c_uchar, c_uint, in_addr, in6_addr, ip6t_entry,
    ip6t_ip6, ip6t_reject_info, ip6t_replace, ipt_entry, ipt_ip, ipt_reject_info, ipt_replace,
    nf_ip_hook_priorities_NF_IP_PRI_FILTER, nf_ip_hook_priorities_NF_IP_PRI_MANGLE,
    nf_ip_hook_priorities_NF_IP_PRI_NAT_DST, nf_ip_hook_priorities_NF_IP_PRI_NAT_SRC,
    nf_ip_hook_priorities_NF_IP_PRI_RAW, nf_nat_ipv4_multi_range_compat, nf_nat_range,
    xt_bpf_info_v1, xt_entry_match__bindgen_ty_1__bindgen_ty_1 as xt_entry_match,
    xt_entry_target__bindgen_ty_1__bindgen_ty_1 as xt_entry_target, xt_mark_tginfo2, xt_tcp,
    xt_tproxy_target_info_v1, xt_udp,
};
use std::any::type_name;
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::mem::size_of;
use std::num::NonZeroU16;
use std::ops::RangeInclusive;
use thiserror::Error;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use {
    fidl_fuchsia_ebpf as febpf, fidl_fuchsia_net as fnet,
    fidl_fuchsia_net_filter_ext as fnet_filter_ext,
    fidl_fuchsia_net_matchers_ext as fnet_matchers_ext,
};

const TABLE_FILTER: &str = "filter";
const TABLE_MANGLE: &str = "mangle";
const TABLE_NAT: &str = "nat";
const TABLE_RAW: &str = "raw";

const CHAIN_PREROUTING: &str = "PREROUTING";
const CHAIN_INPUT: &str = "INPUT";
const CHAIN_FORWARD: &str = "FORWARD";
const CHAIN_OUTPUT: &str = "OUTPUT";
const CHAIN_POSTROUTING: &str = "POSTROUTING";

const IPT_REPLACE_SIZE: usize = size_of::<ipt_replace>();
const IP6T_REPLACE_SIZE: usize = size_of::<ip6t_replace>();

// Verdict codes for Standard targets, calculated the same way as Linux.
pub const VERDICT_DROP: i32 = -(NF_DROP as i32) - 1;
pub const VERDICT_ACCEPT: i32 = -(NF_ACCEPT as i32) - 1;
pub const VERDICT_QUEUE: i32 = -(NF_QUEUE as i32) - 1;
pub const VERDICT_RETURN: i32 = IPT_RETURN;

const TARGET_STANDARD: &str = "";
const TARGET_ERROR: &str = "ERROR";
const TARGET_REDIRECT: &str = "REDIRECT";
const TARGET_TPROXY: &str = "TPROXY";
const TARGET_MARK: &str = "MARK";
const TARGET_REJECT: &str = "REJECT";

#[derive(Debug, Error, PartialEq)]
pub enum IpTableParseError {
    #[error("error during ascii conversion: {0}")]
    AsciiConversion(#[from] AsciiConversionError),
    #[error("error during address conversion: {0}")]
    IpAddressConversion(#[from] IpAddressConversionError),
    #[error("FIDL conversion error: {0}")]
    FidlConversion(#[from] fnet_filter_ext::FidlConversionError),
    #[error("Port matcher error: {0}")]
    PortMatcher(#[from] fnet_matchers_ext::PortError),
    #[error("buffer of size {size} is too small to read ipt_replace or ip6t_replace")]
    BufferTooSmallForMetadata { size: usize },
    #[error("specified size {specified_size} does not match size of entries {entries_size}")]
    SizeMismatch { specified_size: usize, entries_size: usize },
    #[error("reached end of buffer while trying to parse {type_name} at position {position}")]
    ParseEndOfBuffer { type_name: &'static str, position: usize },
    #[error("reached end of buffer while advancing by {offset} at position {position}")]
    AdvanceEndOfBuffer { offset: usize, position: usize },
    #[error("target offset {offset} is too small to fit ipt_entry")]
    TargetOffsetTooSmall { offset: usize },
    #[error("matchers extend beyond target offset {offset}")]
    InvalidTargetOffset { offset: usize },
    #[error("next offset {offset} is too small to fit ipt_entry")]
    NextOffsetTooSmall { offset: usize },
    #[error("target extends beyond next offset {offset}")]
    InvalidNextOffset { offset: usize },
    #[error("match size {size} is too small to fit xt_entry_match")]
    MatchSizeTooSmall { size: usize },
    #[error("target size {size} does not match specified {match_name} matcher")]
    MatchSizeMismatch { size: usize, match_name: &'static str },
    #[error("target size {size} is too small to fit xt_entry_target")]
    TargetSizeTooSmall { size: usize },
    #[error("target size {size} does not match specified {target_name} target")]
    TargetSizeMismatch { size: usize, target_name: &'static str },
    #[error("specified {specified} entries but found {found} entries")]
    NumEntriesMismatch { specified: usize, found: usize },
    #[error("error entry has unexpected matchers")]
    ErrorEntryHasMatchers,
    #[error("table definition does not have trailing error target")]
    NoTrailingErrorTarget,
    #[error("found chain {chain_name} with no policy entry")]
    ChainHasNoPolicy { chain_name: String },
    #[error("found rule specification before first chain definition")]
    RuleBeforeFirstChain,
    #[error("invalid IP flags {flags:#x} found in rule specification")]
    InvalidIpFlags { flags: u8 },
    #[error("invalid IP inverse flags {flags:#x} found in rule specification")]
    InvalidIpInverseFlags { flags: u8 },
    #[error("invalid TCP matcher inverse flags {flags:#x}")]
    InvalidXtTcpInverseFlags { flags: u8 },
    #[error("invalid UDP matcher inverse flags {flags:#x}")]
    InvalidXtUdpInverseFlags { flags: u8 },
    #[error("invalid standard target verdict {verdict}")]
    InvalidVerdict { verdict: i32 },
    #[error("invalid jump target {jump_target}")]
    InvalidJumpTarget { jump_target: usize },
    #[error("invalid valid_hooks field {hooks:#x}")]
    InvalidHookBits { hooks: u32 },
    #[error("invalid redirect target range size {range_size}")]
    InvalidRedirectTargetRangeSize { range_size: u32 },
    #[error("invalid redirect target range [{start}, {end}]")]
    InvalidRedirectTargetRange { start: u16, end: u16 },
    #[error("invalid redirect target flags {flags}")]
    InvalidRedirectTargetFlags { flags: u32 },
    #[error("invalid tproxy target: one of address and port must be specified")]
    InvalidTproxyZeroAddressAndPort,
    #[error("invalid hooks {hooks:#x} found for table {table_name}")]
    InvalidHooksForTable { hooks: u32, table_name: &'static str },
    #[error("hook ranges overlap at offset {offset}")]
    HookRangesOverlap { offset: usize },
    #[error("unrecognized table name {table_name}")]
    UnrecognizedTableName { table_name: String },
    #[error("invalid hook entry or underflow values for index {index} [{start}, {end}]")]
    InvalidHookEntryOrUnderflow { index: u32, start: usize, end: usize },
    #[error("unexpected error target {error_name} found in installed routine")]
    UnexpectedErrorTarget { error_name: String },
    #[error("too many rules")]
    TooManyRules,
    #[error("match extension does not match protocol")]
    MatchExtensionDoesNotMatchProtocol,
    #[error("match extension would overwrite another matcher")]
    MatchExtensionOverwrite,
    #[error("unsupported BPF mode {mode}")]
    UnsupportedBpfMatcherMode { mode: u16 },
    #[error("eBPF program path is not null terminated")]
    NoNulInEbpfProgramPath,
    #[error("Invalid eBPF program path: {path}")]
    InvalidEbpfProgramPath { path: BString },
    #[error("Invalid reject type {raw}")]
    InvalidRejectType { raw: u32 },
    #[error("REJECT rule outside of a filter table")]
    RejectRuleOutsideFilterTable,
}

#[derive(Debug, Error, PartialEq)]
pub enum AsciiConversionError {
    #[error("nul byte not found in ASCII string {chars:?}")]
    NulByteNotFound { chars: Vec<c_char> },
    #[error("unexpected nul byte found in UTF-8 String at position {pos}")]
    NulByteInString { pos: usize },
    #[error("char is out of range for ASCII (0 to 127)")]
    NonAsciiChar,
    #[error("buffer of size {buffer_size} too small to fit data of size {data_size}")]
    BufferTooSmall { buffer_size: usize, data_size: usize },
}

#[derive(Debug, Error, PartialEq)]
pub enum IpAddressConversionError {
    #[error("IPv4 address subnet mask {mask:#b} has non-prefix bits")]
    IpV4SubnetMaskHasNonPrefixBits { mask: u32 },
    #[error("IPv6 address subnet mask {mask:#b} has non-prefix bits")]
    IpV6SubnetMaskHasNonPrefixBits { mask: u128 },
}

/// Metadata passed by `iptables` when updating a table.
///
/// Describes the subsequent buffer of `size` bytes, containing `num_entries` Entries that describe
/// chain definitions, rule specifications, and end of input.
/// IPv4 and IPv6 tables have the same metadata but uses different Linux structs: `ipt_replace` for
/// IPv4, and `ip6t_replace` for IPv6.
#[derive(Clone, Debug)]
pub struct ReplaceInfo {
    /// The table to be replaced.
    pub table_id: TableId,

    /// Number of entries defined on the table.
    pub num_entries: usize,

    /// Size of entries in bytes.
    pub size: usize,

    /// Bitmap of which installed chains are on the table.
    pub valid_hooks: NfIpHooks,

    /// Byte offsets of the first entry of each installed chain.
    pub hook_entry: [c_uint; 5usize],

    /// Byte offsets of the policy of each installed chain.
    pub underflow: [c_uint; 5usize],

    /// Unused field. Number of counters.
    pub num_counters: c_uint,
}

impl TryFrom<ipt_replace> for ReplaceInfo {
    type Error = IpTableParseError;

    fn try_from(replace: ipt_replace) -> Result<Self, Self::Error> {
        let valid_hooks = NfIpHooks::from_bits(replace.valid_hooks)
            .ok_or(IpTableParseError::InvalidHookBits { hooks: replace.valid_hooks })?;
        Ok(Self {
            table_id: TableId::try_from(&replace.name)?,
            num_entries: usize::try_from(replace.num_entries).expect("u32 fits in usize"),
            size: usize::try_from(replace.size).expect("u32 fits in usize"),
            valid_hooks,
            hook_entry: replace.hook_entry,
            underflow: replace.underflow,
            num_counters: replace.num_counters,
        })
    }
}

impl TryFrom<ip6t_replace> for ReplaceInfo {
    type Error = IpTableParseError;

    fn try_from(replace: ip6t_replace) -> Result<Self, Self::Error> {
        let valid_hooks = NfIpHooks::from_bits(replace.valid_hooks)
            .ok_or(IpTableParseError::InvalidHookBits { hooks: replace.valid_hooks })?;
        Ok(Self {
            table_id: TableId::try_from(&replace.name)?,
            num_entries: usize::try_from(replace.num_entries).expect("u32 fits in usize"),
            size: usize::try_from(replace.size).expect("u32 fits in usize"),
            valid_hooks,
            hook_entry: replace.hook_entry,
            underflow: replace.underflow,
            num_counters: replace.num_counters,
        })
    }
}

// Metadata of each Entry.
#[derive(Clone, Debug)]
pub struct EntryInfo {
    pub ip_info: IpInfo,
    pub target_offset: usize,
    pub next_offset: usize,
}

impl TryFrom<ipt_entry> for EntryInfo {
    type Error = IpTableParseError;

    fn try_from(entry: ipt_entry) -> Result<Self, Self::Error> {
        let ip_info = IpInfo::try_from(entry.ip)?;

        let target_offset = usize::from(entry.target_offset);
        if target_offset < size_of::<ipt_entry>() {
            return Err(IpTableParseError::TargetOffsetTooSmall { offset: target_offset });
        }

        let next_offset = usize::from(entry.next_offset);
        if next_offset < size_of::<ipt_entry>() {
            return Err(IpTableParseError::NextOffsetTooSmall { offset: next_offset });
        }

        Ok(Self { ip_info, target_offset, next_offset })
    }
}

impl TryFrom<ip6t_entry> for EntryInfo {
    type Error = IpTableParseError;

    fn try_from(entry: ip6t_entry) -> Result<Self, Self::Error> {
        let ip_info = IpInfo::try_from(entry.ipv6)?;

        let target_offset = usize::from(entry.target_offset);
        if target_offset < size_of::<ip6t_entry>() {
            return Err(IpTableParseError::TargetOffsetTooSmall { offset: target_offset });
        }

        let next_offset = usize::from(entry.next_offset);
        if next_offset < size_of::<ip6t_entry>() {
            return Err(IpTableParseError::NextOffsetTooSmall { offset: next_offset });
        }

        Ok(Self { ip_info, target_offset, next_offset })
    }
}

#[derive(Clone, Debug)]
pub struct IpInfo {
    pub src_subnet: Option<fnet::Subnet>,
    pub dst_subnet: Option<fnet::Subnet>,
    pub in_interface: Option<String>,
    pub out_interface: Option<String>,
    pub inverse_flags: IptIpInverseFlags,
    pub protocol: c_uint,
    pub flags: IptIpFlags,

    // Only for IPv6
    pub tos: c_uchar,
}

impl TryFrom<ipt_ip> for IpInfo {
    type Error = IpTableParseError;

    fn try_from(ip: ipt_ip) -> Result<Self, Self::Error> {
        let ipt_ip {
            src,
            dst,
            smsk,
            dmsk,
            iniface,
            outiface,
            iniface_mask,
            outiface_mask,
            proto,
            flags,
            invflags,
        } = ip;
        let src_subnet =
            ipv4_to_subnet(src, smsk).map_err(IpTableParseError::IpAddressConversion)?;
        let dst_subnet =
            ipv4_to_subnet(dst, dmsk).map_err(IpTableParseError::IpAddressConversion)?;

        let in_interface = if iniface_mask == [0u8; 16] {
            None
        } else {
            Some(ascii_to_string(&iniface).map_err(IpTableParseError::AsciiConversion)?)
        };
        let out_interface = if outiface_mask == [0u8; 16] {
            None
        } else {
            Some(ascii_to_string(&outiface).map_err(IpTableParseError::AsciiConversion)?)
        };

        let flags_v4 = IptIpFlagsV4::from_bits(flags.into())
            .ok_or(IpTableParseError::InvalidIpFlags { flags })?;
        let inverse_flags = IptIpInverseFlags::from_bits(invflags.into())
            .ok_or(IpTableParseError::InvalidIpInverseFlags { flags: invflags })?;

        Ok(Self {
            src_subnet,
            dst_subnet,
            in_interface,
            out_interface,
            inverse_flags,
            protocol: proto.into(),
            flags: IptIpFlags::V4(flags_v4),
            // Unused in IPv4
            tos: 0,
        })
    }
}

impl TryFrom<ip6t_ip6> for IpInfo {
    type Error = IpTableParseError;

    fn try_from(ip: ip6t_ip6) -> Result<Self, Self::Error> {
        let ip6t_ip6 {
            src,
            dst,
            smsk,
            dmsk,
            iniface,
            outiface,
            iniface_mask,
            outiface_mask,
            proto,
            tos,
            flags,
            invflags,
            __bindgen_padding_0,
        } = ip;
        let src_subnet =
            ipv6_to_subnet(src, smsk).map_err(IpTableParseError::IpAddressConversion)?;
        let dst_subnet =
            ipv6_to_subnet(dst, dmsk).map_err(IpTableParseError::IpAddressConversion)?;

        let in_interface = if iniface_mask == [0u8; 16] {
            None
        } else {
            Some(ascii_to_string(&iniface).map_err(IpTableParseError::AsciiConversion)?)
        };
        let out_interface = if outiface_mask == [0u8; 16] {
            None
        } else {
            Some(ascii_to_string(&outiface).map_err(IpTableParseError::AsciiConversion)?)
        };

        let flags_v6 = IptIpFlagsV6::from_bits(flags.into())
            .ok_or(IpTableParseError::InvalidIpFlags { flags })?;
        let inverse_flags = IptIpInverseFlags::from_bits(invflags.into())
            .ok_or(IpTableParseError::InvalidIpInverseFlags { flags: invflags })?;

        Ok(Self {
            src_subnet,
            dst_subnet,
            in_interface,
            out_interface,
            inverse_flags,
            protocol: proto.into(),
            flags: IptIpFlags::V6(flags_v6),
            tos,
        })
    }
}

impl IpInfo {
    // For IPv6, whether to match protocol is configured via a flag.
    fn should_match_protocol(&self) -> bool {
        match self.flags {
            IptIpFlags::V4(_) => true,
            IptIpFlags::V6(flags) => flags.contains(IptIpFlagsV6::PROTOCOL),
        }
    }
}

/// An "Entry" is either:
///
/// 1. Start of a new iptables chain
/// 2. A rule on the chain
/// 3. The policy of the chain
/// 4. End of input
#[derive(Debug)]
pub struct Entry {
    /// bytes since the first entry, referred to by JUMP targets.
    pub byte_pos: usize,

    pub entry_info: EntryInfo,
    pub matchers: Vec<Matcher>,
    pub target: Target,
}

#[derive(Debug)]
pub enum Matcher {
    Unknown,
    Tcp(xt_tcp),
    Udp(xt_udp),
    Bpf { path: BString },
}

#[derive(Debug)]
pub enum Target {
    Unknown { name: String, bytes: Vec<u8> },

    // Parsed from `xt_standard_target`, which contains a numerical verdict.
    //
    // A 0 or positive verdict is a JUMP to another chain or rule, and a negative verdict
    // is one of the builtin targets like ACCEPT, DROP or RETURN.
    Standard(c_int),

    // Parsed from `xt_error_target`, which contains a string.
    //
    // This misleading variant name does not indicate an error in parsing/translation, but rather
    // the start of a chain or the end of input. The inner string is either the name of a chain
    // that the following rule-specifications belong to, or "ERROR" if it is the last entry in the
    // list of entries. Note that "ERROR" does not necessarily indicate the last entry, as a chain
    // can be named "ERROR".
    Error(String),

    // Parsed from `nf_nat_ipv4_multi_range_compat` for IPv4, and `nf_nat_range` for IPv6.
    //
    // The original Linux structs are also used for the DNAT target, and contains information about
    // IP addresses which is ignored for REDIRECT.
    Redirect(NfNatRange),

    // Parsed from `xt_tproxy_target_info` for IPv4, and `xt_tproxy_target_info_v1` for IPv6.
    Tproxy(TproxyInfo),

    // Parsed from `xt_mark_tginfo2`.
    Mark { mask: u32, mark: u32 },

    // Proceed to the next rule in the chain.
    Next,

    // Parsed from `ipt_reject_info` and `ip6t_reject_info`.
    Reject(fnet_filter_ext::RejectType),
}

#[derive(Debug)]
pub struct NfNatRange {
    flags: NfNatRangeFlags,
    start: u16,
    end: u16,
}

#[derive(Debug)]
pub struct TproxyInfo {
    address: Option<fnet::IpAddress>,
    port: Option<NonZeroU16>,
}

fn reject_type_from_raw(
    ip: Ip,
    raw: u32,
) -> Result<fnet_filter_ext::RejectType, IpTableParseError> {
    use fidl_fuchsia_net_filter_ext::RejectType;
    use starnix_uapi::{
        ip6t_reject_with_IP6T_ICMP6_ADDR_UNREACH as IP6T_ICMP6_ADDR_UNREACH,
        ip6t_reject_with_IP6T_ICMP6_ADM_PROHIBITED as IP6T_ICMP6_ADM_PROHIBITED,
        ip6t_reject_with_IP6T_ICMP6_NO_ROUTE as IP6T_ICMP6_NO_ROUTE,
        ip6t_reject_with_IP6T_ICMP6_POLICY_FAIL as IP6T_ICMP6_POLICY_FAIL,
        ip6t_reject_with_IP6T_ICMP6_PORT_UNREACH as IP6T_ICMP6_PORT_UNREACH,
        ip6t_reject_with_IP6T_ICMP6_REJECT_ROUTE as IP6T_ICMP6_REJECT_ROUTE,
        ip6t_reject_with_IP6T_TCP_RESET as IP6T_TCP_RESET,
        ipt_reject_with_IPT_ICMP_ADMIN_PROHIBITED as IPT_ICMP_ADMIN_PROHIBITED,
        ipt_reject_with_IPT_ICMP_HOST_PROHIBITED as IPT_ICMP_HOST_PROHIBITED,
        ipt_reject_with_IPT_ICMP_HOST_UNREACHABLE as IPT_ICMP_HOST_UNREACHABLE,
        ipt_reject_with_IPT_ICMP_NET_PROHIBITED as IPT_ICMP_NET_PROHIBITED,
        ipt_reject_with_IPT_ICMP_NET_UNREACHABLE as IPT_ICMP_NET_UNREACHABLE,
        ipt_reject_with_IPT_ICMP_PORT_UNREACHABLE as IPT_ICMP_PORT_UNREACHABLE,
        ipt_reject_with_IPT_ICMP_PROT_UNREACHABLE as IPT_ICMP_PROT_UNREACHABLE,
        ipt_reject_with_IPT_TCP_RESET as IPT_TCP_RESET,
    };

    match ip {
        Ip::V4 => match raw {
            IPT_TCP_RESET => Ok(RejectType::TcpReset),
            IPT_ICMP_NET_UNREACHABLE => Ok(RejectType::NetUnreachable),
            IPT_ICMP_HOST_UNREACHABLE => Ok(RejectType::HostUnreachable),
            IPT_ICMP_PROT_UNREACHABLE => Ok(RejectType::ProtoUnreachable),
            IPT_ICMP_PORT_UNREACHABLE => Ok(RejectType::PortUnreachable),
            IPT_ICMP_NET_PROHIBITED => Ok(RejectType::RoutePolicyFail),
            IPT_ICMP_HOST_PROHIBITED => Ok(RejectType::RejectRoute),
            IPT_ICMP_ADMIN_PROHIBITED => Ok(RejectType::AdminProhibited),
            _ => Err(IpTableParseError::InvalidRejectType { raw }),
        },
        Ip::V6 => match raw {
            IP6T_TCP_RESET => Ok(RejectType::TcpReset),
            IP6T_ICMP6_NO_ROUTE => Ok(RejectType::NetUnreachable),
            IP6T_ICMP6_ADM_PROHIBITED => Ok(RejectType::AdminProhibited),
            IP6T_ICMP6_ADDR_UNREACH => Ok(RejectType::HostUnreachable),
            IP6T_ICMP6_PORT_UNREACH => Ok(RejectType::PortUnreachable),
            IP6T_ICMP6_POLICY_FAIL => Ok(RejectType::RoutePolicyFail),
            IP6T_ICMP6_REJECT_ROUTE => Ok(RejectType::RejectRoute),
            _ => Err(IpTableParseError::InvalidRejectType { raw }),
        },
    }
}

// `xt_standard_target` without the `target` field.
//
// `target` of type `xt_entry_target` is parsed first to determine the target's variant.
#[repr(C)]
#[derive(IntoBytes, Debug, Default, KnownLayout, FromBytes, Immutable)]
pub struct VerdictWithPadding {
    pub verdict: c_int,
    pub _padding: [u8; 4usize],
}

// `xt_error_target` without the `target` field.
//
// `target` of type `xt_entry_target` is parsed first to determine the target's variant.
#[repr(C)]
#[derive(IntoBytes, Debug, Default, KnownLayout, FromBytes, Immutable)]
pub struct ErrorNameWithPadding {
    pub errorname: [c_char; 30usize],
    pub _padding: [u8; 2usize],
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Ip {
    V4,
    V6,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum TableId {
    Filter,
    Mangle,
    Nat,
    Raw,
}
pub const NUM_TABLES: usize = TableId::Raw as usize + 1;

impl TryFrom<&[c_char; starnix_uapi::XT_TABLE_MAXNAMELEN as usize]> for TableId {
    type Error = IpTableParseError;

    fn try_from(
        value: &[c_char; starnix_uapi::XT_TABLE_MAXNAMELEN as usize],
    ) -> Result<Self, Self::Error> {
        let name = ascii_to_string(value).map_err(IpTableParseError::AsciiConversion)?;
        if name == TABLE_FILTER {
            return Ok(TableId::Filter);
        } else if name == TABLE_MANGLE {
            return Ok(TableId::Mangle);
        } else if name == TABLE_NAT {
            return Ok(TableId::Nat);
        } else if name == TABLE_RAW {
            return Ok(TableId::Raw);
        }
        Err(IpTableParseError::UnrecognizedTableName { table_name: name })
    }
}

impl TableId {
    fn to_str(&self) -> &'static str {
        match self {
            TableId::Filter => TABLE_FILTER,
            TableId::Mangle => TABLE_MANGLE,
            TableId::Nat => TABLE_NAT,
            TableId::Raw => TABLE_RAW,
        }
    }
}

impl From<TableId> for usize {
    fn from(value: TableId) -> Self {
        value as usize
    }
}

pub trait IptReplaceContext {
    fn resolve_ebpf_socket_filter(
        &mut self,
        path: &BString,
    ) -> Result<febpf::ProgramId, IpTableParseError>;
}

/// A parser for both `ipt_replace` and `ip6t_replace`, and its subsequent entries.
#[derive(Debug)]
pub struct IptReplaceParser {
    /// Determines which Linux structures the parser expects.
    protocol: Ip,

    /// Table metadata passed through `ipt_replace` or `ip6t_replace`.
    pub replace_info: ReplaceInfo,

    // Linux bytes to parse.
    //
    // General layout is an `ipt_replace` followed by N "entries", where each "entry" is
    // an `ipt_entry` and a `xt_*_target` with 0 or more "matchers" in between.
    //
    // In this IPv4 example, each row after the first is an entry:
    //
    //        [ ipt_replace ]
    //   0:   [ ipt_entry ][ xt_error_target ]
    //   1:   [ ipt_entry ][ xt_entry_match ][ xt_tcp ] ... [ xt_standard_target ]
    //   2:   [ ipt_entry ][ xt_error_target ]
    //        ...
    //   N-1: [ ipt_entry ][ xt_error_target ]
    //
    // The main difference for IPv6 is that `ipt_entry` is replaced by `ip6t_entry`, which contains
    // 128-bit IP addresses.
    bytes: Vec<u8>,

    /// Current parse position.
    parse_pos: usize,

    /// Keeps track of byte offsets of entries parsed so far. Used to check for errors.
    entry_offsets: HashSet<usize>,
}

impl IptReplaceParser {
    /// Initialize a new parser and tries to parse an `ipt_replace` struct from the buffer.
    /// The rest of the buffer is left unparsed.
    fn new_ipv4(bytes: Vec<u8>) -> Result<Self, IpTableParseError> {
        let (ipt_replace, _) = ipt_replace::read_from_prefix(&bytes)
            .map_err(|_| IpTableParseError::BufferTooSmallForMetadata { size: bytes.len() })?;
        let replace_info = ReplaceInfo::try_from(ipt_replace)?;

        if replace_info.size != bytes.len() - IPT_REPLACE_SIZE {
            return Err(IpTableParseError::SizeMismatch {
                specified_size: replace_info.size,
                entries_size: bytes.len() - IPT_REPLACE_SIZE,
            });
        }

        Ok(Self {
            protocol: Ip::V4,
            replace_info,
            bytes,
            parse_pos: IPT_REPLACE_SIZE,
            entry_offsets: HashSet::new(),
        })
    }

    /// Initialize a new parser and tries to parse an `ip6t_replace` struct from the buffer.
    /// The rest of the buffer is left unparsed.
    fn new_ipv6(bytes: Vec<u8>) -> Result<Self, IpTableParseError> {
        let (ip6t_replace, _) = ip6t_replace::read_from_prefix(&bytes)
            .map_err(|_| IpTableParseError::BufferTooSmallForMetadata { size: bytes.len() })?;
        let replace_info = ReplaceInfo::try_from(ip6t_replace)?;

        if replace_info.size != bytes.len() - IP6T_REPLACE_SIZE {
            return Err(IpTableParseError::SizeMismatch {
                specified_size: replace_info.size,
                entries_size: bytes.len() - IP6T_REPLACE_SIZE,
            });
        }

        Ok(Self {
            protocol: Ip::V6,
            replace_info,
            bytes,
            parse_pos: IP6T_REPLACE_SIZE,
            entry_offsets: HashSet::new(),
        })
    }

    fn finished(&self) -> bool {
        self.parse_pos >= self.bytes.len()
    }

    pub fn entries_bytes(&self) -> &[u8] {
        match self.protocol {
            Ip::V4 => &self.bytes[IPT_REPLACE_SIZE..],
            Ip::V6 => &self.bytes[IP6T_REPLACE_SIZE..],
        }
    }

    fn get_domain(&self) -> fnet_filter_ext::Domain {
        match self.protocol {
            Ip::V4 => fnet_filter_ext::Domain::Ipv4,
            Ip::V6 => fnet_filter_ext::Domain::Ipv6,
        }
    }

    pub fn get_namespace_id(&self) -> fnet_filter_ext::NamespaceId {
        match self.protocol {
            Ip::V4 => fnet_filter_ext::NamespaceId(format!(
                "ipv4-{}",
                self.replace_info.table_id.to_str()
            )),
            Ip::V6 => fnet_filter_ext::NamespaceId(format!(
                "ipv6-{}",
                self.replace_info.table_id.to_str()
            )),
        }
    }

    pub fn table_id(&self) -> TableId {
        self.replace_info.table_id
    }

    pub fn get_namespace(&self) -> fnet_filter_ext::Namespace {
        fnet_filter_ext::Namespace { id: self.get_namespace_id(), domain: self.get_domain() }
    }

    fn get_custom_routine_type(&self) -> fnet_filter_ext::RoutineType {
        match self.table_id() {
            TableId::Nat => fnet_filter_ext::RoutineType::Nat(None),
            TableId::Filter | TableId::Mangle | TableId::Raw => {
                fnet_filter_ext::RoutineType::Ip(None)
            }
        }
    }

    /// Returns whether `offset` points to a valid entry. Parser can only know this after reading
    /// all entries in `bytes`. Panics if the parser has not finished.
    pub fn is_valid_offset(&self, offset: usize) -> bool {
        assert!(self.finished());
        self.entry_offsets.contains(&offset)
    }

    fn bytes_since_first_entry(&self) -> usize {
        match self.protocol {
            Ip::V4 => self
                .parse_pos
                .checked_sub(IPT_REPLACE_SIZE)
                .expect("parse_pos is always larger or equal to size of ipt_replace"),
            Ip::V6 => self
                .parse_pos
                .checked_sub(IP6T_REPLACE_SIZE)
                .expect("parse_pos is always larger or equal to size of ip6t_replace"),
        }
    }

    fn get_next_bytes(&self, offset: usize) -> Option<&[u8]> {
        let new_pos = self.parse_pos + offset;
        if new_pos > self.bytes.len() { None } else { Some(&self.bytes[self.parse_pos..new_pos]) }
    }

    // Parse `bytes` starting from `parse_pos` as type T, without advancing `parse_pos`.
    // Used in cases where part of a structure must be parsed first, before determining how to parse
    // the rest of the structure.

    fn view_next_bytes_as<T: FromBytes>(&self) -> Result<T, IpTableParseError> {
        let bytes = self.get_next_bytes(size_of::<T>()).ok_or_else(|| {
            IpTableParseError::ParseEndOfBuffer {
                type_name: type_name::<T>(),
                position: self.parse_pos,
            }
        })?;
        let obj = T::read_from_bytes(bytes).expect("read_from slice of exact size is successful");
        Ok(obj)
    }

    // Add `offset` to `parse_pos`. Should be used after `view_next_bytes_as`.
    fn advance_parse_pos(&mut self, offset: usize) -> Result<(), IpTableParseError> {
        if self.parse_pos + offset > self.bytes.len() {
            return Err(IpTableParseError::AdvanceEndOfBuffer { offset, position: self.parse_pos });
        }
        self.parse_pos += offset;
        Ok(())
    }

    // Parse `bytes` starting from `parse_pos` as type T, and advance `parse_pos`.
    fn parse_next_bytes_as<T: FromBytes>(&mut self) -> Result<T, IpTableParseError> {
        let obj = self.view_next_bytes_as::<T>()?;
        self.advance_parse_pos(size_of::<T>())?;
        Ok(obj)
    }

    /// Parse the next Entry.
    ///
    /// An Entry is an `ipt_entry` (or `ip6t_entry` for IPv6), followed by 0 or more Matchers, and
    /// finally a Target.
    /// This method must advance `parse_pos`, so callers can assume that repeatedly calling this
    /// method will eventually terminate (i.e. `finished()` returns true) if no error is returned.
    fn parse_entry(&mut self) -> Result<Entry, IpTableParseError> {
        let byte_pos = self.bytes_since_first_entry();

        let entry_info = match self.protocol {
            Ip::V4 => EntryInfo::try_from(self.parse_next_bytes_as::<ipt_entry>()?)?,
            Ip::V6 => EntryInfo::try_from(self.parse_next_bytes_as::<ip6t_entry>()?)?,
        };

        let target_pos = byte_pos + entry_info.target_offset;
        let next_pos = byte_pos + entry_info.next_offset;

        let mut matchers = Vec::new();

        // Each entry has 0 or more matchers.
        while self.bytes_since_first_entry() < target_pos {
            matchers.push(self.parse_matcher()?);
        }

        // Check if matchers extend beyond the `target_offset`.
        if self.bytes_since_first_entry() != target_pos {
            return Err(IpTableParseError::InvalidTargetOffset {
                offset: entry_info.target_offset,
            });
        }

        // Each entry has 1 target.
        let target = self.parse_target()?;

        // Check if matchers and target extend beyond the `next_offset`.
        if self.bytes_since_first_entry() != next_pos {
            return Err(IpTableParseError::InvalidNextOffset { offset: entry_info.next_offset });
        }

        assert!(self.bytes_since_first_entry() > byte_pos, "parse_pos must advance");

        assert!(self.entry_offsets.insert(byte_pos));
        Ok(Entry { byte_pos, entry_info, matchers, target })
    }

    /// Parses next bytes as a `xt_entry_match` struct and a specified matcher struct.
    fn parse_matcher(&mut self) -> Result<Matcher, IpTableParseError> {
        let match_info = self.parse_next_bytes_as::<xt_entry_match>()?;

        let match_size = usize::from(match_info.match_size);
        if match_size < size_of::<xt_entry_match>() {
            return Err(IpTableParseError::MatchSizeTooSmall { size: match_size });
        }
        let remaining_size = match_size - size_of::<xt_entry_match>();
        let name = ascii_to_string(&match_info.name).map_err(IpTableParseError::AsciiConversion)?;
        let revision = match_info.revision;

        let matcher = match (name.as_str(), revision) {
            ("tcp", 0) => {
                if remaining_size < size_of::<xt_tcp>() {
                    return Err(IpTableParseError::MatchSizeMismatch {
                        size: match_size,
                        match_name: "tcp",
                    });
                }
                let tcp = self.view_next_bytes_as::<xt_tcp>()?;
                Matcher::Tcp(tcp)
            }

            ("udp", 0) => {
                if remaining_size < size_of::<xt_udp>() {
                    return Err(IpTableParseError::MatchSizeMismatch {
                        size: match_size,
                        match_name: "udp",
                    });
                }
                let udp = self.view_next_bytes_as::<xt_udp>()?;
                Matcher::Udp(udp)
            }

            ("bpf", 1) => {
                if remaining_size < size_of::<xt_bpf_info_v1>() {
                    return Err(IpTableParseError::MatchSizeMismatch {
                        size: match_size,
                        match_name: "bpf",
                    });
                }
                let bpf = self.view_next_bytes_as::<xt_bpf_info_v1>()?;
                if u32::from(bpf.mode) != starnix_uapi::xt_bpf_modes_XT_BPF_MODE_FD_PINNED {
                    return Err(IpTableParseError::UnsupportedBpfMatcherMode { mode: bpf.mode });
                }

                // SAFETY: reading union variant.
                let cpath = unsafe { &bpf.__bindgen_anon_1.path };
                let path = CStr::from_bytes_until_nul(cpath.as_bytes())
                    .map_err(|_| IpTableParseError::NoNulInEbpfProgramPath)?;
                let path = BString::from(path.to_bytes());

                Matcher::Bpf { path }
            }

            (matcher_name, revision) => {
                track_stub!(
                    TODO("https://fxbug.dev/448203710"),
                    format!("ignored matcher {matcher_name}, revision {revision}").as_str()
                );
                Matcher::Unknown
            }
        };

        // Advance by `remaining_size` to account for padding and unsupported match extensions.
        self.advance_parse_pos(remaining_size)?;
        Ok(matcher)
    }

    /// Parses next bytes as a `xt_entry_target` struct and a specified target struct.
    fn parse_target(&mut self) -> Result<Target, IpTableParseError> {
        let target_info = self.parse_next_bytes_as::<xt_entry_target>()?;

        let target_size = usize::from(target_info.target_size);
        if target_size < size_of::<xt_entry_target>() {
            return Err(IpTableParseError::TargetSizeTooSmall { size: target_size });
        }
        let remaining_size = target_size - size_of::<xt_entry_target>();

        let target_name =
            ascii_to_string(&target_info.name).map_err(IpTableParseError::AsciiConversion)?;
        let target = match (target_name.as_str(), target_info.revision) {
            (TARGET_STANDARD, 0) => {
                if remaining_size < size_of::<VerdictWithPadding>() {
                    return Err(IpTableParseError::TargetSizeMismatch {
                        size: remaining_size,
                        target_name: "standard",
                    });
                }
                let standard_target = self.view_next_bytes_as::<VerdictWithPadding>()?;

                let next_offset = self.bytes_since_first_entry() + remaining_size;
                if standard_target.verdict.try_into() == Ok(next_offset) {
                    Target::Next
                } else {
                    Target::Standard(standard_target.verdict)
                }
            }

            (TARGET_ERROR, 0) => {
                if remaining_size < size_of::<ErrorNameWithPadding>() {
                    return Err(IpTableParseError::TargetSizeMismatch {
                        size: remaining_size,
                        target_name: "error",
                    });
                }
                let error_target = self.view_next_bytes_as::<ErrorNameWithPadding>()?;
                let errorname = ascii_to_string(&error_target.errorname)
                    .map_err(IpTableParseError::AsciiConversion)?;
                Target::Error(errorname)
            }

            (TARGET_REDIRECT, 0) => self.view_as_redirect_target(remaining_size)?,

            (TARGET_TPROXY, 1) => self.view_as_tproxy_target(remaining_size)?,

            (TARGET_MARK, 2) => self.view_as_mark_target(remaining_size)?,

            (TARGET_REJECT, 0) => self.view_as_reject_target(remaining_size)?,

            (target_name, revision) => {
                track_stub!(
                    TODO("https://fxbug.dev/448203710"),
                    format!("ignored unknown target {target_name} with revision: {revision}")
                        .as_str()
                );

                let bytes = self
                    .get_next_bytes(remaining_size)
                    .ok_or(IpTableParseError::TargetSizeMismatch {
                        size: remaining_size,
                        target_name: "unknown",
                    })?
                    .to_vec();
                Target::Unknown { name: target_name.to_owned(), bytes }
            }
        };

        // Advance by `remaining_size` to account for padding and unsupported target extensions.
        self.advance_parse_pos(remaining_size)?;
        Ok(target)
    }

    fn view_as_redirect_target(&self, remaining_size: usize) -> Result<Target, IpTableParseError> {
        match self.protocol {
            Ip::V4 => {
                if remaining_size < size_of::<nf_nat_ipv4_multi_range_compat>() {
                    return Err(IpTableParseError::TargetSizeMismatch {
                        size: remaining_size,
                        target_name: TARGET_REDIRECT,
                    });
                }
                let redirect_target =
                    self.view_next_bytes_as::<nf_nat_ipv4_multi_range_compat>()?;

                // There is always 1 range.
                if redirect_target.rangesize != 1 {
                    return Err(IpTableParseError::InvalidRedirectTargetRangeSize {
                        range_size: redirect_target.rangesize,
                    });
                }
                let range = redirect_target.range[0];
                let flags = NfNatRangeFlags::from_bits(range.flags).ok_or({
                    IpTableParseError::InvalidRedirectTargetFlags { flags: range.flags }
                })?;

                // SAFETY: This union object was created with FromBytes so it's safe to access any
                // variant because all variants must be valid with all bit patterns. All variants of
                // `nf_conntrack_man_proto` are `u16`.
                #[allow(
                    clippy::undocumented_unsafe_blocks,
                    reason = "Force documented unsafe blocks in Starnix"
                )]
                Ok(Target::Redirect(NfNatRange {
                    flags,
                    start: u16::from_be(unsafe { range.min.all }),
                    end: u16::from_be(unsafe { range.max.all }),
                }))
            }
            Ip::V6 => {
                if remaining_size < size_of::<nf_nat_range>() {
                    return Err(IpTableParseError::TargetSizeMismatch {
                        size: remaining_size,
                        target_name: TARGET_REDIRECT,
                    });
                }
                let range = self.view_next_bytes_as::<nf_nat_range>()?;
                let flags = NfNatRangeFlags::from_bits(range.flags).ok_or({
                    IpTableParseError::InvalidRedirectTargetFlags { flags: range.flags }
                })?;

                // SAFETY: This union object was created with FromBytes so it's safe to access any
                // variant because all variants must be valid with all bit patterns. All variants of
                // `nf_conntrack_man_proto` are `u16`.
                #[allow(
                    clippy::undocumented_unsafe_blocks,
                    reason = "Force documented unsafe blocks in Starnix"
                )]
                Ok(Target::Redirect(NfNatRange {
                    flags,
                    start: u16::from_be(unsafe { range.min_proto.all }),
                    end: u16::from_be(unsafe { range.max_proto.all }),
                }))
            }
        }
    }

    fn view_as_tproxy_target(&self, remaining_size: usize) -> Result<Target, IpTableParseError> {
        if remaining_size < size_of::<xt_tproxy_target_info_v1>() {
            return Err(IpTableParseError::TargetSizeMismatch {
                size: remaining_size,
                target_name: "tproxy",
            });
        }
        let tproxy_target = self.view_next_bytes_as::<xt_tproxy_target_info_v1>()?;

        // SAFETY: This union object was created with FromBytes so it's safe to access any variant
        // because all variants must be valid with all bit patterns. `nf_inet_addr` is a IPv4 or
        // or IPv6 address, depending on the protocol of the table.
        let address = if unsafe { tproxy_target.laddr.all } != [0u32; 4] {
            #[allow(
                clippy::undocumented_unsafe_blocks,
                reason = "Force documented unsafe blocks in Starnix"
            )]
            Some(match self.protocol {
                Ip::V4 => ipv4_addr_to_ip_address(unsafe { tproxy_target.laddr.in_ }),
                Ip::V6 => ipv6_addr_to_ip_address(unsafe { tproxy_target.laddr.in6 }),
            })
        } else {
            None
        };
        let port = NonZeroU16::new(u16::from_be(tproxy_target.lport));
        Ok(Target::Tproxy(TproxyInfo { address, port }))
    }

    fn view_as_mark_target(&self, remaining_size: usize) -> Result<Target, IpTableParseError> {
        if remaining_size < size_of::<xt_mark_tginfo2>() {
            return Err(IpTableParseError::TargetSizeMismatch {
                size: remaining_size,
                target_name: "mark",
            });
        }

        let mark_target = self.view_next_bytes_as::<xt_mark_tginfo2>()?;

        Ok(Target::Mark { mark: mark_target.mark, mask: mark_target.mask })
    }

    fn view_as_reject_target(&self, remaining_size: usize) -> Result<Target, IpTableParseError> {
        // `ipt_reject_info` and `ip6t_reject_info` both contain just a single u32 field.
        assert_eq!(size_of::<ipt_reject_info>(), size_of::<ip6t_reject_info>());

        if remaining_size < size_of::<ipt_reject_info>() {
            return Err(IpTableParseError::TargetSizeMismatch {
                size: remaining_size,
                target_name: "reject",
            });
        }

        if self.replace_info.table_id != TableId::Filter {
            return Err(IpTableParseError::RejectRuleOutsideFilterTable);
        }

        let reject_info = self.view_next_bytes_as::<ipt_reject_info>()?;
        let reject_type = reject_type_from_raw(self.protocol, reject_info.with)?;
        Ok(Target::Reject(reject_type))
    }
}

#[derive(Debug)]
pub struct IpTable {
    /// The parser used to translate Linux data into fuchsia.net.filter resources.
    /// Included here as we don't have the reverse translation implemented yet.
    /// TODO(b/307908515): Remove once we can recreate Linux structure from net filter resources.
    pub parser: IptReplaceParser,

    /// `namespace`, `routines` and `rules` make up an IPTable's representation
    /// in fuchsia.net.filter's API, where Namespace stores metadata about the table
    /// like its name, Routine correspond to a chain on the table, and Rule is a rule
    /// on a chain. We can update the table state of the system by dropping the Namespace,
    /// then recreating the Namespace, Routines, and Rules in that order.
    pub namespace: fnet_filter_ext::Namespace,
    pub routines: Vec<fnet_filter_ext::Routine>,
    pub rules: Vec<fnet_filter_ext::Rule>,
}

impl IpTable {
    pub fn from_ipt_replace(
        context: &mut impl IptReplaceContext,
        bytes: Vec<u8>,
    ) -> Result<Self, IpTableParseError> {
        Self::from_parser(context, IptReplaceParser::new_ipv4(bytes)?)
    }

    pub fn from_ip6t_replace(
        context: &mut impl IptReplaceContext,
        bytes: Vec<u8>,
    ) -> Result<Self, IpTableParseError> {
        Self::from_parser(context, IptReplaceParser::new_ipv6(bytes)?)
    }

    fn from_parser(
        context: &mut impl IptReplaceContext,
        mut parser: IptReplaceParser,
    ) -> Result<Self, IpTableParseError> {
        let mut entries = Vec::new();

        // Step 1: Parse entries table bytes into `Entry`s.
        while !parser.finished() {
            entries.push(parser.parse_entry()?);
        }

        if entries.len() != parser.replace_info.num_entries {
            return Err(IpTableParseError::NumEntriesMismatch {
                specified: parser.replace_info.num_entries,
                found: entries.len(),
            });
        }

        // There must be at least 1 entry and the last entry must be an error target named "ERROR".
        Self::check_and_remove_last_entry(&mut entries)?;

        // Step 2: Translate both installed and custom routines. Group remaining `Entry`s with their
        // respective routines.
        let mut installed_routines = InstalledRoutines::new(&parser)?;
        let mut custom_routines = Vec::new();

        for entry in entries {
            if let Some(installed_routine) = installed_routines.is_installed(entry.byte_pos)? {
                // Entries on installed routines cannot define a new custom routine.
                if let Target::Error(error_name) = entry.target {
                    return Err(IpTableParseError::UnexpectedErrorTarget { error_name });
                }

                installed_routine.entries.push(entry);
            } else if let Target::Error(chain_name) = &entry.target {
                if !entry.matchers.is_empty() {
                    return Err(IpTableParseError::ErrorEntryHasMatchers);
                }

                custom_routines.push(CustomRoutine {
                    routine: fnet_filter_ext::Routine {
                        id: fnet_filter_ext::RoutineId {
                            namespace: parser.get_namespace_id(),
                            name: chain_name.clone(),
                        },
                        routine_type: parser.get_custom_routine_type(),
                    },
                    entries: Vec::new(),
                });
            } else {
                let Some(current_routine) = custom_routines.last_mut() else {
                    return Err(IpTableParseError::RuleBeforeFirstChain);
                };

                current_routine.entries.push(entry);
            }
        }

        // Collect installed routines and custom routines. Build a custom routine lookup table to
        // translate JUMP rules that refer to routines by the byte position of their first entry.
        // Only custom routines can be JUMPed to.
        let mut routines = installed_routines.routines();
        let mut custom_routine_map = HashMap::new();

        for custom_routine in &custom_routines {
            if let Some(first_entry) = custom_routine.entries.first() {
                custom_routine_map
                    .insert(first_entry.byte_pos, custom_routine.routine.id.name.to_owned());
            } else {
                // All custom routines must have at least 1 rule.
                return Err(IpTableParseError::ChainHasNoPolicy {
                    chain_name: custom_routine.routine.id.name.to_owned(),
                });
            }

            routines.push(custom_routine.routine.clone());
        }

        // Step 3: Translate rule entries into `fnet_filter_ext::Rule`s.
        let mut rules = Vec::new();

        for installed_routine in installed_routines.into_iter() {
            for (index, entry) in installed_routine.entries.into_iter().enumerate() {
                if let Some(rule) = entry.translate_into_rule(
                    context,
                    installed_routine.routine.id.clone(),
                    index,
                    &custom_routine_map,
                )? {
                    rules.push(rule);
                }
            }
        }
        for custom_routine in custom_routines {
            for (index, entry) in custom_routine.entries.into_iter().enumerate() {
                if let Some(rule) = entry.translate_into_rule(
                    context,
                    custom_routine.routine.id.clone(),
                    index,
                    &custom_routine_map,
                )? {
                    rules.push(rule);
                }
            }
        }

        let namespace = parser.get_namespace();
        Ok(IpTable { parser, namespace, routines, rules })
    }

    pub fn into_changes(self) -> impl Iterator<Item = fnet_filter_ext::Change> {
        [
            // Firstly, remove the existing table, along with all of its routines and rules.
            // We will call Commit with idempotent=true so that this would succeed even if
            // the table did not exist prior to this change.
            fnet_filter_ext::Change::Remove(fnet_filter_ext::ResourceId::Namespace(
                self.namespace.id.clone(),
            )),
            // Recreate the table.
            fnet_filter_ext::Change::Create(fnet_filter_ext::Resource::Namespace(self.namespace)),
        ]
        .into_iter()
        .chain(
            self.routines
                .into_iter()
                .map(fnet_filter_ext::Resource::Routine)
                .map(fnet_filter_ext::Change::Create),
        )
        .chain(
            self.rules
                .into_iter()
                .map(fnet_filter_ext::Resource::Rule)
                .map(fnet_filter_ext::Change::Create),
        )
    }

    fn check_and_remove_last_entry(entries: &mut Vec<Entry>) -> Result<(), IpTableParseError> {
        let last_entry = entries.last().ok_or(IpTableParseError::NoTrailingErrorTarget)?;
        if !last_entry.matchers.is_empty() {
            return Err(IpTableParseError::ErrorEntryHasMatchers);
        }
        if let Target::Error(chain_name) = &last_entry.target {
            if chain_name.as_str() != TARGET_ERROR {
                return Err(IpTableParseError::NoTrailingErrorTarget);
            }
        } else {
            return Err(IpTableParseError::NoTrailingErrorTarget);
        }
        entries.truncate(entries.len() - 1);
        Ok(())
    }
}

/// A user-defined routine and the rule-specifications that belong to it.
struct CustomRoutine {
    routine: fnet_filter_ext::Routine,
    entries: Vec<Entry>,
}

/// A built-in installed routine, specified by the byte range of the entries that belong to it.
struct InstalledRoutine {
    routine: fnet_filter_ext::Routine,
    byte_range: RangeInclusive<usize>,
    entries: Vec<Entry>,
}

struct InstalledRoutines {
    prerouting: Option<InstalledRoutine>,
    input: Option<InstalledRoutine>,
    forward: Option<InstalledRoutine>,
    output: Option<InstalledRoutine>,
    postrouting: Option<InstalledRoutine>,
}

impl InstalledRoutines {
    fn new(parser: &IptReplaceParser) -> Result<Self, IpTableParseError> {
        match parser.table_id() {
            TableId::Nat => {
                if parser.replace_info.valid_hooks != NfIpHooks::NAT {
                    return Err(IpTableParseError::InvalidHooksForTable {
                        hooks: parser.replace_info.valid_hooks.bits(),
                        table_name: TABLE_NAT,
                    });
                }
                Self::new_nat(parser)
            }
            TableId::Mangle => {
                if parser.replace_info.valid_hooks != NfIpHooks::MANGLE {
                    return Err(IpTableParseError::InvalidHooksForTable {
                        hooks: parser.replace_info.valid_hooks.bits(),
                        table_name: TABLE_MANGLE,
                    });
                }
                Self::new_ip(parser, nf_ip_hook_priorities_NF_IP_PRI_MANGLE)
            }
            TableId::Filter => {
                if parser.replace_info.valid_hooks != NfIpHooks::FILTER {
                    return Err(IpTableParseError::InvalidHooksForTable {
                        hooks: parser.replace_info.valid_hooks.bits(),
                        table_name: TABLE_FILTER,
                    });
                }
                Self::new_ip(parser, nf_ip_hook_priorities_NF_IP_PRI_FILTER)
            }
            TableId::Raw => {
                if parser.replace_info.valid_hooks != NfIpHooks::RAW {
                    return Err(IpTableParseError::InvalidHooksForTable {
                        hooks: parser.replace_info.valid_hooks.bits(),
                        table_name: TABLE_RAW,
                    });
                }
                Self::new_ip(parser, nf_ip_hook_priorities_NF_IP_PRI_RAW)
            }
        }
    }

    fn new_nat(parser: &IptReplaceParser) -> Result<Self, IpTableParseError> {
        let prerouting = Some(InstalledRoutine {
            routine: fnet_filter_ext::Routine {
                id: fnet_filter_ext::RoutineId {
                    namespace: parser.get_namespace_id(),
                    name: CHAIN_PREROUTING.to_string(),
                },
                routine_type: fnet_filter_ext::RoutineType::Nat(Some(
                    fnet_filter_ext::InstalledNatRoutine {
                        hook: fnet_filter_ext::NatHook::Ingress,
                        priority: nf_ip_hook_priorities_NF_IP_PRI_NAT_DST,
                    },
                )),
            },
            byte_range: Self::make_byte_range(parser, NF_IP_PRE_ROUTING)?,
            entries: Vec::new(),
        });
        let input = Some(InstalledRoutine {
            routine: fnet_filter_ext::Routine {
                id: fnet_filter_ext::RoutineId {
                    namespace: parser.get_namespace_id(),
                    name: CHAIN_INPUT.to_string(),
                },
                routine_type: fnet_filter_ext::RoutineType::Nat(Some(
                    fnet_filter_ext::InstalledNatRoutine {
                        hook: fnet_filter_ext::NatHook::LocalIngress,
                        priority: nf_ip_hook_priorities_NF_IP_PRI_NAT_SRC,
                    },
                )),
            },
            byte_range: Self::make_byte_range(parser, NF_IP_LOCAL_IN)?,
            entries: Vec::new(),
        });
        let forward = None;
        let output = Some(InstalledRoutine {
            routine: fnet_filter_ext::Routine {
                id: fnet_filter_ext::RoutineId {
                    namespace: parser.get_namespace_id(),
                    name: CHAIN_OUTPUT.to_string(),
                },
                routine_type: fnet_filter_ext::RoutineType::Nat(Some(
                    fnet_filter_ext::InstalledNatRoutine {
                        hook: fnet_filter_ext::NatHook::LocalEgress,
                        priority: nf_ip_hook_priorities_NF_IP_PRI_NAT_DST,
                    },
                )),
            },
            byte_range: Self::make_byte_range(parser, NF_IP_LOCAL_OUT)?,
            entries: Vec::new(),
        });

        let postrouting = Some(InstalledRoutine {
            routine: fnet_filter_ext::Routine {
                id: fnet_filter_ext::RoutineId {
                    namespace: parser.get_namespace_id(),
                    name: CHAIN_POSTROUTING.to_string(),
                },
                routine_type: fnet_filter_ext::RoutineType::Nat(Some(
                    fnet_filter_ext::InstalledNatRoutine {
                        hook: fnet_filter_ext::NatHook::Egress,
                        priority: nf_ip_hook_priorities_NF_IP_PRI_NAT_SRC,
                    },
                )),
            },
            byte_range: Self::make_byte_range(parser, NF_IP_POST_ROUTING)?,
            entries: Vec::new(),
        });

        Ok(Self { prerouting, input, forward, output, postrouting })
    }

    fn new_ip(parser: &IptReplaceParser, priority: i32) -> Result<Self, IpTableParseError> {
        let prerouting = if parser.replace_info.valid_hooks.contains(NfIpHooks::PREROUTING) {
            Some(InstalledRoutine {
                routine: fnet_filter_ext::Routine {
                    id: fnet_filter_ext::RoutineId {
                        namespace: parser.get_namespace_id(),
                        name: CHAIN_PREROUTING.to_string(),
                    },
                    routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                        fnet_filter_ext::InstalledIpRoutine {
                            hook: fnet_filter_ext::IpHook::Ingress,
                            priority,
                        },
                    )),
                },
                byte_range: Self::make_byte_range(parser, NF_IP_PRE_ROUTING)?,
                entries: Vec::new(),
            })
        } else {
            None
        };
        let input = if parser.replace_info.valid_hooks.contains(NfIpHooks::INPUT) {
            Some(InstalledRoutine {
                routine: fnet_filter_ext::Routine {
                    id: fnet_filter_ext::RoutineId {
                        namespace: parser.get_namespace_id(),
                        name: CHAIN_INPUT.to_string(),
                    },
                    routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                        fnet_filter_ext::InstalledIpRoutine {
                            hook: fnet_filter_ext::IpHook::LocalIngress,
                            priority,
                        },
                    )),
                },
                byte_range: Self::make_byte_range(parser, NF_IP_LOCAL_IN)?,
                entries: Vec::new(),
            })
        } else {
            None
        };
        let forward = if parser.replace_info.valid_hooks.contains(NfIpHooks::FORWARD) {
            Some(InstalledRoutine {
                routine: fnet_filter_ext::Routine {
                    id: fnet_filter_ext::RoutineId {
                        namespace: parser.get_namespace_id(),
                        name: CHAIN_FORWARD.to_string(),
                    },
                    routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                        fnet_filter_ext::InstalledIpRoutine {
                            hook: fnet_filter_ext::IpHook::Forwarding,
                            priority,
                        },
                    )),
                },
                byte_range: Self::make_byte_range(parser, NF_IP_FORWARD)?,
                entries: Vec::new(),
            })
        } else {
            None
        };
        let output = if parser.replace_info.valid_hooks.contains(NfIpHooks::OUTPUT) {
            Some(InstalledRoutine {
                routine: fnet_filter_ext::Routine {
                    id: fnet_filter_ext::RoutineId {
                        namespace: parser.get_namespace_id(),
                        name: CHAIN_OUTPUT.to_string(),
                    },
                    routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                        fnet_filter_ext::InstalledIpRoutine {
                            hook: fnet_filter_ext::IpHook::LocalEgress,
                            priority,
                        },
                    )),
                },
                byte_range: Self::make_byte_range(parser, NF_IP_LOCAL_OUT)?,
                entries: Vec::new(),
            })
        } else {
            None
        };
        let postrouting = if parser.replace_info.valid_hooks.contains(NfIpHooks::POSTROUTING) {
            Some(InstalledRoutine {
                routine: fnet_filter_ext::Routine {
                    id: fnet_filter_ext::RoutineId {
                        namespace: parser.get_namespace_id(),
                        name: CHAIN_POSTROUTING.to_string(),
                    },
                    routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                        fnet_filter_ext::InstalledIpRoutine {
                            hook: fnet_filter_ext::IpHook::Egress,
                            priority,
                        },
                    )),
                },
                byte_range: Self::make_byte_range(parser, NF_IP_POST_ROUTING)?,
                entries: Vec::new(),
            })
        } else {
            None
        };

        Ok(Self { prerouting, input, forward, output, postrouting })
    }

    /// Returns a new byte range for the i-th hook. `index` must be less than `NF_IP_NUMHOOKS`.
    fn make_byte_range(
        parser: &IptReplaceParser,
        index: u32,
    ) -> Result<RangeInclusive<usize>, IpTableParseError> {
        assert!(index < NF_IP_NUMHOOKS);

        let start = parser.replace_info.hook_entry[index as usize] as usize;
        let end = parser.replace_info.underflow[index as usize] as usize;

        // Both start and end must point to an Entry.
        if start > end || !parser.is_valid_offset(start) || !parser.is_valid_offset(end) {
            return Err(IpTableParseError::InvalidHookEntryOrUnderflow { index, start, end });
        }

        Ok(start..=end)
    }

    fn iter(&self) -> impl Iterator<Item = &InstalledRoutine> {
        [&self.prerouting, &self.input, &self.forward, &self.output, &self.postrouting]
            .into_iter()
            .filter_map(|installed_routine| installed_routine.as_ref())
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = &mut InstalledRoutine> {
        [
            &mut self.prerouting,
            &mut self.input,
            &mut self.forward,
            &mut self.output,
            &mut self.postrouting,
        ]
        .into_iter()
        .filter_map(|installed_routine| installed_routine.as_mut())
    }

    fn into_iter(self) -> impl Iterator<Item = InstalledRoutine> {
        [self.prerouting, self.input, self.forward, self.output, self.postrouting]
            .into_iter()
            .filter_map(|installed_routine| installed_routine)
    }

    /// Given an `offset`, determine whether it belongs in exactly 1 installed routine of the table.
    /// Returns a mutable reference of the installed routine.
    /// Errors if an offset is found to be part of multiple installed routines.
    fn is_installed(
        &mut self,
        offset: usize,
    ) -> Result<Option<&mut InstalledRoutine>, IpTableParseError> {
        self.iter_mut()
            .filter(|routine| routine.byte_range.contains(&offset))
            .at_most_one()
            .map_err(|_| IpTableParseError::HookRangesOverlap { offset })
    }

    fn routines(&self) -> Vec<fnet_filter_ext::Routine> {
        self.iter().map(|installed_routine| installed_routine.routine.clone()).collect()
    }
}

impl Entry {
    // Creates a `fnet_filter_ext::Matchers` object and populate it with IP matchers in `ip_info`,
    // and extension matchers like `xt_tcp`.
    // Returns None if any unsupported matchers are found.
    fn get_rule_matchers(
        &self,
        context: &mut impl IptReplaceContext,
    ) -> Result<Option<fnet_filter_ext::Matchers>, IpTableParseError> {
        let mut matchers = fnet_filter_ext::Matchers::default();

        if self.populate_matchers_with_ipt_ip(&mut matchers)?.is_none() {
            return Ok(None);
        }
        if self.populate_matchers_with_match_extensions(context, &mut matchers)?.is_none() {
            return Ok(None);
        }

        Ok(Some(matchers))
    }

    // Creates a `fnet_filter_ext::Action` from `target`.
    // Returns None if target is unsupported.
    fn get_rule_action(
        &self,
        routine_map: &HashMap<usize, String>,
    ) -> Result<Option<fnet_filter_ext::Action>, IpTableParseError> {
        match self.target {
            Target::Unknown { name: _, bytes: _ } => Ok(None),

            // Error targets should already be translated into `Routine`s.
            Target::Error(_) => unreachable!(),

            Target::Standard(verdict) => Self::translate_standard_target(verdict, routine_map),

            Target::Redirect(ref range) => {
                if !range.flags.contains(NfNatRangeFlags::PROTO_SPECIFIED) {
                    Ok(Some(fnet_filter_ext::Action::Redirect { dst_port: None }))
                } else {
                    let invalid_range_fn = || IpTableParseError::InvalidRedirectTargetRange {
                        start: range.start,
                        end: range.end,
                    };

                    if range.start > range.end {
                        return Err(invalid_range_fn());
                    }
                    let start = NonZeroU16::new(range.start).ok_or_else(invalid_range_fn)?;
                    let end = NonZeroU16::new(range.end).ok_or_else(invalid_range_fn)?;
                    Ok(Some(fnet_filter_ext::Action::Redirect {
                        dst_port: Some(fnet_filter_ext::PortRange(start..=end)),
                    }))
                }
            }

            Target::Tproxy(ref tproxy_info) => {
                let tproxy = match (tproxy_info.address, tproxy_info.port) {
                    (None, None) => return Err(IpTableParseError::InvalidTproxyZeroAddressAndPort),
                    (None, Some(port)) => fnet_filter_ext::TransparentProxy::LocalPort(port),
                    (Some(address), None) => fnet_filter_ext::TransparentProxy::LocalAddr(address),
                    (Some(address), Some(port)) => {
                        fnet_filter_ext::TransparentProxy::LocalAddrAndPort(address, port)
                    }
                };

                Ok(Some(fnet_filter_ext::Action::TransparentProxy(tproxy)))
            }

            Target::Mark { mark, mask } => Ok(Some(fnet_filter_ext::Action::Mark {
                domain: fnet::MarkDomain::Mark1,
                action: fnet_filter_ext::MarkAction::SetMark { mark, clearing_mask: mask },
            })),

            Target::Reject(reject_type) => Ok(Some(fnet_filter_ext::Action::Reject(reject_type))),

            Target::Next => Ok(Some(fnet_filter_ext::Action::None)),
        }
    }

    fn populate_matchers_with_ipt_ip(
        &self,
        matchers: &mut fnet_filter_ext::Matchers,
    ) -> Result<Option<()>, IpTableParseError> {
        let ip_info = &self.entry_info.ip_info;

        if let Some(subnet) = ip_info.src_subnet {
            let subnet = fnet_matchers_ext::Subnet::try_from(subnet)
                .map_err(|err| IpTableParseError::FidlConversion(err.into()))?;
            matchers.src_addr = Some(fnet_matchers_ext::Address {
                matcher: fnet_matchers_ext::AddressMatcherType::Subnet(subnet),
                invert: ip_info.inverse_flags.contains(IptIpInverseFlags::SOURCE_IP_ADDRESS),
            });
        }

        if let Some(subnet) = ip_info.dst_subnet {
            let subnet = fnet_matchers_ext::Subnet::try_from(subnet)
                .map_err(|err| IpTableParseError::FidlConversion(err.into()))?;
            matchers.dst_addr = Some(fnet_matchers_ext::Address {
                matcher: fnet_matchers_ext::AddressMatcherType::Subnet(subnet),
                invert: ip_info.inverse_flags.contains(IptIpInverseFlags::DESTINATION_IP_ADDRESS),
            });
        }

        if let Some(ref interface) = ip_info.in_interface {
            if ip_info.inverse_flags.contains(IptIpInverseFlags::INPUT_INTERFACE) {
                track_stub!(
                    TODO("https://fxbug.dev/448203710"),
                    "ignored rule-specification",
                    IptIpInverseFlags::INPUT_INTERFACE
                );
                return Ok(None);
            }
            matchers.in_interface = Some(fnet_matchers_ext::Interface::Name(interface.clone()))
        }

        if let Some(ref interface) = ip_info.out_interface {
            if ip_info.inverse_flags.contains(IptIpInverseFlags::OUTPUT_INTERFACE) {
                track_stub!(
                    TODO("https://fxbug.dev/448203710"),
                    "ignored rule-specification",
                    IptIpInverseFlags::OUTPUT_INTERFACE
                );
                return Ok(None);
            }
            matchers.out_interface = Some(fnet_matchers_ext::Interface::Name(interface.clone()))
        }

        if ip_info.should_match_protocol() {
            match ip_info.protocol {
                // matches both TCP and UDP, which is true by default.
                IPPROTO_IP => {}

                IPPROTO_TCP => {
                    matchers.transport_protocol = Some(fnet_matchers_ext::TransportProtocol::Tcp {
                        // These fields are set later by `xt_tcp` match extension, if present.
                        src_port: None,
                        dst_port: None,
                    });
                }

                IPPROTO_UDP => {
                    matchers.transport_protocol = Some(fnet_matchers_ext::TransportProtocol::Udp {
                        // These fields are set later by `xt_udp` match extension, if present.
                        src_port: None,
                        dst_port: None,
                    });
                }

                IPPROTO_ICMP => {
                    matchers.transport_protocol = Some(fnet_matchers_ext::TransportProtocol::Icmp)
                }

                IPPROTO_ICMPV6 => {
                    matchers.transport_protocol = Some(fnet_matchers_ext::TransportProtocol::Icmpv6)
                }

                protocol => {
                    track_stub!(
                        TODO("https://fxbug.dev/448203710"),
                        "ignored rule-specification with protocol",
                        protocol
                    );
                    return Ok(None);
                }
            };
        }

        Ok(Some(()))
    }

    fn populate_matchers_with_match_extensions(
        &self,
        context: &mut impl IptReplaceContext,
        fnet_filter_matchers: &mut fnet_filter_ext::Matchers,
    ) -> Result<Option<()>, IpTableParseError> {
        for matcher in &self.matchers {
            match matcher {
                Matcher::Tcp(xt_tcp {
                    spts,
                    dpts,
                    invflags,
                    option: _,
                    flg_mask: _,
                    flg_cmp: _,
                }) => {
                    // TCP match extension is only valid if protocol is specified as TCP.
                    let Some(fnet_matchers_ext::TransportProtocol::Tcp { src_port, dst_port }) =
                        fnet_filter_matchers.transport_protocol.as_mut()
                    else {
                        return Err(IpTableParseError::MatchExtensionDoesNotMatchProtocol);
                    };

                    let inverse_flags = XtTcpInverseFlags::from_bits((*invflags).into())
                        .ok_or(IpTableParseError::InvalidXtTcpInverseFlags { flags: *invflags })?;

                    if src_port.is_some() || dst_port.is_some() {
                        return Err(IpTableParseError::MatchExtensionOverwrite);
                    }
                    src_port.replace(
                        fnet_matchers_ext::Port::new(
                            spts[0],
                            spts[1],
                            inverse_flags.contains(XtTcpInverseFlags::SOURCE_PORT),
                        )
                        .map_err(IpTableParseError::PortMatcher)?,
                    );
                    dst_port.replace(
                        fnet_matchers_ext::Port::new(
                            dpts[0],
                            dpts[1],
                            inverse_flags.contains(XtTcpInverseFlags::DESTINATION_PORT),
                        )
                        .map_err(IpTableParseError::PortMatcher)?,
                    );
                }
                Matcher::Udp(xt_udp { spts, dpts, invflags, __bindgen_padding_0 }) => {
                    // UDP match extension is only valid if protocol is specified as UDP.
                    let Some(fnet_matchers_ext::TransportProtocol::Udp { src_port, dst_port }) =
                        fnet_filter_matchers.transport_protocol.as_mut()
                    else {
                        return Err(IpTableParseError::MatchExtensionDoesNotMatchProtocol);
                    };

                    let inverse_flags = XtUdpInverseFlags::from_bits((*invflags).into())
                        .ok_or(IpTableParseError::InvalidXtUdpInverseFlags { flags: *invflags })?;

                    if src_port.is_some() || dst_port.is_some() {
                        return Err(IpTableParseError::MatchExtensionOverwrite);
                    }
                    src_port.replace(
                        fnet_matchers_ext::Port::new(
                            spts[0],
                            spts[1],
                            inverse_flags.contains(XtUdpInverseFlags::SOURCE_PORT),
                        )
                        .map_err(IpTableParseError::PortMatcher)?,
                    );
                    dst_port.replace(
                        fnet_matchers_ext::Port::new(
                            dpts[0],
                            dpts[1],
                            inverse_flags.contains(XtUdpInverseFlags::DESTINATION_PORT),
                        )
                        .map_err(IpTableParseError::PortMatcher)?,
                    );
                }
                Matcher::Bpf { path } => {
                    fnet_filter_matchers.ebpf_program =
                        Some(context.resolve_ebpf_socket_filter(path)?);
                }
                Matcher::Unknown => return Ok(None),
            }
        }

        Ok(Some(()))
    }

    fn translate_standard_target(
        verdict: i32,
        routine_map: &HashMap<usize, String>,
    ) -> Result<Option<fnet_filter_ext::Action>, IpTableParseError> {
        match verdict {
            // A 0 or positive verdict is a JUMP to another chain or rule, but jumping to another
            // rule is not supported by fuchsia.net.filter.
            verdict if verdict >= 0 => {
                let jump_target = usize::try_from(verdict).expect("positive i32 fits into usize");
                if let Some(routine_name) = routine_map.get(&jump_target) {
                    Ok(Some(fnet_filter_ext::Action::Jump(routine_name.clone())))
                } else {
                    Err(IpTableParseError::InvalidJumpTarget { jump_target })
                }
            }

            // A negative verdict is one of the builtin targets.
            VERDICT_DROP => Ok(Some(fnet_filter_ext::Action::Drop)),

            VERDICT_ACCEPT => Ok(Some(fnet_filter_ext::Action::Accept)),

            VERDICT_QUEUE => {
                track_stub!(
                    TODO("https://fxbug.dev/448203710"),
                    "ignored unsupported QUEUE target"
                );
                Ok(None)
            }

            VERDICT_RETURN => Ok(Some(fnet_filter_ext::Action::Return)),

            verdict => Err(IpTableParseError::InvalidVerdict { verdict }),
        }
    }

    fn translate_into_rule(
        self,
        context: &mut impl IptReplaceContext,
        routine_id: fnet_filter_ext::RoutineId,
        index: usize,
        routine_map: &HashMap<usize, String>,
    ) -> Result<Option<fnet_filter_ext::Rule>, IpTableParseError> {
        let index = u32::try_from(index).map_err(|_| IpTableParseError::TooManyRules)?;

        let Some(matchers) = self.get_rule_matchers(context)? else {
            return Ok(None);
        };

        let Some(action) = self.get_rule_action(routine_map)? else {
            return Ok(None);
        };

        Ok(Some(fnet_filter_ext::Rule {
            id: fnet_filter_ext::RuleId { routine: routine_id, index },
            matchers,
            action,
        }))
    }
}

// Errors if any character is not in ASCII range (0-127).
fn ascii_to_bytes(chars: &[c_char]) -> Result<Vec<u8>, AsciiConversionError> {
    if chars.iter().any(|&c| c as u8 > 127) {
        return Err(AsciiConversionError::NonAsciiChar);
    }
    Ok(chars.as_bytes().to_owned())
}

fn ascii_to_string(chars: &[c_char]) -> Result<String, AsciiConversionError> {
    let bytes = ascii_to_bytes(chars)?;
    let c_str = CStr::from_bytes_until_nul(&bytes)
        .map_err(|_| AsciiConversionError::NulByteNotFound { chars: chars.to_vec() })?;
    // We've verified that all characters are in ASCII range, so the conversion should succeed.
    Ok(c_str.to_str().expect("failed CStr to Str conversion").to_owned())
}

pub const fn string_to_ascii_buffer<const N: usize>(
    string: &str,
) -> Result<[c_char; N], AsciiConversionError> {
    let bytes = string.as_bytes();
    if bytes.len() > N - 1 {
        return Err(AsciiConversionError::BufferTooSmall {
            buffer_size: N,
            data_size: bytes.len() + 1,
        });
    }
    let mut chars = [0; N];

    // Using `while` loop instead of `for` loop allows to make this function `const`.
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        match byte {
            0 => return Err(AsciiConversionError::NulByteInString { pos: i }),
            byte if byte > 127 => return Err(AsciiConversionError::NonAsciiChar),
            _ => (),
        }
        chars[i] = byte as c_char;
        i += 1;
    }
    Ok(chars)
}

// Assumes `addr` is big endian.
fn ipv4_addr_to_ip_address(addr: in_addr) -> fnet::IpAddress {
    fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: u32::from_be(addr.s_addr).to_be_bytes() })
}

// Assumes `addr` is big endian.
fn ipv6_addr_to_ip_address(addr: in6_addr) -> fnet::IpAddress {
    // SAFETY: This union object was created with FromBytes so it's safe to access any variant
    // because all variants must be valid with all bit patterns. `in6_addr__bindgen_ty_1` is an IPv6
    // address, represented as sixteen 8-bit octets, or eight 16-bit segments, or four 32-bit words.
    // All variants have the same size and represent the same address.
    let addr_bytes = unsafe { addr.in6_u.u6_addr8 };
    fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr: addr_bytes })
}

// Assumes `mask` is big endian.
fn ipv4_mask_to_prefix_len(mask: in_addr) -> Result<u8, IpAddressConversionError> {
    let mask = u32::from_be(mask.s_addr);

    // Check that all 1's in the mask are before all 0's.
    // To do this, we can simply find if its 2-complement is a power of 2.
    if !mask.wrapping_neg().is_power_of_two() {
        return Err(IpAddressConversionError::IpV4SubnetMaskHasNonPrefixBits { mask });
    }

    // Impossible to have more 1's in a `u32` than 255.
    Ok(mask.count_ones() as u8)
}

// Assumes `mask` is in big endian order.
fn ipv6_mask_to_prefix_len(mask: [u8; 16]) -> Result<u8, IpAddressConversionError> {
    let mask = u128::from_be_bytes(mask);

    // Check that all 1's in the mask are before all 0's.
    // To do this, we can simply find if its 2-complement is a power of 2.
    if !mask.wrapping_neg().is_power_of_two() {
        return Err(IpAddressConversionError::IpV6SubnetMaskHasNonPrefixBits { mask });
    }

    // Impossible to have more 1's in a `u128` than 255.
    Ok(mask.count_ones() as u8)
}

// Converts an IPv4 address and subnet mask to fuchsia.net.Subnet.
//
// Assumes `ipv4_addr` and `subnet_mask` are both big endian. Returns Ok(None) if subnet mask is 0.
// Errors if not all 1's in the subnet_mask are before all 0's.
pub fn ipv4_to_subnet(
    addr: in_addr,
    mask: in_addr,
) -> Result<Option<fnet::Subnet>, IpAddressConversionError> {
    if mask.s_addr == 0 {
        Ok(None)
    } else {
        Ok(Some(fnet::Subnet {
            addr: ipv4_addr_to_ip_address(addr),
            prefix_len: ipv4_mask_to_prefix_len(mask)?,
        }))
    }
}

pub fn ipv6_to_subnet(
    addr: in6_addr,
    mask: in6_addr,
) -> Result<Option<fnet::Subnet>, IpAddressConversionError> {
    // SAFETY: This union object was created with FromBytes so it's safe to access any variant
    // because all variants must be valid with all bit patterns. `in6_addr__bindgen_ty_1` is an IPv6
    // address, represented as sixteen 8-bit octets, or eight 16-bit segments, or four 32-bit words.
    // All variants have the same size and represent the same address.
    let mask_bytes = unsafe { mask.in6_u.u6_addr8 };

    if mask_bytes == [0u8; 16] {
        Ok(None)
    } else {
        Ok(Some(fnet::Subnet {
            addr: ipv6_addr_to_ip_address(addr),
            prefix_len: ipv6_mask_to_prefix_len(mask_bytes)?,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itertools::Itertools;
    use net_declare::{fidl_ip, fidl_subnet};
    use starnix_uapi::{
        IP6T_F_PROTO, IPT_INV_SRCIP, XT_TCP_INV_DSTPT, c_char, in_addr, in6_addr__bindgen_ty_1,
        ipt_entry, ipt_ip, ipt_replace, xt_tcp, xt_udp,
    };
    use test_case::test_case;
    use {fidl_fuchsia_net as fnet, fidl_fuchsia_net_filter_ext as fnet_filter_ext};

    const IPV4_SUBNET: fnet::Subnet = fidl_subnet!("192.0.2.0/24");
    const IPV4_ADDR: fnet::IpAddress = fidl_ip!("192.0.2.0");
    const IPV6_SUBNET: fnet::Subnet = fidl_subnet!("2001:db8::/32");
    const IPV6_ADDR: fnet::IpAddress = fidl_ip!("2001:db8::");
    const PORT: u16 = 2345u16.to_be();
    const PORT_RANGE_START: u16 = 2000u16.to_be();
    const PORT_RANGE_END: u16 = 3000u16.to_be();
    const NONZERO_PORT: NonZeroU16 = NonZeroU16::new(2345).unwrap();
    const NONZERO_PORT_RANGE: RangeInclusive<NonZeroU16> =
        RangeInclusive::new(NonZeroU16::new(2000).unwrap(), NonZeroU16::new(3000).unwrap());
    const MARK: u32 = 1;
    const MASK: u32 = 2;

    const STANDARD_TARGET_SIZE: usize =
        size_of::<xt_entry_target>() + size_of::<VerdictWithPadding>();

    fn extend_with_standard_verdict(bytes: &mut Vec<u8>, verdict: i32) {
        bytes.extend_from_slice(
            xt_entry_target { target_size: STANDARD_TARGET_SIZE as u16, ..Default::default() }
                .as_bytes(),
        );
        bytes.extend_from_slice(VerdictWithPadding { verdict, ..Default::default() }.as_bytes());
    }

    fn extend_with_error_name(bytes: &mut Vec<u8>, error_name: &str) {
        bytes.extend_from_slice(
            xt_entry_target {
                target_size: 64,
                name: string_to_ascii_buffer(TARGET_ERROR).unwrap(),
                revision: 0,
            }
            .as_bytes(),
        );
        bytes.extend_from_slice(
            ErrorNameWithPadding {
                errorname: string_to_ascii_buffer(error_name).unwrap(),
                ..Default::default()
            }
            .as_bytes(),
        );
    }

    fn extend_with_standard_target_ipv4_entry(bytes: &mut Vec<u8>, verdict: i32) {
        bytes.extend_from_slice(
            ipt_entry { target_offset: 112, next_offset: 152, ..Default::default() }.as_bytes(),
        );
        extend_with_standard_verdict(bytes, verdict);
    }

    fn extend_with_standard_target_ipv6_entry(bytes: &mut Vec<u8>, verdict: i32) {
        bytes.extend_from_slice(
            ip6t_entry { target_offset: 168, next_offset: 208, ..Default::default() }.as_bytes(),
        );
        extend_with_standard_verdict(bytes, verdict);
    }

    fn extend_with_error_target_ipv4_entry(bytes: &mut Vec<u8>, error_name: &str) {
        bytes.extend_from_slice(
            ipt_entry { target_offset: 112, next_offset: 176, ..Default::default() }.as_bytes(),
        );
        extend_with_error_name(bytes, error_name);
    }

    fn extend_with_error_target_ipv6_entry(bytes: &mut Vec<u8>, error_name: &str) {
        bytes.extend_from_slice(
            ip6t_entry { target_offset: 168, next_offset: 232, ..Default::default() }.as_bytes(),
        );
        extend_with_error_name(bytes, error_name);
    }

    fn filter_namespace_v4() -> fnet_filter_ext::Namespace {
        fnet_filter_ext::Namespace {
            id: fnet_filter_ext::NamespaceId("ipv4-filter".to_owned()),
            domain: fnet_filter_ext::Domain::Ipv4,
        }
    }

    fn filter_namespace_v6() -> fnet_filter_ext::Namespace {
        fnet_filter_ext::Namespace {
            id: fnet_filter_ext::NamespaceId("ipv6-filter".to_owned()),
            domain: fnet_filter_ext::Domain::Ipv6,
        }
    }

    fn filter_input_routine(namespace: &fnet_filter_ext::Namespace) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_INPUT.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                fnet_filter_ext::InstalledIpRoutine {
                    hook: fnet_filter_ext::IpHook::LocalIngress,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_FILTER,
                },
            )),
        }
    }

    fn filter_forward_routine(namespace: &fnet_filter_ext::Namespace) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_FORWARD.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                fnet_filter_ext::InstalledIpRoutine {
                    hook: fnet_filter_ext::IpHook::Forwarding,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_FILTER,
                },
            )),
        }
    }

    fn filter_output_routine(namespace: &fnet_filter_ext::Namespace) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_OUTPUT.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                fnet_filter_ext::InstalledIpRoutine {
                    hook: fnet_filter_ext::IpHook::LocalEgress,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_FILTER,
                },
            )),
        }
    }

    fn filter_custom_routine(
        namespace: &fnet_filter_ext::Namespace,
        chain_name: &str,
    ) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: chain_name.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Ip(None),
        }
    }

    fn mangle_namespace_v4() -> fnet_filter_ext::Namespace {
        fnet_filter_ext::Namespace {
            id: fnet_filter_ext::NamespaceId("ipv4-mangle".to_owned()),
            domain: fnet_filter_ext::Domain::Ipv4,
        }
    }

    fn mangle_namespace_v6() -> fnet_filter_ext::Namespace {
        fnet_filter_ext::Namespace {
            id: fnet_filter_ext::NamespaceId("ipv6-mangle".to_owned()),
            domain: fnet_filter_ext::Domain::Ipv6,
        }
    }

    fn mangle_prerouting_routine(
        namespace: &fnet_filter_ext::Namespace,
    ) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_PREROUTING.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                fnet_filter_ext::InstalledIpRoutine {
                    hook: fnet_filter_ext::IpHook::Ingress,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_MANGLE,
                },
            )),
        }
    }

    fn mangle_input_routine(namespace: &fnet_filter_ext::Namespace) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_INPUT.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                fnet_filter_ext::InstalledIpRoutine {
                    hook: fnet_filter_ext::IpHook::LocalIngress,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_MANGLE,
                },
            )),
        }
    }

    fn mangle_forward_routine(namespace: &fnet_filter_ext::Namespace) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_FORWARD.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                fnet_filter_ext::InstalledIpRoutine {
                    hook: fnet_filter_ext::IpHook::Forwarding,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_MANGLE,
                },
            )),
        }
    }

    fn mangle_output_routine(namespace: &fnet_filter_ext::Namespace) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_OUTPUT.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                fnet_filter_ext::InstalledIpRoutine {
                    hook: fnet_filter_ext::IpHook::LocalEgress,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_MANGLE,
                },
            )),
        }
    }

    fn mangle_postrouting_routine(
        namespace: &fnet_filter_ext::Namespace,
    ) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_POSTROUTING.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Ip(Some(
                fnet_filter_ext::InstalledIpRoutine {
                    hook: fnet_filter_ext::IpHook::Egress,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_MANGLE,
                },
            )),
        }
    }

    fn nat_namespace_v4() -> fnet_filter_ext::Namespace {
        fnet_filter_ext::Namespace {
            id: fnet_filter_ext::NamespaceId("ipv4-nat".to_owned()),
            domain: fnet_filter_ext::Domain::Ipv4,
        }
    }

    fn nat_namespace_v6() -> fnet_filter_ext::Namespace {
        fnet_filter_ext::Namespace {
            id: fnet_filter_ext::NamespaceId("ipv6-nat".to_owned()),
            domain: fnet_filter_ext::Domain::Ipv6,
        }
    }

    fn nat_prerouting_routine(namespace: &fnet_filter_ext::Namespace) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_PREROUTING.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Nat(Some(
                fnet_filter_ext::InstalledNatRoutine {
                    hook: fnet_filter_ext::NatHook::Ingress,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_NAT_DST,
                },
            )),
        }
    }

    fn nat_input_routine(namespace: &fnet_filter_ext::Namespace) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_INPUT.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Nat(Some(
                fnet_filter_ext::InstalledNatRoutine {
                    hook: fnet_filter_ext::NatHook::LocalIngress,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_NAT_SRC,
                },
            )),
        }
    }

    fn nat_output_routine(namespace: &fnet_filter_ext::Namespace) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_OUTPUT.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Nat(Some(
                fnet_filter_ext::InstalledNatRoutine {
                    hook: fnet_filter_ext::NatHook::LocalEgress,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_NAT_DST,
                },
            )),
        }
    }

    fn nat_postrouting_routine(namespace: &fnet_filter_ext::Namespace) -> fnet_filter_ext::Routine {
        fnet_filter_ext::Routine {
            id: fnet_filter_ext::RoutineId {
                namespace: namespace.id.clone(),
                name: CHAIN_POSTROUTING.to_owned(),
            },
            routine_type: fnet_filter_ext::RoutineType::Nat(Some(
                fnet_filter_ext::InstalledNatRoutine {
                    hook: fnet_filter_ext::NatHook::Egress,
                    priority: nf_ip_hook_priorities_NF_IP_PRI_NAT_SRC,
                },
            )),
        }
    }

    fn ipv4_subnet_addr() -> in_addr {
        let bytes = "192.0.2.0".parse::<std::net::Ipv4Addr>().unwrap().octets();
        in_addr { s_addr: u32::from_be_bytes(bytes).to_be() }
    }

    fn ipv4_subnet_mask() -> in_addr {
        let bytes = "255.255.255.0".parse::<std::net::Ipv4Addr>().unwrap().octets();
        in_addr { s_addr: u32::from_be_bytes(bytes).to_be() }
    }

    #[fuchsia::test]
    fn ipv4_subnet_test() {
        assert_eq!(ipv4_addr_to_ip_address(ipv4_subnet_addr()), IPV4_ADDR);
        assert_eq!(ipv4_to_subnet(ipv4_subnet_addr(), ipv4_subnet_mask()), Ok(Some(IPV4_SUBNET)));
    }

    fn ipv6_subnet_addr() -> in6_addr {
        let bytes = "2001:db8::".parse::<std::net::Ipv6Addr>().unwrap().octets();
        in6_addr { in6_u: in6_addr__bindgen_ty_1 { u6_addr8: bytes } }
    }

    fn ipv6_subnet_mask() -> in6_addr {
        let bytes = "ffff:ffff::".parse::<std::net::Ipv6Addr>().unwrap().octets();
        in6_addr { in6_u: in6_addr__bindgen_ty_1 { u6_addr8: bytes } }
    }

    #[fuchsia::test]
    fn ipv6_subnet_test() {
        assert_eq!(ipv6_addr_to_ip_address(ipv6_subnet_addr()), IPV6_ADDR);
        assert_eq!(ipv6_to_subnet(ipv6_subnet_addr(), ipv6_subnet_mask()), Ok(Some(IPV6_SUBNET)));
    }

    fn ipv4_table_with_ip_matchers() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Start of INPUT built-in chain.
        let input_hook_entry = entries_bytes.len() as u32;

        // Entry 0: drop TCP packets other than from `IPV4_SUBNET`.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip {
                    src: ipv4_subnet_addr(),
                    smsk: ipv4_subnet_mask(),
                    invflags: IPT_INV_SRCIP as u8,
                    proto: IPPROTO_TCP as u16,
                    ..Default::default()
                },
                target_offset: 112,
                next_offset: 152,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_DROP);

        // Entry 1: accept UDP packets from `IPV4_SUBNET`.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip {
                    src: ipv4_subnet_addr(),
                    smsk: ipv4_subnet_mask(),
                    proto: IPPROTO_UDP as u16,
                    ..Default::default()
                },
                target_offset: 112,
                next_offset: 152,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 2: drop all packets going to en0 interface.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip {
                    iniface: string_to_ascii_buffer("en0").unwrap(),
                    iniface_mask: [255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    ..Default::default()
                },
                target_offset: 112,
                next_offset: 152,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_DROP);

        // Entry 3: drop all ICMP packets.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip { proto: IPPROTO_ICMP as u16, ..Default::default() },
                target_offset: 112,
                next_offset: 152,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_DROP);

        // Entry 4: drop all ICMPV6 packets.
        // Note: this rule doesn't make sense on a IPv4 table, but iptables will defer to Netstack
        // to report this error.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip { proto: IPPROTO_ICMPV6 as u16, ..Default::default() },
                target_offset: 112,
                next_offset: 152,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_DROP);

        // Entry 5: policy of INPUT chain.
        let input_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of FORWARD built-in chain.
        let forward_hook_entry = entries_bytes.len() as u32;

        // Entry 6: policy of FORWARD chain.
        // Note: FORWARD chain has no other rules.
        let forward_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of OUTPUT built-in chain.
        let output_hook_entry = entries_bytes.len() as u32;

        // Entry 7: accept all packets going from wifi1 interface.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip {
                    outiface: string_to_ascii_buffer("wifi1").unwrap(),
                    outiface_mask: [255, 255, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    ..Default::default()
                },
                target_offset: 112,
                next_offset: 152,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 8: policy of OUTPUT chain.
        let output_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_DROP);

        // Entry 9: end of input.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ipt_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 10,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::FILTER.bits(),
            hook_entry: [0, input_hook_entry, forward_hook_entry, output_hook_entry, 0],
            underflow: [0, input_underflow, forward_underflow, output_underflow, 0],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.extend(entries_bytes);
        bytes
    }

    fn ipv6_table_with_ip_matchers() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Start of INPUT built-in chain.
        let input_hook_entry = entries_bytes.len() as u32;

        // Entry 0: drop TCP packets other than from `IPV6_SUBNET`.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    src: ipv6_subnet_addr(),
                    smsk: ipv6_subnet_mask(),
                    invflags: IPT_INV_SRCIP as u8,
                    proto: IPPROTO_TCP as u16,
                    flags: IP6T_F_PROTO as u8,
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 208,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_DROP);

        // Entry 1: accept UDP packets from `IPV6_SUBNET`.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    src: ipv6_subnet_addr(),
                    smsk: ipv6_subnet_mask(),
                    proto: IPPROTO_UDP as u16,
                    flags: IP6T_F_PROTO as u8,
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 208,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 2: drop all packets going to en0 interface.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    iniface: string_to_ascii_buffer("en0").unwrap(),
                    iniface_mask: [255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 208,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_DROP);

        // Entry 3: drop all ICMP packets.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    proto: IPPROTO_ICMP as u16,
                    flags: IP6T_F_PROTO as u8,
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 208,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_DROP);

        // Entry 4: drop all ICMPV6 packets.
        // Note: this rule doesn't make sense on a IPv4 table, but iptables will defer to Netstack
        // to report this error.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    proto: IPPROTO_ICMPV6 as u16,
                    flags: IP6T_F_PROTO as u8,
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 208,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_DROP);

        // Entry 5: policy of INPUT chain.
        let input_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of FORWARD built-in chain.
        let forward_hook_entry = entries_bytes.len() as u32;

        // Entry 6: policy of FORWARD chain.
        // Note: FORWARD chain has no other rules.
        let forward_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of OUTPUT built-in chain.
        let output_hook_entry = entries_bytes.len() as u32;

        // Entry 7: accept all packets going out from wifi1 interface.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    outiface: string_to_ascii_buffer("wifi1").unwrap(),
                    outiface_mask: [255, 255, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 208,
                ..Default::default()
            }
            .as_bytes(),
        );
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 8: policy of OUTPUT chain.
        let output_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_DROP);

        // Entry 9: end of input.
        extend_with_error_target_ipv6_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ip6t_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 10,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::FILTER.bits(),
            hook_entry: [0, input_hook_entry, forward_hook_entry, output_hook_entry, 0],
            underflow: [0, input_underflow, forward_underflow, output_underflow, 0],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.append(&mut entries_bytes);
        bytes
    }

    #[test_case(
        ipv4_table_with_ip_matchers(),
        IpTable::from_ipt_replace,
        filter_namespace_v4(),
        IPV4_SUBNET;
        "ipv4"
    )]
    #[test_case(
        ipv6_table_with_ip_matchers(),
        IpTable::from_ip6t_replace,
        filter_namespace_v6(),
        IPV6_SUBNET;
        "ipv6"
    )]
    fn parse_ip_matchers_test(
        table_bytes: Vec<u8>,
        parse_fn: fn(&mut TestIpTablesContext, Vec<u8>) -> Result<IpTable, IpTableParseError>,
        expected_namespace: fnet_filter_ext::Namespace,
        expected_subnet: fnet::Subnet,
    ) {
        let mut context = TestIpTablesContext::new();
        let table = parse_fn(&mut context, table_bytes).unwrap();
        assert_eq!(table.namespace, expected_namespace);

        assert_eq!(
            table.routines,
            [
                filter_input_routine(&expected_namespace),
                filter_forward_routine(&expected_namespace),
                filter_output_routine(&expected_namespace),
            ]
        );

        let expected_rules = [
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: filter_input_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    src_addr: Some(fnet_matchers_ext::Address {
                        matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                            fnet_matchers_ext::Subnet::try_from(expected_subnet).unwrap(),
                        ),
                        invert: true,
                    }),
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Drop,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 1,
                    routine: filter_input_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    src_addr: Some(fnet_matchers_ext::Address {
                        matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                            fnet_matchers_ext::Subnet::try_from(expected_subnet).unwrap(),
                        ),
                        invert: false,
                    }),
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 2,
                    routine: filter_input_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    in_interface: Some(fnet_matchers_ext::Interface::Name("en0".to_string())),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Drop,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 3,
                    routine: filter_input_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Icmp),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Drop,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 4,
                    routine: filter_input_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Icmpv6),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Drop,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 5,
                    routine: filter_input_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: filter_forward_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: filter_output_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    out_interface: Some(fnet_matchers_ext::Interface::Name("wifi1".to_string())),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 1,
                    routine: filter_output_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Drop,
            },
        ];

        for (rule, expected) in table.rules.iter().zip_eq(expected_rules.into_iter()) {
            assert_eq!(rule, &expected);
        }
    }

    const TEST_EBPF_PROGRAM_PATH: &str = "/sys/fs/bpf/test_prog";
    const TEST_EBPF_PROGRAM_ID: febpf::ProgramId = febpf::ProgramId { id: 42 };

    fn table_with_match_extensions() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Start of INPUT built-in chain.
        let input_hook_entry = entries_bytes.len() as u32;

        // Entry 0: DROP all TCP packets except those destined to port 8000. Offset 0
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip { proto: IPPROTO_TCP as u16, ..Default::default() },
                target_offset: 160,
                next_offset: 200,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_match {
                match_size: 48,
                name: string_to_ascii_buffer("tcp").unwrap(),
                revision: 0,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_tcp {
                spts: [0, 65535],
                dpts: [8000, 8000],
                invflags: XT_TCP_INV_DSTPT as u8,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(&[0, 0, 0, 0]); // padding
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_DROP);

        // Entry 1: ACCEPT UDP packets with source port between 2000-3000.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip { proto: IPPROTO_UDP as u16, ..Default::default() },
                target_offset: 160,
                next_offset: 200,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_match {
                match_size: 48,
                name: string_to_ascii_buffer("udp").unwrap(),
                revision: 0,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_udp { spts: [2000, 3000], dpts: [0, 65535], ..Default::default() }.as_bytes(),
        );
        entries_bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 2: Drop packets that match custom BPF matcher.
        const BPF_MATCHER_SIZE: usize = size_of::<xt_entry_match>() + size_of::<xt_bpf_info_v1>();
        const BPF_TARGET_OFFSET: usize = BPF_MATCHER_SIZE + size_of::<ipt_entry>();
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: Default::default(),
                target_offset: BPF_TARGET_OFFSET as u16,
                next_offset: (BPF_TARGET_OFFSET + STANDARD_TARGET_SIZE) as u16,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_match {
                match_size: BPF_MATCHER_SIZE as u16,
                name: string_to_ascii_buffer("bpf").unwrap(),
                revision: 1,
            }
            .as_bytes(),
        );
        let bpf_info = xt_bpf_info_v1 {
            mode: starnix_uapi::xt_bpf_modes_XT_BPF_MODE_FD_PINNED as u16,
            bpf_program_num_elem: 0,
            fd: 0,
            __bindgen_anon_1: starnix_uapi::xt_bpf_info_v1__bindgen_ty_1 {
                path: string_to_ascii_buffer(TEST_EBPF_PROGRAM_PATH).unwrap(),
            },
            filter: starnix_uapi::uaddr::default().into(),
        };
        entries_bytes.extend_from_slice(bpf_info.as_bytes());
        extend_with_standard_verdict(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 3: policy of INPUT chain.
        let input_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of FORWARD built-in chain.
        let forward_hook_entry = entries_bytes.len() as u32;

        // Entry 4: policy of FORWARD chain.
        // Note: FORWARD chain has no other rules.
        let forward_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of OUTPUT built-in chain.
        let output_hook_entry = entries_bytes.len() as u32;

        // Entry 5: policy of OUTPUT chain.
        // Note: OUTPUT chain has no other rules.
        let output_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_DROP);

        // Entry 6: end of input.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ipt_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 7,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::FILTER.bits(),
            hook_entry: [0, input_hook_entry, forward_hook_entry, output_hook_entry, 0],
            underflow: [0, input_underflow, forward_underflow, output_underflow, 0],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();
        bytes.extend(entries_bytes);
        bytes
    }

    #[fuchsia::test]
    fn parse_match_extensions_test() {
        let mut context = TestIpTablesContext::new();
        let table = IpTable::from_ipt_replace(&mut context, table_with_match_extensions()).unwrap();

        let expected_namespace = filter_namespace_v4();
        assert_eq!(table.namespace, expected_namespace);

        assert_eq!(
            table.routines,
            [
                filter_input_routine(&expected_namespace),
                filter_forward_routine(&expected_namespace),
                filter_output_routine(&expected_namespace),
            ]
        );

        let mut rules = table.rules.into_iter();

        assert_eq!(
            rules.next().unwrap(),
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: filter_input_routine(&expected_namespace).id,
                },
                matchers: fnet_filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                        src_port: Some(fnet_matchers_ext::Port::new(0, 65535, false).unwrap()),
                        dst_port: Some(fnet_matchers_ext::Port::new(8000, 8000, true).unwrap()),
                    }),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Drop,
            }
        );
        assert_eq!(
            rules.next().unwrap(),
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 1,
                    routine: filter_input_routine(&expected_namespace).id,
                },
                matchers: fnet_filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                        src_port: Some(fnet_matchers_ext::Port::new(2000, 3000, false).unwrap()),
                        dst_port: Some(fnet_matchers_ext::Port::new(0, 65535, false).unwrap()),
                    }),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Accept,
            }
        );
        assert_eq!(
            rules.next().unwrap(),
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 2,
                    routine: filter_input_routine(&expected_namespace).id,
                },
                matchers: fnet_filter_ext::Matchers {
                    ebpf_program: Some(TEST_EBPF_PROGRAM_ID),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Accept,
            }
        );

        assert_eq!(
            rules.next().unwrap(),
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 3,
                    routine: filter_input_routine(&expected_namespace).id,
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            }
        );

        assert_eq!(
            rules.next().unwrap(),
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: filter_forward_routine(&expected_namespace).id,
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            }
        );

        assert_eq!(
            rules.next().unwrap(),
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: filter_output_routine(&expected_namespace).id,
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Drop,
            }
        );

        assert!(rules.next().is_none());
    }

    fn ipv4_table_with_jump_target() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Start of INPUT built-in chain.
        let input_hook_entry = entries_bytes.len() as u32;

        // Entry 0 (byte 0): JUMP to chain1 for all packets.
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, 784);

        // Entry 1 (byte 152): policy of INPUT chain.
        let input_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of FORWARD built-in chain.
        let forward_hook_entry = entries_bytes.len() as u32;

        // Entry 2 (byte 304): policy of FORWARD chain.
        // Note: FORWARD chain has no other rules.
        let forward_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of OUTPUT built-in chain.
        let output_hook_entry = entries_bytes.len() as u32;

        // Entry 3 (byte 456): policy of OUTPUT chain.
        // Note: OUTPUT chain has no other rules.
        let output_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_DROP);

        // Entry 4 (byte 608): start of the chain1.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, "chain1");

        // Entry 5 (byte 784): jump to chain2 for all packets.
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, 1264);

        // Entry 6 (byte 936): policy of chain1.
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_RETURN);

        // Entry 7 (byte 1088): start of chain2.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, "chain2");

        // Entry 8 (byte 1264): policy of chain2.
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_RETURN);

        // Entry 9 (byte 1416): end of input.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ipt_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 10,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::FILTER.bits(),
            hook_entry: [0, input_hook_entry, forward_hook_entry, output_hook_entry, 0],
            underflow: [0, input_underflow, forward_underflow, output_underflow, 0],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.extend(entries_bytes);
        bytes
    }

    fn ipv6_table_with_jump_target() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Start of INPUT built-in chain.
        let input_hook_entry = entries_bytes.len() as u32;

        // Entry 0 (byte 0): JUMP to chain1 for all packets.
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, 1064);

        // Entry 1 (byte 208): policy of INPUT chain.
        let input_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of FORWARD built-in chain.
        let forward_hook_entry = entries_bytes.len() as u32;

        // Entry 2 (byte 416): policy of FORWARD chain.
        // Note: FORWARD chain has no other rules.
        let forward_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of OUTPUT built-in chain.
        let output_hook_entry = entries_bytes.len() as u32;

        // Entry 3 (byte 624): policy of OUTPUT chain.
        // Note: OUTPUT chain has no other rules.
        let output_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_DROP);

        // Entry 4 (byte 832): start of the chain1.
        extend_with_error_target_ipv6_entry(&mut entries_bytes, "chain1");

        // Entry 5 (byte 1064): jump to chain2 for all packets.
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, 1712);

        // Entry 6 (byte 1272): policy of chain1.
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_RETURN);

        // Entry 7 (byte 1480): start of chain2.
        extend_with_error_target_ipv6_entry(&mut entries_bytes, "chain2");

        // Entry 8 (byte 1712): policy of chain2.
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_RETURN);

        // Entry 9 (byte 1920): end of input.
        extend_with_error_target_ipv6_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ip6t_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 10,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::FILTER.bits(),
            hook_entry: [0, input_hook_entry, forward_hook_entry, output_hook_entry, 0],
            underflow: [0, input_underflow, forward_underflow, output_underflow, 0],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.extend(entries_bytes);
        bytes
    }

    #[test_case(
        ipv4_table_with_jump_target(),
        IpTable::from_ipt_replace,
        filter_namespace_v4();
        "ipv4"
    )]
    #[test_case(
        ipv6_table_with_jump_target(),
        IpTable::from_ip6t_replace,
        filter_namespace_v6();
        "ipv6"
    )]
    fn parse_jump_target_test(
        table_bytes: Vec<u8>,
        parse_fn: fn(&mut TestIpTablesContext, Vec<u8>) -> Result<IpTable, IpTableParseError>,
        expected_namespace: fnet_filter_ext::Namespace,
    ) {
        let mut context = TestIpTablesContext::new();
        let table = parse_fn(&mut context, table_bytes).unwrap();
        assert_eq!(table.namespace, expected_namespace);

        assert_eq!(
            table.routines,
            [
                filter_input_routine(&expected_namespace),
                filter_forward_routine(&expected_namespace),
                filter_output_routine(&expected_namespace),
                filter_custom_routine(&expected_namespace, "chain1"),
                filter_custom_routine(&expected_namespace, "chain2")
            ]
        );

        let expected_rules = [
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: filter_input_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Jump("chain1".to_string()),
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 1,
                    routine: filter_input_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: filter_forward_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: filter_output_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Drop,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: filter_custom_routine(&expected_namespace, "chain1").id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Jump("chain2".to_string()),
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 1,
                    routine: filter_custom_routine(&expected_namespace, "chain1").id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Return,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: filter_custom_routine(&expected_namespace, "chain2").id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Return,
            },
        ];

        for (rule, expected) in table.rules.iter().zip_eq(expected_rules.into_iter()) {
            assert_eq!(rule, &expected);
        }
    }

    // Same layout as `nf_nat_ipv4_multi_range_compat`.
    #[repr(C)]
    #[derive(IntoBytes, Debug, Default, KnownLayout, FromBytes, Immutable)]
    struct RedirectTargetV4 {
        pub rangesize: u32,
        pub flags: u32,
        pub _min_ip: u32,
        pub _max_ip: u32,
        pub min: u16,
        pub max: u16,
    }

    // Same layout as `xt_tproxy_target_info_v1` with an IPv4 address.
    #[repr(C)]
    #[derive(IntoBytes, Default, KnownLayout, FromBytes, Immutable)]
    struct TproxyTargetV4 {
        pub _mark_mask: u32,
        pub _mark_value: u32,
        pub laddr: in_addr,
        pub _addr_padding: [u32; 3],
        pub lport: u16,
        pub _padding: [u8; 2],
    }

    fn ipv4_table_with_redirect_and_tproxy() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Start of PREROUTING built-in chain.
        let prerouting_hook_entry = entries_bytes.len() as u32;

        // Entry 0: TPROXY TCP traffic to addr.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip { proto: IPPROTO_TCP as u16, ..Default::default() },
                target_offset: 112,
                next_offset: 172,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 60,
                name: string_to_ascii_buffer(TARGET_TPROXY).unwrap(),
                revision: 1,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            TproxyTargetV4 { laddr: ipv4_subnet_addr(), ..Default::default() }.as_bytes(),
        );

        // Entry 1: TPROXY UDP traffic to port.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip { proto: IPPROTO_UDP as u16, ..Default::default() },
                target_offset: 112,
                next_offset: 172,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 60,
                name: string_to_ascii_buffer(TARGET_TPROXY).unwrap(),
                revision: 1,
            }
            .as_bytes(),
        );
        entries_bytes
            .extend_from_slice(TproxyTargetV4 { lport: PORT, ..Default::default() }.as_bytes());

        // Entry 2: TPROXY TCP traffic to addr and port.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip { proto: IPPROTO_TCP as u16, ..Default::default() },
                target_offset: 112,
                next_offset: 172,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 60,
                name: string_to_ascii_buffer(TARGET_TPROXY).unwrap(),
                revision: 1,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            TproxyTargetV4 { laddr: ipv4_subnet_addr(), lport: PORT, ..Default::default() }
                .as_bytes(),
        );

        // Entry 3: policy of PREROUTING chain.
        let prerouting_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of INPUT built-in chain.
        let input_hook_entry = entries_bytes.len() as u32;

        // Entry 4: policy of INPUT chain.
        // Note: INPUT chain has no other rules.
        let input_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of OUTPUT built-in chain.
        let output_hook_entry = entries_bytes.len() as u32;

        // Entry 5: REDIRECT TCP traffic without port change.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip { proto: IPPROTO_TCP as u16, ..Default::default() },
                target_offset: 112,
                next_offset: 164,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 52,
                name: string_to_ascii_buffer(TARGET_REDIRECT).unwrap(),
                revision: 0,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            RedirectTargetV4 { rangesize: 1, flags: 0, ..Default::default() }.as_bytes(),
        );

        // Entry 6: REDIRECT UDP traffic to a single port.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip { proto: IPPROTO_UDP as u16, ..Default::default() },
                target_offset: 112,
                next_offset: 164,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 52,
                name: string_to_ascii_buffer(TARGET_REDIRECT).unwrap(),
                revision: 0,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            RedirectTargetV4 {
                rangesize: 1,
                flags: NfNatRangeFlags::PROTO_SPECIFIED.bits(),
                min: PORT,
                max: PORT,
                ..Default::default()
            }
            .as_bytes(),
        );

        // Entry 7: REDIRECT TCP traffic to a port range.
        entries_bytes.extend_from_slice(
            ipt_entry {
                ip: ipt_ip { proto: IPPROTO_TCP as u16, ..Default::default() },
                target_offset: 112,
                next_offset: 164,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 52,
                name: string_to_ascii_buffer(TARGET_REDIRECT).unwrap(),
                revision: 0,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            RedirectTargetV4 {
                rangesize: 1,
                flags: NfNatRangeFlags::PROTO_SPECIFIED.bits(),
                min: PORT_RANGE_START,
                max: PORT_RANGE_END,
                ..Default::default()
            }
            .as_bytes(),
        );

        // Entry 8: policy of OUTPUT chain.
        // Note: OUTPUT chain has no other rules.
        let output_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of POSTROUTING built-in chain.
        let postrouting_hook_entry = entries_bytes.len() as u32;

        // Entry 9: policy of POSTROUTING chain.
        // Note: POSTROUTING chain has no other rules.
        let postrouting_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_DROP);

        // Entry 10: end of input.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ipt_replace {
            name: string_to_ascii_buffer("nat").unwrap(),
            num_entries: 11,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::NAT.bits(),
            hook_entry: [
                prerouting_hook_entry,
                input_hook_entry,
                0,
                output_hook_entry,
                postrouting_hook_entry,
            ],
            underflow: [
                prerouting_underflow,
                input_underflow,
                0,
                output_underflow,
                postrouting_underflow,
            ],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.extend(entries_bytes);
        bytes
    }

    // Same layout as `nf_nat_range`.
    #[repr(C)]
    #[derive(IntoBytes, Default, KnownLayout, FromBytes, Immutable)]
    struct RedirectTargetV6 {
        pub flags: u32,
        pub _min_addr: [u32; 4],
        pub _max_addr: [u32; 4],
        pub min_proto: u16,
        pub max_proto: u16,
    }

    // Same layout as `xt_tproxy_target_info_v1` with an IPv6 address.
    #[repr(C)]
    #[derive(IntoBytes, Default, KnownLayout, FromBytes, Immutable)]
    struct TproxyTargetV6 {
        pub _mark_mask: u32,
        pub _mark_value: u32,
        pub laddr: in6_addr,
        pub lport: u16,
        pub _padding: [u8; 2],
    }

    fn ipv6_table_with_redirect_and_tproxy() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Start of PREROUTING built-in chain.
        let prerouting_hook_entry = entries_bytes.len() as u32;

        // Entry 0: TPROXY TCP traffic to addr.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    proto: IPPROTO_TCP as u16,
                    flags: IP6T_F_PROTO as u8,
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 228,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 60,
                name: string_to_ascii_buffer(TARGET_TPROXY).unwrap(),
                revision: 1,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            TproxyTargetV6 { laddr: ipv6_subnet_addr(), ..Default::default() }.as_bytes(),
        );

        // Entry 1: TPROXY UDP traffic to port.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    proto: IPPROTO_UDP as u16,
                    flags: IP6T_F_PROTO as u8,
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 228,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 60,
                name: string_to_ascii_buffer(TARGET_TPROXY).unwrap(),
                revision: 1,
            }
            .as_bytes(),
        );
        entries_bytes
            .extend_from_slice(TproxyTargetV6 { lport: PORT, ..Default::default() }.as_bytes());

        // Entry 2: TPROXY TCP traffic to addr and port.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    proto: IPPROTO_TCP as u16,
                    flags: IP6T_F_PROTO as u8,
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 228,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 60,
                name: string_to_ascii_buffer(TARGET_TPROXY).unwrap(),
                revision: 1,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            TproxyTargetV6 { laddr: ipv6_subnet_addr(), lport: PORT, ..Default::default() }
                .as_bytes(),
        );

        // Entry 3: policy of PREROUTING chain.
        let prerouting_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of INPUT built-in chain.
        let input_hook_entry = entries_bytes.len() as u32;

        // Entry 4: policy of INPUT chain.
        // Note: INPUT chain has no other rules.
        let input_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of OUTPUT built-in chain.
        let output_hook_entry = entries_bytes.len() as u32;

        // Entry 5: REDIRECT TCP traffic without port change.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    proto: IPPROTO_TCP as u16,
                    flags: IP6T_F_PROTO as u8,
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 240,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 72,
                name: string_to_ascii_buffer(TARGET_REDIRECT).unwrap(),
                revision: 0,
            }
            .as_bytes(),
        );
        entries_bytes
            .extend_from_slice(RedirectTargetV6 { flags: 0, ..Default::default() }.as_bytes());

        // Entry 6: REDIRECT UDP traffic to a single port.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    proto: IPPROTO_UDP as u16,
                    flags: IP6T_F_PROTO as u8,
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 240,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 72,
                name: string_to_ascii_buffer(TARGET_REDIRECT).unwrap(),
                revision: 0,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            RedirectTargetV6 {
                flags: NfNatRangeFlags::PROTO_SPECIFIED.bits(),
                min_proto: PORT,
                max_proto: PORT,
                ..Default::default()
            }
            .as_bytes(),
        );

        // Entry 7: REDIRECT TCP traffic to a port range.
        entries_bytes.extend_from_slice(
            ip6t_entry {
                ipv6: ip6t_ip6 {
                    proto: IPPROTO_TCP as u16,
                    flags: IP6T_F_PROTO as u8,
                    ..Default::default()
                },
                target_offset: 168,
                next_offset: 240,
                ..Default::default()
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: 72,
                name: string_to_ascii_buffer(TARGET_REDIRECT).unwrap(),
                revision: 0,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(
            RedirectTargetV6 {
                flags: NfNatRangeFlags::PROTO_SPECIFIED.bits(),
                min_proto: PORT_RANGE_START,
                max_proto: PORT_RANGE_END,
                ..Default::default()
            }
            .as_bytes(),
        );

        // Entry 8: policy of OUTPUT chain.
        let output_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of POSTROUTING built-in chain.
        let postrouting_hook_entry = entries_bytes.len() as u32;

        // Entry 9: policy of POSTROUTING chain.
        // Note: POSTROUTING chain has no other rules.
        let postrouting_underflow = entries_bytes.len() as u32;
        extend_with_standard_target_ipv6_entry(&mut entries_bytes, VERDICT_DROP);

        // Entry 10: end of input.
        extend_with_error_target_ipv6_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ip6t_replace {
            name: string_to_ascii_buffer("nat").unwrap(),
            num_entries: 11,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::NAT.bits(),
            hook_entry: [
                prerouting_hook_entry,
                input_hook_entry,
                0,
                output_hook_entry,
                postrouting_hook_entry,
            ],
            underflow: [
                prerouting_underflow,
                input_underflow,
                0,
                output_underflow,
                postrouting_underflow,
            ],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.extend(entries_bytes);
        bytes
    }

    struct TestIpTablesContext {
        registered_ebpf_program: bool,
    }

    impl TestIpTablesContext {
        fn new() -> Self {
            Self { registered_ebpf_program: false }
        }
    }

    impl IptReplaceContext for TestIpTablesContext {
        fn resolve_ebpf_socket_filter(
            &mut self,
            path: &BString,
        ) -> Result<febpf::ProgramId, IpTableParseError> {
            if path == TEST_EBPF_PROGRAM_PATH {
                self.registered_ebpf_program = true;
                return Ok(TEST_EBPF_PROGRAM_ID);
            }
            Err(IpTableParseError::InvalidEbpfProgramPath { path: path.clone() })
        }
    }

    #[test_case(
        ipv4_table_with_redirect_and_tproxy(),
        IpTable::from_ipt_replace,
        nat_namespace_v4(),
        IPV4_ADDR;
        "ipv4"
    )]
    #[test_case(
        ipv6_table_with_redirect_and_tproxy(),
        IpTable::from_ip6t_replace,
        nat_namespace_v6(),
        IPV6_ADDR;
        "ipv6"
    )]
    fn parse_redirect_tproxy_test(
        table_bytes: Vec<u8>,
        parse_table: fn(&mut TestIpTablesContext, Vec<u8>) -> Result<IpTable, IpTableParseError>,
        expected_namespace: fnet_filter_ext::Namespace,
        expected_ip_addr: fnet::IpAddress,
    ) {
        let mut context = TestIpTablesContext::new();
        let table = parse_table(&mut context, table_bytes).unwrap();
        assert_eq!(table.namespace, expected_namespace);

        assert_eq!(
            table.routines,
            [
                nat_prerouting_routine(&expected_namespace),
                nat_input_routine(&expected_namespace),
                nat_output_routine(&expected_namespace),
                nat_postrouting_routine(&expected_namespace),
            ]
        );

        let expected_rules = [
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: nat_prerouting_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::TransparentProxy(
                    fnet_filter_ext::TransparentProxy::LocalAddr(expected_ip_addr),
                ),
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 1,
                    routine: nat_prerouting_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::TransparentProxy(
                    fnet_filter_ext::TransparentProxy::LocalPort(NONZERO_PORT),
                ),
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 2,
                    routine: nat_prerouting_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::TransparentProxy(
                    fnet_filter_ext::TransparentProxy::LocalAddrAndPort(
                        expected_ip_addr,
                        NONZERO_PORT,
                    ),
                ),
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 3,
                    routine: nat_prerouting_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: nat_input_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: nat_output_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Redirect { dst_port: None },
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 1,
                    routine: nat_output_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Redirect {
                    dst_port: Some(fnet_filter_ext::PortRange(NONZERO_PORT..=NONZERO_PORT)),
                },
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 2,
                    routine: nat_output_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: fnet_filter_ext::Action::Redirect {
                    dst_port: Some(fnet_filter_ext::PortRange(NONZERO_PORT_RANGE)),
                },
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 3,
                    routine: nat_output_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: nat_postrouting_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Drop,
            },
        ];

        for (rule, expected) in table.rules.iter().zip_eq(expected_rules.into_iter()) {
            assert_eq!(rule, &expected);
        }
    }

    fn ip_table_with_mark(
        xt_entry: &[u8],
        extend_with_standard_target: fn(&mut Vec<u8>, i32),
        extend_with_error_target: fn(&mut Vec<u8>, &str),
    ) -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Start of PREROUTING built-in chain.
        let prerouting_hook_entry = entries_bytes.len() as u32;

        // Entry 1: policy of PREROUTING chain.
        // Note: PREROUTING chain has no other rules.
        let prerouting_underflow = entries_bytes.len() as u32;
        extend_with_standard_target(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of INPUT built-in chain.
        let input_hook_entry = entries_bytes.len() as u32;

        // Entry 2: The mark target.
        entries_bytes.extend_from_slice(xt_entry);
        entries_bytes.extend_from_slice(
            xt_entry_target {
                target_size: u16::try_from(
                    size_of::<xt_entry_target>() + size_of::<xt_mark_tginfo2>(),
                )
                .unwrap(),
                name: string_to_ascii_buffer(TARGET_MARK).unwrap(),
                revision: 2,
            }
            .as_bytes(),
        );
        entries_bytes.extend_from_slice(xt_mark_tginfo2 { mark: MARK, mask: MASK }.as_bytes());

        // Entry 3: policy of INPUT chain.
        let input_underflow = entries_bytes.len() as u32;
        extend_with_standard_target(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of FORWARD built-in chain.
        let forward_hook_entry = entries_bytes.len() as u32;

        // Entry 4: policy of FORWARD chain.
        // Note: FORWARD chain has no other rules.
        let forward_underflow = entries_bytes.len() as u32;
        extend_with_standard_target(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of OUTPUT built-in chain.
        let output_hook_entry = entries_bytes.len() as u32;

        // Entry 5: policy of OUTPUT chain.
        // Note: OUTPUT chain has no other rules.
        let output_underflow = entries_bytes.len() as u32;
        extend_with_standard_target(&mut entries_bytes, VERDICT_ACCEPT);

        // Start of POSTROUTING built-in chain.
        let postrouting_hook_entry = entries_bytes.len() as u32;

        // Entry 6: policy of POSTROUTING chain.
        // Note: POSTROUTING chain has no other rules.
        let postrouting_underflow = entries_bytes.len() as u32;
        extend_with_standard_target(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 7: end of input.
        extend_with_error_target(&mut entries_bytes, TARGET_ERROR);

        assert_eq!(IPT_REPLACE_SIZE, IP6T_REPLACE_SIZE);
        let mut bytes = ipt_replace {
            name: string_to_ascii_buffer("mangle").unwrap(),
            num_entries: 7,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::MANGLE.bits(),
            hook_entry: [
                prerouting_hook_entry,
                input_hook_entry,
                forward_hook_entry,
                output_hook_entry,
                postrouting_hook_entry,
            ],
            underflow: [
                prerouting_underflow,
                input_underflow,
                forward_underflow,
                output_underflow,
                postrouting_underflow,
            ],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.extend(entries_bytes);
        bytes
    }

    #[test_case(
        IpTable::from_ipt_replace,
        ipt_entry {
            target_offset: u16::try_from(size_of::<ipt_entry>()).unwrap(),
            next_offset: u16::try_from(
                size_of::<ipt_entry>()
                    + size_of::<xt_entry_target>()
                    + size_of::<xt_mark_tginfo2>(),
            )
            .unwrap(),
            ..Default::default()
        }
        .as_bytes(),
        extend_with_standard_target_ipv4_entry,
        extend_with_error_target_ipv4_entry,
        mangle_namespace_v4(); "ipv4"
    )]
    #[test_case(
        IpTable::from_ip6t_replace,
        ip6t_entry {
            target_offset: u16::try_from(size_of::<ip6t_entry>()).unwrap(),
            next_offset: u16::try_from(
                size_of::<ip6t_entry>()
                    + size_of::<xt_entry_target>()
                    + size_of::<xt_mark_tginfo2>(),
            )
            .unwrap(),
            ..Default::default()
        }
        .as_bytes(),
        extend_with_standard_target_ipv6_entry,
        extend_with_error_target_ipv6_entry,
        mangle_namespace_v6(); "ipv6"
    )]
    fn parse_mark_test(
        parse_fn: fn(&mut TestIpTablesContext, Vec<u8>) -> Result<IpTable, IpTableParseError>,
        xt_entry: &[u8],
        extend_with_standard_target: fn(&mut Vec<u8>, i32),
        extend_with_error_target: fn(&mut Vec<u8>, &str),
        expected_namespace: fnet_filter_ext::Namespace,
    ) {
        let mut context = TestIpTablesContext::new();
        let table = parse_fn(
            &mut context,
            ip_table_with_mark(xt_entry, extend_with_standard_target, extend_with_error_target),
        )
        .unwrap();

        assert_eq!(table.namespace, expected_namespace);

        assert_eq!(
            table.routines,
            [
                mangle_prerouting_routine(&expected_namespace),
                mangle_input_routine(&expected_namespace),
                mangle_forward_routine(&expected_namespace),
                mangle_output_routine(&expected_namespace),
                mangle_postrouting_routine(&expected_namespace),
            ]
        );

        let expected_rules = [
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: mangle_prerouting_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: mangle_input_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Mark {
                    domain: fnet::MarkDomain::Mark1,
                    action: fnet_filter_ext::MarkAction::SetMark {
                        clearing_mask: MASK,
                        mark: MARK,
                    },
                },
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 1,
                    routine: mangle_input_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: mangle_forward_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: mangle_output_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
            fnet_filter_ext::Rule {
                id: fnet_filter_ext::RuleId {
                    index: 0,
                    routine: mangle_postrouting_routine(&expected_namespace).id.clone(),
                },
                matchers: fnet_filter_ext::Matchers::default(),
                action: fnet_filter_ext::Action::Accept,
            },
        ];

        for (rule, expected) in table.rules.iter().zip_eq(expected_rules.into_iter()) {
            assert_eq!(rule, &expected);
        }
    }

    fn table_with_wrong_size() -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(
            ipt_replace {
                name: string_to_ascii_buffer("filter").unwrap(),
                num_entries: 1,
                size: 0,
                ..Default::default()
            }
            .as_bytes(),
        );

        extend_with_error_target_ipv4_entry(&mut bytes, TARGET_ERROR);
        bytes
    }

    fn table_with_wrong_num_entries() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        extend_with_error_target_ipv4_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ipt_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 3,
            size: entries_bytes.len() as u32,
            ..Default::default()
        }
        .as_bytes()
        .to_owned();
        bytes.extend(entries_bytes);
        bytes
    }

    fn table_with_no_entries() -> Vec<u8> {
        ipt_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 0,
            size: 0,
            ..Default::default()
        }
        .as_bytes()
        .to_owned()
    }

    fn table_with_invalid_hook_bits() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Entry 0: end of input.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ipt_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 1,
            size: entries_bytes.len() as u32,
            valid_hooks: 0b00011,
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.extend(entries_bytes);
        bytes
    }

    fn table_with_invalid_hook_entry() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Entry 0: end of input.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ipt_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 1,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::FILTER.bits(),

            // 8 does not refer to a valid entry.
            hook_entry: [0, 8, 0, 0, 0],
            underflow: [0, 8, 0, 0, 0],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.extend(entries_bytes);
        bytes
    }

    fn table_with_hook_ranges_overlap() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Entry 0: policy of INPUT, FORWARD, OUTPUT.
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 1: end of input.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, "ERROR");

        let mut bytes = ipt_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 2,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::FILTER.bits(),
            hook_entry: [0, 0, 0, 0, 0],
            underflow: [0, 0, 0, 0, 0],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.extend(entries_bytes);
        bytes
    }

    fn table_with_unexpected_error_target() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Entry 0: start of "mychain".
        // From hook_entry, this should contain rules of the INPUT chain.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, "mychain");

        // Entry 1: policy of FORWARD.
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 2: policy of OUTPUT.
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 3: end of input.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ipt_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 4,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::FILTER.bits(),
            hook_entry: [0, 0, 176, 328, 0],
            underflow: [0, 0, 176, 328, 0],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.extend(entries_bytes);
        bytes
    }

    fn table_with_chain_with_no_policy() -> Vec<u8> {
        let mut entries_bytes = Vec::new();

        // Entry 0: policy of INPUT.
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 1: policy of FORWARD.
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 2: policy of OUTPUT.
        extend_with_standard_target_ipv4_entry(&mut entries_bytes, VERDICT_ACCEPT);

        // Entry 3: start of "mychain".
        extend_with_error_target_ipv4_entry(&mut entries_bytes, "mychain");

        // Entry 4: end of input.
        extend_with_error_target_ipv4_entry(&mut entries_bytes, TARGET_ERROR);

        let mut bytes = ipt_replace {
            name: string_to_ascii_buffer("filter").unwrap(),
            num_entries: 5,
            size: entries_bytes.len() as u32,
            valid_hooks: NfIpHooks::FILTER.bits(),
            hook_entry: [0, 0, 152, 304, 0],
            underflow: [0, 0, 152, 304, 0],
            ..Default::default()
        }
        .as_bytes()
        .to_owned();

        bytes.extend(entries_bytes);
        bytes
    }

    #[test_case(
        table_with_wrong_size(),
        IpTableParseError::SizeMismatch { specified_size: 0, entries_size: 176 };
        "wrong size"
    )]
    #[test_case(
        table_with_wrong_num_entries(),
        IpTableParseError::NumEntriesMismatch { specified: 3, found: 1 };
        "wrong number of entries"
    )]
    #[test_case(
        table_with_no_entries(),
        IpTableParseError::NoTrailingErrorTarget;
        "no trailing error target"
    )]
    #[test_case(
        table_with_invalid_hook_bits(),
        IpTableParseError::InvalidHooksForTable { hooks: 3, table_name: TABLE_FILTER };
        "invalid hook bits"
    )]
    #[test_case(
        table_with_invalid_hook_entry(),
        IpTableParseError::InvalidHookEntryOrUnderflow { index: 1, start: 8, end: 8 };
        "hook does not point to entry"
    )]
    #[test_case(
        table_with_hook_ranges_overlap(),
        IpTableParseError::HookRangesOverlap { offset: 0 };
        "hook ranges overlap"
    )]
    #[test_case(
        table_with_unexpected_error_target(),
        IpTableParseError::UnexpectedErrorTarget { error_name: "mychain".to_owned() };
        "unexpected error target"
    )]
    #[test_case(
        table_with_chain_with_no_policy(),
        IpTableParseError::ChainHasNoPolicy { chain_name: String::from("mychain") };
        "chain with no policy"
    )]
    fn parse_table_error(bytes: Vec<u8>, expected_error: IpTableParseError) {
        let mut context = TestIpTablesContext::new();
        assert_eq!(IpTable::from_ipt_replace(&mut context, bytes).unwrap_err(), expected_error);
    }

    #[test_case(&[], Err(AsciiConversionError::NulByteNotFound { chars: vec![] }); "empty slice")]
    #[test_case(&[0], Ok(String::from("")); "size 1 slice")]
    #[test_case(
        &[102, 105, 108, 116, 101, 114, 0],
        Ok(String::from("filter"));
        "valid string with trailing nul byte"
    )]
    #[test_case(
        &[102, 105, 108, 116, 101, 114, 0, 0, 0, 0],
        Ok(String::from("filter"));
        "multiple trailing nul bytes"
    )]
    #[test_case(&[0; 8], Ok(String::from("")); "empty string")]
    #[test_case(&[0, 88, 88, 88, 88, 88], Ok(String::from("")); "ignores chars after nul byte")]
    fn ascii_to_string_test(input: &[c_char], expected: Result<String, AsciiConversionError>) {
        assert_eq!(ascii_to_string(input), expected);
    }

    #[fuchsia::test]
    fn ascii_to_string_non_ascii_test() {
        #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
        {
            let invalid_bytes: [c_char; 4] = [159, 146, 150, 0];
            assert_eq!(ascii_to_string(&invalid_bytes), Err(AsciiConversionError::NonAsciiChar));
        }
        #[cfg(target_arch = "x86_64")]
        {
            let invalid_bytes: [c_char; 4] = [-97, -110, -106, 0];
            assert_eq!(ascii_to_string(&invalid_bytes), Err(AsciiConversionError::NonAsciiChar));
        }
    }

    #[test_case("", Ok([0, 0, 0, 0, 0, 0, 0, 0]); "empty string")]
    #[test_case("filter", Ok([102, 105, 108, 116, 101, 114, 0, 0]); "valid string")]
    #[test_case(
        "very long string",
        Err(AsciiConversionError::BufferTooSmall { buffer_size: 8, data_size: 17 });
        "string does not fit"
    )]
    #[test_case(
        "\u{211D}",
        Err(AsciiConversionError::NonAsciiChar);
        "non-ASCII character"
    )]
    fn string_to_8_char_buffer_test(
        input: &str,
        output: Result<[c_char; 8], AsciiConversionError>,
    ) {
        assert_eq!(string_to_ascii_buffer::<8>(input), output);
    }

    #[test_case( [0, 0, 0, 0], [0x0, 0x0, 0x0, 0x0], Ok(None); "unset")]
    #[test_case(
        [127, 0, 0, 1],
        [0xff, 0xff, 0xff, 0xff],
        Ok(Some(fidl_subnet!("127.0.0.1/32")));
        "full address"
    )]
    #[test_case(
        [127, 0, 0, 1],
        [0xff, 0x0, 0x0, 0xff],
        Err(IpAddressConversionError::IpV4SubnetMaskHasNonPrefixBits { mask: 4278190335 });
        "invalid mask"
    )]
    #[test_case(
        [192, 0, 2, 15],
        [0xff, 0xff, 0xff, 0x0],
        Ok(Some(fidl_subnet!("192.0.2.15/24")));
        "subnet"
    )]
    fn ipv4_address_test(
        be_addr: [u8; 4],
        be_mask: [u8; 4],
        expected: Result<Option<fnet::Subnet>, IpAddressConversionError>,
    ) {
        assert_eq!(
            ipv4_to_subnet(
                in_addr { s_addr: u32::from_be_bytes(be_addr).to_be() },
                in_addr { s_addr: u32::from_be_bytes(be_mask).to_be() },
            ),
            expected
        );
    }

    #[test_case(
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        Ok(None);
        "unset"
    )]
    #[test_case(
        [0xff, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xc3],
        [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff
        ],
        Ok(Some(fidl_subnet!("ff06::c3/128")));
        "full address"
    )]
    #[test_case(
        [0xff, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xc3],
        [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0],
        Ok(Some(fidl_subnet!("ff06::c3/96")));
        "subnet"
    )]
    #[test_case(
        [0xff, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xc3],
        [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0xf],
        Err(IpAddressConversionError::IpV6SubnetMaskHasNonPrefixBits {
            mask: 340282366920938463463374607427473244175,
        });
        "invalid mask"
    )]
    fn ipv6_address_test(
        be_addr: [u8; 16],
        be_mask: [u8; 16],
        expected: Result<Option<fnet::Subnet>, IpAddressConversionError>,
    ) {
        assert_eq!(
            ipv6_to_subnet(
                in6_addr { in6_u: in6_addr__bindgen_ty_1 { u6_addr8: be_addr } },
                in6_addr { in6_u: in6_addr__bindgen_ty_1 { u6_addr8: be_mask } },
            ),
            expected
        );
    }
}
