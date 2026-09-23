// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use addr::{TargetAddr, TargetIpAddr};
use ffx_list_args::{AddressTypes, Format};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum FormatterError {
    #[error("Target must contain an address")]
    EmptyAddresses,

    #[error("Target must contain a serial number")]
    MissingSerialNumber,

    #[error("Invalid interface ID: {0}")]
    InvalidInterfaceId(#[from] netext::InvalidInterfaceIdError),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T, E = FormatterError> = std::result::Result<T, E>;
use ffx_target::TargetInfo;
use ffx_target::info::{RemoteControlState, TargetState};
use netext::ScopedSocketAddr;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cmp::max;
use std::fmt::Write;

const NAME: &'static str = "NAME";
const SERIAL: &'static str = "SERIAL";
const TYPE: &'static str = "TYPE";
const STATE: &'static str = "STATE";
const ADDRS: &'static str = "ADDRS/IP";
const RCS: &'static str = "RCS";
const MANUAL: &'static str = "MANUAL";

const UNKNOWN: &'static str = "<unknown>";

const PADDING_SPACES: usize = 4;

const DEFAULT_SSH_PORT: u16 = 22;

pub fn port_str(ta: TargetAddr) -> String {
    match ta {
        TargetAddr::Net(mut addr) => {
            let mut port = addr.port();
            if port == 0 {
                port = DEFAULT_SSH_PORT;
            }
            addr.set_port(port);
            addr.to_string()
        }
        TargetAddr::VSockCtx(_) | TargetAddr::UsbCtx(_) => format!("{ta}"),
    }
}

pub fn port_str_scoped(ta: TargetAddr) -> Result<String> {
    match ta {
        TargetAddr::Net(mut addr) => {
            let mut port = addr.port();
            if port == 0 {
                port = DEFAULT_SSH_PORT;
            }
            addr.set_port(port);
            let ssaddr = ScopedSocketAddr::from_socket_addr(addr)?;
            Ok(ssaddr.to_string())
        }
        TargetAddr::VSockCtx(_) | TargetAddr::UsbCtx(_) => Ok(ta.to_string()),
    }
}

/// Sanitizes a string from untrusted target metadata by stripping ANSI escape sequences
/// and control characters to prevent terminal injection attacks.
pub fn sanitize(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    // CSI sequence: ESC [ [0x30-0x3F]* [0x20-0x2F]* [0x40-0x7E]
                    chars.next();
                    while let Some(&p) = chars.peek() {
                        if (p >= '0' && p <= '9') || (p >= ':' && p <= '?') {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    while let Some(&p) = chars.peek() {
                        if p >= ' ' && p <= '/' {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    if let Some(&p) = chars.peek() {
                        if p >= '@' && p <= '~' {
                            chars.next();
                        }
                    }
                }
                Some(']') => {
                    // OSC sequence: ESC ] ... (BEL | ESC \)
                    chars.next();
                    while let Some(o) = chars.next() {
                        if o == '\x07' {
                            break;
                        }
                        if o == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some('P') | Some('X') | Some('^') | Some('_') => {
                    // DCS / SOS / PM / APC: ESC (P|X|^|_) ... (BEL | ESC \)
                    chars.next();
                    while let Some(o) = chars.next() {
                        if o == '\x07' {
                            break;
                        }
                        if o == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(&next_c) if next_c >= ' ' && next_c <= '/' => {
                    // Charset / 2-byte escape sequences: ESC [0x20-0x2F]+ [0x30-0x7E]
                    while let Some(&p) = chars.peek() {
                        if p >= ' ' && p <= '/' {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    if let Some(&p) = chars.peek() {
                        if p >= '0' && p <= '~' {
                            chars.next();
                        }
                    }
                }
                Some(&next_c) if next_c >= '@' && next_c <= '_' => {
                    // 2-byte Fe escape sequence: ESC [@-_]
                    chars.next();
                }
                _ => {
                    // Stray ESC, skip it.
                }
            }
        } else if c == '\u{009b}' {
            // C1 CSI
            while let Some(&p) = chars.peek() {
                if (p >= '0' && p <= '9') || (p >= ':' && p <= '?') {
                    chars.next();
                } else {
                    break;
                }
            }
            while let Some(&p) = chars.peek() {
                if p >= ' ' && p <= '/' {
                    chars.next();
                } else {
                    break;
                }
            }
            if let Some(&p) = chars.peek() {
                if p >= '@' && p <= '~' {
                    chars.next();
                }
            }
        } else if c == '\u{009d}'
            || c == '\u{0090}'
            || c == '\u{0098}'
            || c == '\u{009e}'
            || c == '\u{009f}'
        {
            // C1 OSC / DCS / SOS / PM / APC
            while let Some(o) = chars.next() {
                if o == '\x07' || o == '\u{009c}' {
                    break;
                }
                if o == '\x1b' && chars.peek() == Some(&'\\') {
                    chars.next();
                    break;
                }
            }
        } else if !c.is_control() {
            result.push(c);
        }
    }

    result
}

fn nodename_to_string(index: Option<usize>, nodename: Option<String>) -> String {
    match nodename {
        Some(name) => sanitize(&name),
        None => match index {
            Some(index) => format!("<unknown-{}>", index),
            None => target_errors::UNKNOWN_TARGET_NAME.to_owned(),
        },
    }
}

fn has_multiple_unknown_targets(targets: &Vec<TargetInfo>) -> bool {
    let mut unknown_count = 0;
    for target in targets.iter() {
        if target.nodename.is_none() {
            unknown_count += 1;
        }
        if unknown_count > 1 {
            return true;
        }
    }
    false
}

fn matches_addr_type(addr: &TargetAddr, ty: AddressTypes) -> bool {
    match addr {
        TargetAddr::Net(a) => {
            if a.is_ipv4() {
                ty.contains(AddressTypes::IPV4)
            } else {
                debug_assert!(a.is_ipv6());
                ty.contains(AddressTypes::IPV6)
            }
        }
        TargetAddr::VSockCtx(_) => ty.contains(AddressTypes::VSOCK),
        TargetAddr::UsbCtx(_) => ty.contains(AddressTypes::USB),
    }
}

pub fn filter_targets_by_address_types(
    targets: Vec<TargetInfo>,
    address_types: AddressTypes,
) -> Vec<TargetInfo> {
    targets
        .into_iter()
        .filter_map(|mut target| {
            if address_types.is_empty() {
                None
            } else {
                target.addresses.retain(|addr| matches_addr_type(addr, address_types));
                Some(target)
            }
        })
        .collect()
}

/// Simple trait for a target formatter.
pub trait TargetFormatter {
    fn lines(&self) -> Result<Vec<String>>;
}

impl TryFrom<(Format, AddressTypes, Vec<TargetInfo>)> for Box<dyn TargetFormatter> {
    type Error = FormatterError;

    fn try_from(tup: (Format, AddressTypes, Vec<TargetInfo>)) -> Result<Self> {
        let (format, address_types, targets) = tup;
        let targets = filter_targets_by_address_types(targets, address_types);
        Ok(match format {
            Format::Tabular => Box::new(TabularTargetFormatter::from(targets)),
            Format::Simple => Box::new(SimpleTargetFormatter::try_from(targets)?),
            Format::Addresses => Box::new(AddressesTargetFormatter::try_from(targets)?),
            Format::AddressesWithLexicalScope => {
                Box::new(AddressesWithLexicalScopeTargetFormatter::try_from(targets)?)
            }
            Format::Serials => Box::new(SerialsTargetFormatter::try_from(targets)?),
            Format::NameOnly => Box::new(NameOnlyTargetFormatter::from(targets)),
            Format::Json => Box::new(JsonTargetFormatter::from(targets)),
        })
    }
}

#[derive(Debug)]
pub struct AddressesTarget(TargetAddr);

impl TryFrom<TargetInfo> for AddressesTarget {
    type Error = FormatterError;

    fn try_from(t: TargetInfo) -> Result<Self> {
        // Prefer Usb or Vsock connections
        if let Some(addr) = t
            .addresses
            .iter()
            .find(|x| matches!(x, TargetAddr::UsbCtx(_) | TargetAddr::VSockCtx(_)))
        {
            return Ok(Self(*addr));
        }
        if t.addresses.is_empty() {
            return Err(FormatterError::EmptyAddresses);
        }
        Ok(Self(t.addresses[0]))
    }
}

pub struct AddressesTargetFormatter {
    targets: Vec<AddressesTarget>,
}

impl TryFrom<Vec<TargetInfo>> for AddressesTargetFormatter {
    type Error = FormatterError;

    fn try_from(targets: Vec<TargetInfo>) -> Result<Self> {
        // TODO(b/496914261): Re evaluate silently swallowing errors
        let targets = targets.into_iter().flat_map(AddressesTarget::try_from).collect::<Vec<_>>();
        Ok(Self { targets })
    }
}

impl TargetFormatter for AddressesTargetFormatter {
    fn lines(&self) -> Result<Vec<String>> {
        Ok(self.targets.iter().map(|t| port_str(t.0)).collect())
    }
}

pub struct AddressesWithLexicalScopeTargetFormatter {
    targets: Vec<AddressesTarget>,
}

impl TryFrom<Vec<TargetInfo>> for AddressesWithLexicalScopeTargetFormatter {
    type Error = FormatterError;

    fn try_from(targets: Vec<TargetInfo>) -> Result<Self> {
        // TODO(b/496914261): Re evaluate silently swallowing errors
        let targets = targets.into_iter().flat_map(AddressesTarget::try_from).collect::<Vec<_>>();
        Ok(Self { targets })
    }
}

impl TargetFormatter for AddressesWithLexicalScopeTargetFormatter {
    fn lines(&self) -> Result<Vec<String>> {
        self.targets.iter().map(|t| port_str_scoped(t.0)).collect()
    }
}

#[derive(Debug)]
pub struct SerialsTarget(String);

impl TryFrom<TargetInfo> for SerialsTarget {
    type Error = FormatterError;

    fn try_from(t: TargetInfo) -> Result<Self> {
        let Some(serial) = t.serial_number else {
            return Err(FormatterError::MissingSerialNumber);
        };
        Ok(Self(sanitize(&serial)))
    }
}

pub struct SerialsTargetFormatter {
    targets: Vec<SerialsTarget>,
}

impl TryFrom<Vec<TargetInfo>> for SerialsTargetFormatter {
    type Error = FormatterError;

    fn try_from(targets: Vec<TargetInfo>) -> Result<Self> {
        let targets = targets.into_iter().flat_map(SerialsTarget::try_from).collect::<Vec<_>>();
        Ok(Self { targets })
    }
}

impl TargetFormatter for SerialsTargetFormatter {
    fn lines(&self) -> Result<Vec<String>> {
        Ok(self.targets.iter().map(|t| t.0.clone()).collect())
    }
}

pub struct NameOnlyTarget(String);

impl From<(Option<usize>, TargetInfo)> for NameOnlyTarget {
    fn from((index, target): (Option<usize>, TargetInfo)) -> Self {
        let name = nodename_to_string(index, target.nodename);
        Self(name)
    }
}

pub struct NameOnlyTargetFormatter {
    targets: Vec<NameOnlyTarget>,
}

impl From<Vec<TargetInfo>> for NameOnlyTargetFormatter {
    fn from(targets: Vec<TargetInfo>) -> Self {
        let use_index = has_multiple_unknown_targets(&targets);
        let targets = targets
            .into_iter()
            .enumerate()
            .map(|(i, t)| (if use_index { Some(i) } else { None }, t))
            .map(NameOnlyTarget::from)
            .collect::<Vec<_>>();
        Self { targets }
    }
}

impl TargetFormatter for NameOnlyTargetFormatter {
    fn lines(&self) -> Result<Vec<String>> {
        Ok(self.targets.iter().map(|t| t.0.clone()).collect())
    }
}

#[derive(Debug)]
pub struct SimpleTarget(String, TargetAddr);

pub struct SimpleTargetFormatter {
    targets: Vec<SimpleTarget>,
}

impl TryFrom<Vec<TargetInfo>> for SimpleTargetFormatter {
    type Error = FormatterError;

    fn try_from(targets: Vec<TargetInfo>) -> Result<Self> {
        let targets = targets.into_iter().flat_map(SimpleTarget::try_from).collect::<Vec<_>>();
        Ok(Self { targets })
    }
}

impl TargetFormatter for SimpleTargetFormatter {
    fn lines(&self) -> Result<Vec<String>> {
        Ok(self.targets.iter().map(|t| format!("{} {}", t.1, t.0)).collect())
    }
}

impl TryFrom<TargetInfo> for SimpleTarget {
    type Error = FormatterError;

    fn try_from(t: TargetInfo) -> Result<Self> {
        let nodename = t.nodename.as_deref().map(sanitize).unwrap_or_else(|| "".to_string());
        let AddressesTarget(addr) = t.try_into()?;

        Ok(Self(nodename, addr))
    }
}

pub struct JsonTargetFormatter {
    pub targets: Vec<JsonTarget>,
}

impl From<Vec<TargetInfo>> for JsonTargetFormatter {
    fn from(targets: Vec<TargetInfo>) -> Self {
        let use_index = has_multiple_unknown_targets(&targets);
        let targets = targets
            .into_iter()
            .enumerate()
            .map(|(i, t)| (if use_index { Some(i) } else { None }, t))
            .map(JsonTarget::from)
            .collect::<Vec<_>>();
        Self { targets }
    }
}

impl TargetFormatter for JsonTargetFormatter {
    fn lines(&self) -> Result<Vec<String>> {
        let t = self.targets.clone();
        Ok(vec![serde_json::to_string(&t)?])
    }
}

#[derive(Debug, PartialEq, Eq)]
enum StringifiedField {
    String(String),
    Array(Vec<String>),
}

impl Default for StringifiedField {
    fn default() -> Self {
        StringifiedField::String(String::new())
    }
}

impl StringifiedField {
    fn len(&self) -> usize {
        match self {
            StringifiedField::String(_s) => 1,
            StringifiedField::Array(a) => a.len(),
        }
    }

    fn string_len(&self) -> usize {
        match self {
            StringifiedField::String(s) => s.len(),
            StringifiedField::Array(a) => a.iter().map(|s| s.len()).max().unwrap_or(0),
        }
    }

    fn at_index(&self, index: usize) -> Option<String> {
        match self {
            StringifiedField::String(s) => {
                if index == 0 {
                    Some(s.clone())
                } else {
                    None
                }
            }
            StringifiedField::Array(a) => a.get(index).cloned(),
        }
    }
}

// Convenience macro to make potential addition/removal of fields less likely
// to affect internal logic. Other functions that construct these targets will
// fail to compile if more fields are added.
macro_rules! make_structs_and_support_functions {
    ($( $field:ident ),+ $(,)?) => {
        #[derive(Default)]
        struct Limits {
            $(
                $field: usize,
            )*
        }

        impl Limits {
            fn update(&mut self, target: &mut StringifiedTarget) {
                $(
                    self.$field = max(self.$field, target.$field.string_len());
                    target.__longest_array = max(target.__longest_array, target.$field.len());
                )*
            }

            fn capacity(&self) -> usize {
                let mut result = 0;
                $(
                    result += self.$field + PADDING_SPACES;
                )*
                result
            }
        }

        #[derive(Debug, PartialEq, Eq)]
        struct StringifiedTarget {
            __longest_array: usize,
            __is_default: bool,
            $(
                $field: StringifiedField,
            )*
        }

        impl Default for StringifiedTarget {
            fn default() -> Self {
                Self {
                    __longest_array: 0,
                    __is_default: false,
                    $(
                        $field: StringifiedField::default(),
                    )*
                }
            }
        }

        make_structs_and_support_functions!(@print_func $($field,)*);
    };

    (@print_func $nodename:ident, $last_field:ident, $($field:ident),* $(,)?) => {
        #[inline]
        fn format_fields(target: &StringifiedTarget, limits: &Limits) -> String {
            fn format_fields_(target: &StringifiedTarget, limits: &Limits, index: usize) -> String {
                let mut s = String::with_capacity(limits.capacity());
                let nodename = match target.$nodename.at_index(index) {
                    Some(nodename) => nodename,
                    None => String::new(),
                };
                write!(s, "{:width$}", nodename, width = limits.$nodename + PADDING_SPACES).unwrap();
                $(
                    write!(s, "{:width$}", target.$field.at_index(index).unwrap_or_else(String::new), width = limits.$field + PADDING_SPACES).unwrap();
                )*
                // Skips spaces on the end.
                write!(s, "{}", target.$last_field.at_index(index).unwrap_or_else(String::new)).unwrap();
                s
            }

            (0..target.__longest_array).map(|i| format_fields_(target, limits, i)).collect::<Vec<_>>().join("\n")
        }
    };
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq, JsonSchema)]
#[serde(tag = "type")]
pub enum JsonTargetAddress {
    Ip { ip: String, ssh_port: u16 },
    VSock { cid: u32 },
    Usb { cid: u32 },
}

impl From<TargetAddr> for JsonTargetAddress {
    fn from(addr: TargetAddr) -> Self {
        match &addr {
            TargetAddr::Net(sock_addr) => JsonTargetAddress::Ip {
                ip: ScopedSocketAddr::from_socket_addr(*sock_addr)
                    .map(|s| s.ip_string())
                    .unwrap_or_else(|_| TargetIpAddr::from(*sock_addr).resolved_str()),
                ssh_port: sock_addr.port(),
            },
            TargetAddr::VSockCtx(cid) => JsonTargetAddress::VSock { cid: *cid },
            TargetAddr::UsbCtx(cid) => JsonTargetAddress::Usb { cid: *cid },
        }
    }
}

// LINT.IfChange
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq, JsonSchema)]
pub struct JsonTarget {
    nodename: String,
    rcs_state: String,
    serial: String,
    target_type: String,
    target_state: String,
    addresses: Vec<JsonTargetAddress>,
    is_default: bool,
    is_manual: bool,
}
// LINT.ThenChange(//src/testing/host-target-testing/ffx/ffx.go)

// Second field is printed last in this implementation, everything else is printed in order.
make_structs_and_support_functions!(
    nodename,
    rcs_state,
    serial,
    target_type,
    target_state,
    addresses,
    is_manual,
);

impl StringifiedTarget {
    fn from_target_addr(addr: TargetAddr) -> String {
        addr.optional_port_str()
    }

    fn from_addresses(mut v: Vec<TargetAddr>) -> String {
        format!(
            "[{}]",
            v.drain(..)
                .map(|a| StringifiedTarget::from_target_addr(a))
                .collect::<Vec<_>>()
                .join(",; ")
        )
    }

    fn field_from_addresses(v: Vec<TargetAddr>) -> StringifiedField {
        let all_addresses = StringifiedTarget::from_addresses(v);
        StringifiedField::Array(all_addresses.split(';').map(String::from).collect::<Vec<_>>())
    }

    fn from_rcs_state(r: RemoteControlState) -> String {
        match r {
            RemoteControlState::Down | RemoteControlState::Unknown => "N".to_string(),
            RemoteControlState::Up => "Y".to_string(),
        }
    }

    fn from_target_type(board_config: Option<&str>, product_config: Option<&str>) -> String {
        match (board_config, product_config) {
            (None, None) => String::from("Unknown"),
            (board, product) => {
                let board = board.map(sanitize);
                let product = product.map(sanitize);
                format!(
                    "{}.{}",
                    product.as_deref().unwrap_or(UNKNOWN),
                    board.as_deref().unwrap_or(UNKNOWN)
                )
            }
        }
    }

    fn from_target_state(t: TargetState) -> String {
        match t {
            TargetState::Unknown => "Unknown".to_string(),
            TargetState::Product => "Product".to_string(),
            TargetState::Fastboot => "Fastboot".to_string(),
            TargetState::Zedboot => "Zedboot (R)".to_string(),
        }
    }

    fn from_bool(b: bool) -> String {
        String::from(if b { "Y" } else { "N" })
    }
}

impl From<(Option<usize>, TargetInfo)> for StringifiedTarget {
    fn from((index, target): (Option<usize>, TargetInfo)) -> Self {
        let target_type = StringifiedTarget::from_target_type(
            target.board_config.as_deref(),
            target.product_config.as_deref(),
        );
        Self {
            __is_default: target.is_default.unwrap_or_default(),
            nodename: StringifiedField::String(nodename_to_string(index, target.nodename)),
            serial: StringifiedField::String(
                target
                    .serial_number
                    .as_deref()
                    .map(sanitize)
                    .unwrap_or_else(|| UNKNOWN.to_string()),
            ),
            addresses: StringifiedTarget::field_from_addresses(target.addresses),
            rcs_state: StringifiedField::String(StringifiedTarget::from_rcs_state(
                target.rcs_state,
            )),
            target_type: StringifiedField::String(target_type),
            target_state: StringifiedField::String(StringifiedTarget::from_target_state(
                target.target_state,
            )),
            is_manual: StringifiedField::String(StringifiedTarget::from_bool(target.is_manual)),
            ..Default::default()
        }
    }
}

impl From<(Option<usize>, TargetInfo)> for JsonTarget {
    fn from((index, target): (Option<usize>, TargetInfo)) -> Self {
        Self {
            nodename: nodename_to_string(index, target.nodename),
            serial: target
                .serial_number
                .as_deref()
                .map(sanitize)
                .unwrap_or_else(|| UNKNOWN.to_string()),
            addresses: target
                .addresses
                .into_iter()
                .map(JsonTargetAddress::from)
                .collect::<Vec<_>>(),
            rcs_state: StringifiedTarget::from_rcs_state(target.rcs_state),
            target_type: StringifiedTarget::from_target_type(
                target.board_config.as_deref(),
                target.product_config.as_deref(),
            ),
            target_state: StringifiedTarget::from_target_state(target.target_state),
            is_default: target.is_default.unwrap_or_default(),
            is_manual: target.is_manual,
        }
    }
}

pub struct TabularTargetFormatter {
    targets: Vec<StringifiedTarget>,
    limits: Limits,
}

impl TargetFormatter for TabularTargetFormatter {
    fn lines(&self) -> Result<Vec<String>> {
        Ok(self.targets.iter().map(|t| format_fields(t, &self.limits)).collect())
    }
}

impl From<Vec<TargetInfo>> for TabularTargetFormatter {
    fn from(mut targets: Vec<TargetInfo>) -> Self {
        // First target is the table header in this case, since the formatting
        // for the table header is (for now) identical to the rest of the
        // targets
        let mut initial = vec![StringifiedTarget {
            nodename: StringifiedField::String(NAME.to_string()),
            serial: StringifiedField::String(SERIAL.to_string()),
            addresses: StringifiedField::String(ADDRS.to_string()),
            rcs_state: StringifiedField::String(RCS.to_string()),
            is_manual: StringifiedField::String(MANUAL.to_string()),
            target_type: StringifiedField::String(TYPE.to_string()),
            target_state: StringifiedField::String(STATE.to_string()),
            ..Default::default()
        }];
        let mut limits = Limits::default();
        limits.update(&mut initial[0]);

        let acc = Self { targets: initial, limits };
        let use_index = has_multiple_unknown_targets(&targets);
        targets.drain(..).enumerate().fold(acc, |mut a, (index, t)| {
            let index = if use_index { Some(index) } else { None };
            let mut s = StringifiedTarget::from((index, t));
            a.limits.update(&mut s);
            a.targets.push(s);
            a
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use net_declare::std_ip;
    use std::collections::HashMap;
    use std::sync::LazyLock;

    fn make_target(addr: TargetAddr) -> TargetInfo {
        TargetInfo {
            nodename: Some("lorberding".to_string()),
            addresses: vec![addr],
            rcs_state: RemoteControlState::Unknown,
            target_state: TargetState::Unknown,
            ..Default::default()
        }
    }

    fn make_ip_v4_addr(port: u16) -> TargetAddr {
        TargetAddr::new(std_ip!("127.0.0.1"), 0, port)
    }

    fn make_ip_v6_port_info(scope_id: u32, port: u16) -> TargetAddr {
        TargetAddr::new(std_ip!("fe80::1:101:101:101"), scope_id, port)
    }

    fn make_usb_addr(ctx: u32) -> TargetAddr {
        TargetAddr::UsbCtx(ctx)
    }

    static EMPTY_FORMATTER_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_empty_formatter_golden").trim().to_owned()
    });
    static ONE_TARGET_WITH_DEFAULT_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_one_target_with_default_golden")
            .trim()
            .to_owned()
    });
    static ONE_TARGET_NO_DEFAULT_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_one_target_no_default_golden").trim().to_owned()
    });
    static EMPTY_NODENAME_WITH_DEFAULT_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_empty_nodename_with_default_golden")
            .trim()
            .to_owned()
    });
    static EMPTY_NODENAME_WITH_DEFAULT_MULTIPLE_UNKNOWN_GOLDEN: LazyLock<String> =
        LazyLock::new(|| {
            include_str!(
                "../test_data/target_formatter_empty_nodename_with_default_multiple_unknown_golden"
            )
            .trim()
            .to_owned()
        });
    static EMPTY_NODENAME_NO_DEFAULT_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_empty_nodename_no_default_golden")
            .trim()
            .to_owned()
    });
    static SIMPLE_FORMATTER_WITH_DEFAULT_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_simple_formatter_with_default_golden")
            .trim()
            .to_owned()
    });
    static NAME_ONLY_FORMATTER_WITH_DEFAULT_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_name_only_formatter_with_default_golden")
            .trim()
            .to_owned()
    });
    static NAME_ONLY_FORMATTER_MULTIPLE_UNKNOWN_WITH_DEFAULT_GOLDEN: LazyLock<String> =
        LazyLock::new(|| {
            include_str!("../test_data/target_formatter_name_only_multiple_unknown_formatter_with_default_golden").trim().to_owned()
        });
    static DEVICE_FINDER_FORMAT_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_device_finder_format_golden").trim().to_owned()
    });
    static DEVICE_FINDER_FORMAT_IPV4_ONLY_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_device_finder_format_ipv4_only_golden")
            .trim()
            .to_owned()
    });
    static DEVICE_FINDER_FORMAT_IPV6_ONLY_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_device_finder_format_ipv6_only_golden")
            .trim()
            .to_owned()
    });
    static ADDRESSES_FORMAT_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_addresses_format_golden").trim().to_owned()
    });
    static BUILD_CONFIG_FULL_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_build_config_full_golden").trim().to_owned()
    });
    static BUILD_CONFIG_PRODUCT_MISSING_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_build_config_product_missing_golden")
            .trim()
            .to_owned()
    });
    static BUILD_CONFIG_BOARD_MISSING_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_build_config_board_missing_golden")
            .trim()
            .to_owned()
    });
    static JSON_BUILD_CONFIG_FULL_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_json_build_config_full_golden")
            .trim()
            .to_owned()
    });
    static JSON_BUILD_CONFIG_FULL_DEFAULT_TARGET_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_json_build_config_full_default_target_golden")
            .trim()
            .to_owned()
    });
    static JSON_BUILD_CONFIG_PRODUCT_MISSING_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_json_build_config_product_missing_golden")
            .trim()
            .to_owned()
    });
    static JSON_BUILD_CONFIG_BOARD_MISSING_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_json_build_config_board_missing_golden")
            .trim()
            .to_owned()
    });
    static JSON_BUILD_CONFIG_BOTH_MISSING_GOLDEN: LazyLock<String> = LazyLock::new(|| {
        include_str!("../test_data/target_formatter_json_build_config_both_missing_golden")
            .trim()
            .to_owned()
    });

    fn make_valid_target_with_default(default: bool) -> TargetInfo {
        let is_default = if default { Some(true) } else { None };
        TargetInfo {
            nodename: Some("fooberdoober".to_string()),
            addresses: vec![
                TargetAddr::new(std_ip!("101:101:101:101:101:101:101:101"), 198, 0),
                TargetAddr::new(std_ip!("122.24.25.25"), 186, 0),
            ],
            is_default,
            ..Default::default()
        }
    }

    fn make_valid_default_target() -> TargetInfo {
        make_valid_target_with_default(true)
    }

    fn make_valid_target() -> TargetInfo {
        make_valid_target_with_default(false)
    }

    fn make_valid_ipv4_only_target() -> TargetInfo {
        TargetInfo {
            nodename: Some("fooberdoober4".to_string()),
            addresses: vec![TargetAddr::new(std_ip!("122.24.25.25"), 186, 0)],
            rcs_state: RemoteControlState::Unknown,
            target_state: TargetState::Unknown,
            ..Default::default()
        }
    }

    #[test]
    fn test_matches_addr_type() {
        let ipv4 = TargetAddr::new(std_ip!("127.0.0.1"), 0, 0);
        let ipv6 = TargetAddr::new(std_ip!("fe80::1"), 0, 0);
        let usb = TargetAddr::UsbCtx(1);
        let vsock = TargetAddr::VSockCtx(2);

        assert!(matches_addr_type(&ipv4, AddressTypes::IPV4));
        assert!(!matches_addr_type(&ipv4, AddressTypes::IPV6));
        assert!(matches_addr_type(&ipv6, AddressTypes::IPV6));
        assert!(!matches_addr_type(&ipv6, AddressTypes::IPV4));
        assert!(matches_addr_type(&usb, AddressTypes::USB));
        assert!(!matches_addr_type(&usb, AddressTypes::VSOCK));
        assert!(matches_addr_type(&vsock, AddressTypes::VSOCK));
        assert!(!matches_addr_type(&vsock, AddressTypes::USB));
    }

    #[test]
    fn test_json_target_address_from_invalid_scope() {
        let addr = TargetAddr::new(std_ip!("fe80::1"), 65535, 8080);
        let json_addr = JsonTargetAddress::from(addr);
        match json_addr {
            JsonTargetAddress::Ip { ip, ssh_port } => {
                assert_eq!(ip, "fe80::1%65535");
                assert_eq!(ssh_port, 8080);
            }
            _ => panic!("Expected JsonTargetAddress::Ip"),
        }
    }

    #[test]
    fn test_empty_formatter() {
        let formatter = TabularTargetFormatter::try_from(Vec::<TargetInfo>::new()).unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 60); // Just some manual math.
        assert_eq!(lines.join("\n"), EMPTY_FORMATTER_GOLDEN.to_string());
    }

    #[fuchsia::test]
    async fn test_formatter_one_target() {
        let formatter = TabularTargetFormatter::try_from(vec![
            make_valid_default_target(),
            TargetInfo {
                nodename: Some("lorberding".to_string()),
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                ..Default::default()
            },
        ])
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines.join("\n"), ONE_TARGET_WITH_DEFAULT_GOLDEN.to_string());

        let formatter = TabularTargetFormatter::try_from(vec![
            make_valid_target(),
            TargetInfo {
                nodename: Some("lorberding".to_string()),
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                ..Default::default()
            },
        ])
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines.join("\n"), ONE_TARGET_NO_DEFAULT_GOLDEN.to_string());
    }

    #[fuchsia::test]
    async fn test_formatter_empty_nodename() {
        let formatter = TabularTargetFormatter::try_from(vec![
            make_valid_default_target(),
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                serial_number: Some("cereal".to_owned()),
                ..Default::default()
            },
        ])
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines.join("\n"), EMPTY_NODENAME_WITH_DEFAULT_GOLDEN.to_string());

        let formatter = TabularTargetFormatter::try_from(vec![
            make_valid_target(),
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                serial_number: Some("cereal".to_owned()),
                ..Default::default()
            },
        ])
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines.join("\n"), EMPTY_NODENAME_NO_DEFAULT_GOLDEN.to_string());
    }

    #[fuchsia::test]
    async fn test_formatter_multiple_empty_nodename() {
        let formatter = TabularTargetFormatter::try_from(vec![
            make_valid_default_target(),
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                serial_number: Some("cereal".to_owned()),
                ..Default::default()
            },
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:100"), 42, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                ..Default::default()
            },
        ])
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines.join("\n"),
            EMPTY_NODENAME_WITH_DEFAULT_MULTIPLE_UNKNOWN_GOLDEN.to_string()
        );
    }

    #[fuchsia::test]
    async fn test_simple_formatter() {
        let formatter = SimpleTargetFormatter::try_from(vec![
            make_valid_default_target(),
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                ..Default::default()
            },
        ])
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines.join("\n").trim(), SIMPLE_FORMATTER_WITH_DEFAULT_GOLDEN.to_string());

        let formatter = SimpleTargetFormatter::try_from(vec![
            make_valid_target(),
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                ..Default::default()
            },
        ])
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines.join("\n").trim(), SIMPLE_FORMATTER_WITH_DEFAULT_GOLDEN.to_string());
    }

    #[test]
    fn test_simple_formatter_with_invalid() {
        let names =
            vec!["nodename0", "nodename1", "nodename2", "nodename3", "nodename4", "nodename5"];
        let mut targets = names
            .into_iter()
            .map(|name| {
                let mut t = make_valid_target();
                t.nodename = Some(name.to_string());
                t
            })
            .collect::<Vec<_>>();

        targets[1].addresses = vec![];
        targets[3].rcs_state = RemoteControlState::Unknown;

        let formatter = SimpleTargetFormatter::try_from(targets).unwrap();
        assert_eq!(formatter.targets.len(), 5);
    }

    #[fuchsia::test]
    async fn test_name_only_formatter() {
        let formatter = NameOnlyTargetFormatter::try_from(vec![
            make_valid_default_target(),
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                ..Default::default()
            },
        ])
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines.join("\n"), NAME_ONLY_FORMATTER_WITH_DEFAULT_GOLDEN.to_string());

        let formatter = NameOnlyTargetFormatter::try_from(vec![
            make_valid_target(),
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                ..Default::default()
            },
        ])
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines.join("\n"), NAME_ONLY_FORMATTER_WITH_DEFAULT_GOLDEN.to_string());
    }

    #[fuchsia::test]
    async fn test_name_only_multiple_unknown_formatter() {
        let formatter = NameOnlyTargetFormatter::try_from(vec![
            make_valid_default_target(),
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                ..Default::default()
            },
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 42, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                ..Default::default()
            },
        ])
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines.join("\n"),
            NAME_ONLY_FORMATTER_MULTIPLE_UNKNOWN_WITH_DEFAULT_GOLDEN.to_string()
        );

        let formatter = NameOnlyTargetFormatter::try_from(vec![
            make_valid_target(),
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                ..Default::default()
            },
            TargetInfo {
                nodename: None,
                addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 42, 0)],
                rcs_state: RemoteControlState::Unknown,
                target_state: TargetState::Unknown,
                ..Default::default()
            },
        ])
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines.join("\n"),
            NAME_ONLY_FORMATTER_MULTIPLE_UNKNOWN_WITH_DEFAULT_GOLDEN.to_string()
        );
    }

    #[test]
    fn test_name_only_formatter_with_invalid() {
        let names =
            vec!["nodename0", "nodename1", "nodename2", "nodename3", "nodename4", "nodename5"];
        let mut targets = names
            .into_iter()
            .map(|name| {
                let mut t = make_valid_target();
                t.nodename = Some(name.to_string());
                t
            })
            .collect::<Vec<_>>();

        targets[1].addresses = vec![];
        targets[3].rcs_state = RemoteControlState::Unknown;

        let formatter = NameOnlyTargetFormatter::try_from(targets).unwrap();
        // NameOnlyTargetFormatter is infalliable
        assert_eq!(formatter.targets.len(), 6);
    }

    #[test]
    fn test_stringified_target_missing_nodename() {
        let mut t = make_valid_target();
        t.nodename = None;
        assert!(StringifiedTarget::try_from((None, t)).is_ok());
    }

    #[test]
    fn test_device_finder_format() {
        let formatter = Box::<dyn TargetFormatter>::try_from((
            Format::Simple,
            AddressTypes::all(),
            vec![make_valid_target(), make_valid_target()],
        ))
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n"), DEVICE_FINDER_FORMAT_GOLDEN.to_string());
    }

    #[test]
    fn test_device_finder_format_ipv4_only() {
        let formatter = Box::<dyn TargetFormatter>::try_from((
            Format::Simple,
            AddressTypes::IPV4,
            vec![make_valid_ipv4_only_target(), make_valid_target()],
        ))
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n"), DEVICE_FINDER_FORMAT_IPV4_ONLY_GOLDEN.to_string());
    }

    #[test]
    fn test_device_finder_format_ipv6_only() {
        let formatter = Box::<dyn TargetFormatter>::try_from((
            Format::Simple,
            AddressTypes::IPV6,
            vec![make_valid_ipv4_only_target(), make_valid_target()],
        ))
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n"), DEVICE_FINDER_FORMAT_IPV6_ONLY_GOLDEN.to_string());
    }

    #[test]
    fn test_addresses_format() {
        let formatter = Box::<dyn TargetFormatter>::try_from((
            Format::Addresses,
            AddressTypes::all(),
            vec![make_valid_target(), make_valid_target()],
        ))
        .unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n"), ADDRESSES_FORMAT_GOLDEN.to_string());
    }

    #[test]
    fn test_build_config_full() {
        let b = String::from("board");
        let p = String::from("default");
        let mut t = make_valid_target();
        t.board_config = Some(b);
        t.product_config = Some(p);
        let formatter = TabularTargetFormatter::try_from(vec![t]).unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n").trim(), BUILD_CONFIG_FULL_GOLDEN.to_string());
    }

    #[test]
    fn test_build_config_product_missing() {
        let b = String::from("x64");
        let mut t = make_valid_target();
        t.board_config = Some(b);
        t.product_config = None;
        let formatter = TabularTargetFormatter::try_from(vec![t]).unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n").trim(), BUILD_CONFIG_PRODUCT_MISSING_GOLDEN.to_string());
    }

    #[test]
    fn test_build_config_board_missing() {
        let p = String::from("foo");
        let mut t = make_valid_target();
        t.board_config = None;
        t.product_config = Some(p);
        let formatter = TabularTargetFormatter::try_from(vec![t]).unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n").trim(), BUILD_CONFIG_BOARD_MISSING_GOLDEN.to_string());
    }

    #[test]
    fn test_json_target_address_from_ipv6_scope() {
        // Using index 1, which typically maps to a symbolic name like "lo" or "eth0" on the host.
        let addr = TargetAddr::new(std_ip!("fe80::1"), 1, 8080);
        let json_addr = JsonTargetAddress::from(addr);

        assert_eq!(
            json_addr,
            JsonTargetAddress::Ip { ip: "fe80::1%lo".to_string(), ssh_port: 8080 }
        );
    }

    #[test]
    fn test_stringified_product_state() {
        let mut t = make_valid_target();
        t.target_state = TargetState::Product;
        assert!(StringifiedTarget::try_from((None, t)).is_ok());
    }

    #[test]
    fn test_stringified_fastboot_state() {
        let mut t = make_valid_target();
        t.target_state = TargetState::Fastboot;
        assert!(StringifiedTarget::try_from((None, t)).is_ok());
    }

    #[test]
    fn test_stringified_unknown_state() {
        let mut t = make_valid_target();
        t.target_state = TargetState::Unknown;
        assert!(StringifiedTarget::try_from((None, t)).is_ok());
    }

    #[test]
    fn test_addresses_target_formatter_some_invalid() {
        let names =
            vec!["nodename0", "nodename1", "nodename2", "nodename3", "nodename4", "nodename5"];
        let mut targets = names
            .into_iter()
            .map(|name| {
                let mut t = make_valid_target();
                t.nodename = Some(name.to_string());
                t
            })
            .collect::<Vec<_>>();

        targets[1].addresses = vec![];
        targets[3].rcs_state = RemoteControlState::Unknown;
        targets[4].addresses = vec![];

        let formatter = AddressesTargetFormatter::try_from(targets).unwrap();
        assert_eq!(formatter.targets.len(), 4);
    }
    #[test]
    fn test_json_target_formatter_valid() {
        let names =
            vec!["nodename0", "nodename1", "nodename2", "nodename3", "nodename4", "nodename5"];
        let targets = names
            .into_iter()
            .map(|name| {
                let mut t = make_valid_target();
                t.nodename = Some(name.to_string());
                t
            })
            .collect::<Vec<_>>();

        let formatter = JsonTargetFormatter::try_from(targets).unwrap();
        assert_eq!(formatter.targets.len(), 6);
    }

    #[test]
    fn test_json_formatter_build_config_full() {
        let b = String::from("board");
        let p = String::from("default");
        let mut t = make_valid_target();
        t.board_config = Some(b);
        t.product_config = Some(p);
        let formatter = JsonTargetFormatter::try_from(vec![t]).unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n"), JSON_BUILD_CONFIG_FULL_GOLDEN.to_string());
    }

    #[test]
    fn test_json_formatter_build_config_full_default_target() {
        let b = String::from("board");
        let p = String::from("default");
        let mut t = make_valid_default_target();
        t.board_config = Some(b);
        t.product_config = Some(p);
        let formatter = JsonTargetFormatter::try_from(vec![t]).unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n"), JSON_BUILD_CONFIG_FULL_DEFAULT_TARGET_GOLDEN.to_string());
    }

    #[test]
    fn test_json_formatter_build_config_product_missing() {
        let b = String::from("x64");
        let mut t = make_valid_target();
        t.board_config = Some(b);
        t.product_config = None;
        let formatter = JsonTargetFormatter::try_from(vec![t]).unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n"), JSON_BUILD_CONFIG_PRODUCT_MISSING_GOLDEN.to_string());
    }

    #[test]
    fn test_json_formatter_build_config_board_missing() {
        let p = String::from("foo");
        let mut t = make_valid_target();
        t.board_config = None;
        t.product_config = Some(p);
        let formatter = JsonTargetFormatter::try_from(vec![t]).unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n"), JSON_BUILD_CONFIG_BOARD_MISSING_GOLDEN.to_string());
    }

    #[test]
    fn test_json_formatter_build_config_both_missing() {
        let mut t = make_valid_target();
        t.board_config = None;
        t.product_config = None;
        let formatter = JsonTargetFormatter::try_from(vec![t]).unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.join("\n"), JSON_BUILD_CONFIG_BOTH_MISSING_GOLDEN.to_string());
    }

    fn get_first_address(json: &str) -> (String, u16) {
        let parsed_json: Vec<HashMap<String, serde_json::Value>> =
            serde_json::from_str(&json).unwrap();
        let addresses: Vec<serde_json::Value> =
            serde_json::from_value(parsed_json[0]["addresses"].clone()).unwrap();
        let first_address: HashMap<String, serde_json::Value> =
            serde_json::from_value(addresses[0].clone()).unwrap();
        let ip = serde_json::from_value(first_address["ip"].clone()).unwrap();
        let port = serde_json::from_value(first_address["ssh_port"].clone()).unwrap();
        (ip, port)
    }

    #[fuchsia::test]
    async fn test_nonstandard_port_ipv4() {
        let target = make_target(make_ip_v4_addr(1234));
        let formatter = JsonTargetFormatter::try_from(vec![target.clone()]).unwrap();
        let json = formatter.lines().unwrap()[0].clone();
        let (first_ip, first_port) = get_first_address(&json);
        assert_eq!(first_ip, "127.0.0.1".to_string());
        assert_eq!(first_port, 1234);

        let formatter = TabularTargetFormatter::try_from(vec![target.clone()]).unwrap();
        let out = formatter.lines().unwrap()[1].clone();
        assert!(out.contains("127.0.0.1:1234"));

        let formatter = AddressesTargetFormatter::try_from(vec![target.clone()]).unwrap();
        let out = formatter.lines().unwrap()[0].clone();
        assert_eq!(out, "127.0.0.1:1234".to_string());
    }

    #[fuchsia::test]
    async fn test_nonstandard_port_ipv6() {
        let addr = make_ip_v6_port_info(1, 1234);
        let target = make_target(addr);
        let formatter = JsonTargetFormatter::try_from(vec![target.clone()]).unwrap();
        let json = formatter.lines().unwrap()[0].clone();
        let (first_ip, first_port) = get_first_address(&json);
        assert_eq!(first_ip, "fe80::1:101:101:101%lo".to_string());
        assert_eq!(first_port, 1234);

        let formatter = TabularTargetFormatter::try_from(vec![target.clone()]).unwrap();
        let out = formatter.lines().unwrap()[1].clone();
        assert!(out.contains("[fe80::1:101:101:101%lo]:1234"));

        let formatter = AddressesTargetFormatter::try_from(vec![target.clone()]).unwrap();
        let out = formatter.lines().unwrap()[0].clone();
        assert_eq!(out, "[fe80::1:101:101:101%1]:1234".to_string());
    }

    #[fuchsia::test]
    async fn test_addresses_std_ports() {
        let target = make_target(make_ip_v4_addr(0));

        let formatter = AddressesTargetFormatter::try_from(vec![target.clone()]).unwrap();
        let out = formatter.lines().unwrap()[0].clone();
        assert_eq!(out, "127.0.0.1:22".to_string());

        let target = make_target(make_ip_v6_port_info(0, 22));
        let formatter = AddressesTargetFormatter::try_from(vec![target.clone()]).unwrap();
        let out = formatter.lines().unwrap()[0].clone();
        assert_eq!(out, "[fe80::1:101:101:101]:22".to_string());
    }

    #[fuchsia::test]
    async fn test_address_target() {
        let mut t = make_target(make_ip_v4_addr(0));
        let usb_addr = make_usb_addr(1);
        t.addresses.push(usb_addr);
        let addr_target = AddressesTarget::try_from(t).unwrap();
        assert_eq!(addr_target.0, usb_addr);
    }

    #[fuchsia::test]
    async fn test_formatter_one_target_is_default() {
        let mut target1 = make_valid_target();
        target1.nodename = Some("default-target".to_string());
        target1.is_default = Some(true);
        let target2 = TargetInfo {
            nodename: Some("other-target".to_string()),
            addresses: vec![TargetAddr::new(std_ip!("fe80::101:101:101:101"), 137, 0)],
            rcs_state: RemoteControlState::Unknown,
            target_state: TargetState::Unknown,
            ..Default::default()
        };
        let formatter = TabularTargetFormatter::from(vec![target1, target2]);
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 3);
        let default_target_line = &lines[1];
        assert!(default_target_line.contains("default-target"));
        assert!(!default_target_line.contains("default-target*"));
        let other_target_line = &lines[2];
        assert!(other_target_line.contains("other-target"));
        assert!(!other_target_line.contains("other-target*"));
    }

    #[test]
    fn test_json_formatter_is_default() {
        let mut t = make_valid_target();
        t.is_default = Some(true);
        let formatter = JsonTargetFormatter::from(vec![t]);
        let json_target = &formatter.targets[0];
        assert!(json_target.is_default);
    }

    #[test]
    fn test_addresses_target_try_from_empty_addresses() {
        let mut t = make_valid_target();
        t.addresses = vec![];
        let err = AddressesTarget::try_from(t).unwrap_err();
        assert!(matches!(err, FormatterError::EmptyAddresses));
    }

    #[test]
    fn test_serials_target_try_from_missing_serial() {
        let mut t = make_valid_target();
        t.serial_number = None;
        let err = SerialsTarget::try_from(t).unwrap_err();
        assert!(matches!(err, FormatterError::MissingSerialNumber));
    }

    #[test]
    fn test_simple_target_try_from_empty_addresses() {
        let mut t = make_valid_target();
        t.addresses = vec![];
        let err = SimpleTarget::try_from(t).unwrap_err();
        assert!(matches!(err, FormatterError::EmptyAddresses));
    }

    #[test]
    fn test_port_str_scoped_invalid_interface() {
        // Use an absurdly high scope ID that shouldn't map to a real interface.
        let addr = TargetAddr::new(std_ip!("fe80::1"), 999999, 8080);
        let res = port_str_scoped(addr);
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), FormatterError::InvalidInterfaceId(_)));
    }

    #[test]
    fn test_sanitize_clean_string() {
        assert_eq!(sanitize("normal-target-name_123"), "normal-target-name_123");
        assert_eq!(sanitize("target with spaces"), "target with spaces");
        assert_eq!(sanitize("tårget-üñîçødé"), "tårget-üñîçødé");
    }

    #[test]
    fn test_sanitize_csi_escape_sequences() {
        // Colors / SGR
        assert_eq!(sanitize("\x1b[31mred-target\x1b[0m"), "red-target");
        assert_eq!(sanitize("\x1b[1;32;40mbold-green\x1b[0m"), "bold-green");
        // Screen clearing & cursor movement
        assert_eq!(sanitize("\x1b[2J\x1b[Hspoofed"), "spoofed");
        assert_eq!(sanitize("\x1b[2K\rline-clear"), "line-clear");
        // DEC private modes
        assert_eq!(sanitize("\x1b[?25lhidden-cursor\x1b[?25h"), "hidden-cursor");
    }

    #[test]
    fn test_sanitize_osc_escape_sequences() {
        // OSC terminated with BEL (\x07)
        assert_eq!(sanitize("\x1b]0;evil title\x07my-target"), "my-target");
        // OSC terminated with ST (ESC \)
        assert_eq!(sanitize("\x1b]0;evil title\x1b\\my-target"), "my-target");
    }

    #[test]
    fn test_sanitize_other_escape_sequences() {
        // 2-byte Fe sequences
        assert_eq!(sanitize("\x1bNsingle-shift"), "single-shift");
        // Character set designations
        assert_eq!(sanitize("\x1b(Bcharset-target"), "charset-target");
        // DCS / SOS / PM / APC sequences
        assert_eq!(sanitize("\x1bPdevice-control\x07target"), "target");
        assert_eq!(sanitize("\x1b_application-command\x1b\\target"), "target");
        // Incomplete / stray escape
        assert_eq!(sanitize("\x1b"), "");
        assert_eq!(sanitize("\x1b[31"), "");
    }

    #[test]
    fn test_sanitize_c1_control_codes() {
        // C1 CSI (\u{009b})
        assert_eq!(sanitize("\u{009b}31mred\u{009b}0m"), "red");
        // C1 OSC (\u{009d})
        assert_eq!(sanitize("\u{009d}0;title\x07target"), "target");
    }

    #[test]
    fn test_sanitize_control_characters() {
        // ASCII control characters: CR, LF, TAB, BEL, BS, NUL, DEL
        assert_eq!(sanitize("target\r\nspoofed"), "targetspoofed");
        assert_eq!(sanitize("target\x07bell"), "targetbell");
        assert_eq!(sanitize("target\x08backspace"), "targetbackspace");
        assert_eq!(sanitize("target\t\x00tab-null"), "targettab-null");
        assert_eq!(sanitize("target\x7fdel"), "targetdel");
    }

    #[test]
    fn test_tabular_formatter_sanitizes_metadata() {
        let mut target = make_valid_target();
        target.addresses = vec![make_ip_v4_addr(0)];
        target.nodename = Some("\x1b[31mevil-nodename\x1b[0m".to_string());
        target.serial_number = Some("\x1b[2Jevil-serial".to_string());
        target.product_config = Some("\x1b]0;evil\x07evil-product".to_string());
        target.board_config = Some("evil-board\r\n".to_string());

        let formatter = TabularTargetFormatter::from(vec![target]);
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 2);
        let target_line = &lines[1];
        assert!(target_line.contains("evil-nodename"));
        assert!(target_line.contains("evil-serial"));
        assert!(target_line.contains("evil-product.evil-board"));
        assert!(!target_line.contains("\x1b"));
        assert!(!target_line.contains('\r'));
        assert!(!target_line.contains('\n'));
        assert!(!target_line.contains('\x07'));
    }

    #[test]
    fn test_simple_formatter_sanitizes_metadata() {
        let mut target = make_valid_target();
        target.nodename = Some("\x1b[31mevil-nodename\x1b[0m".to_string());

        let formatter = SimpleTargetFormatter::try_from(vec![target]).unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("evil-nodename"));
        assert!(!lines[0].contains("\x1b"));
    }

    #[test]
    fn test_serials_formatter_sanitizes_metadata() {
        let mut target = make_valid_target();
        target.serial_number = Some("\x1b[32mevil-serial\x1b[0m".to_string());

        let formatter = SerialsTargetFormatter::try_from(vec![target]).unwrap();
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "evil-serial");
    }

    #[test]
    fn test_name_only_formatter_sanitizes_metadata() {
        let mut target = make_valid_target();
        target.nodename = Some("\x1b[33mevil-nodename\x1b[0m".to_string());

        let formatter = NameOnlyTargetFormatter::from(vec![target]);
        let lines = formatter.lines().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "evil-nodename");
    }

    #[test]
    fn test_json_formatter_sanitizes_metadata() {
        let mut target = make_valid_target();
        target.nodename = Some("\x1b[31mevil-nodename\x1b[0m".to_string());
        target.serial_number = Some("\x1b[2Jevil-serial".to_string());
        target.product_config = Some("\x1b]0;evil\x07evil-product".to_string());
        target.board_config = Some("evil-board\r\n".to_string());

        let formatter = JsonTargetFormatter::from(vec![target]);
        let json_target = &formatter.targets[0];
        assert_eq!(json_target.nodename, "evil-nodename");
        assert_eq!(json_target.serial, "evil-serial");
        assert_eq!(json_target.target_type, "evil-product.evil-board");
    }
}
