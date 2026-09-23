// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use either::Either;
use serde::{Deserialize, Deserializer};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};

use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;

use crate::DeviceClass;

const INTERFACE_PREFIX_WLAN: &str = "wlan";
const INTERFACE_PREFIX_ETHERNET: &str = "eth";
const INTERFACE_PREFIX_AP: &str = "ap";
const INTERFACE_PREFIX_BLACKHOLE: &str = "blackhole";

// Interfaces with different InterfaceNamingIdentifiers are expected to have
// different names.
//
// If two interfaces have the same MAC, they are expected to produce a different
// name in two possible ways: 1) have a different port class, or 2) use the
// NormalizedMac naming scheme (which avoids conflicts via retries).
#[derive(PartialEq, Eq, Debug, Clone, Hash)]
pub(crate) struct InterfaceNamingIdentifier {
    pub(crate) mac: fidl_fuchsia_net_ext::MacAddress,
    pub(crate) topological_path: String,
}

pub(crate) fn generate_identifier(
    mac_address: &fidl_fuchsia_net_ext::MacAddress,
    topological_path: &str,
) -> InterfaceNamingIdentifier {
    InterfaceNamingIdentifier { mac: *mac_address, topological_path: topological_path.to_string() }
}

// Get the NormalizedMac using the last octet of the MAC address. The offset
// modifies the last_byte in an attempt to avoid naming conflicts.
// For example, a MAC of `[0x1, 0x1, 0x1, 0x1, 0x1, 0x9]` with offset 0
// becomes `9`.
fn get_mac_identifier_from_octets(
    octets: &[u8; 6],
    interface_type: crate::InterfaceType,
    offset: u8,
) -> Result<u8, anyhow::Error> {
    if offset == u8::MAX {
        return Err(anyhow::format_err!(
            "could not find unique identifier for mac={:?}, interface_type={:?}",
            octets,
            interface_type
        ));
    }

    let last_byte = octets[octets.len() - 1];
    let (identifier, _) = last_byte.overflowing_add(offset);
    Ok(identifier)
}

// Get the normalized bus path for a topological path.
// For example, a PCI device at `02:00.1` becomes `02001`.
// At the time of writing, typical topological paths appear similar to:
//
// PCI:
// "/dev/sys/platform/pt/PCI0/bus/02:00.0/02:00.0/e1000/ethernet"
//
// USB over PCI:
// "/dev/sys/platform/pt/PCI0/bus/00:14.0/00:14.0/xhci/usb/007/ifc-000/<snip>/wlan/wlan-ethernet/ethernet"
// 00:14:0 following "/PCI0/bus/" represents BDF (Bus Device Function)
//
// USB over DWC:
// "/dev/sys/platform/05:00:18/usb-phy-composite/aml_usb_phy/dwc2/dwc2_phy/dwc2/usb-peripheral/function-000/usb-cdc-netdev/network-device"
// 05:00:18 following "platform" represents
// vid(vendor id):pid(product id):did(device id) and are defined in each board file
//
// SDIO
// "/dev/sys/platform/05:00:6/aml-sd-emmc/sdio/broadcom-wlanphy"
// 05:00:6 following "platform" represents
// vid(vendor id):pid(product id):did(device id) and are defined in each board file
//
// Ethernet Jack for VIM2
// "/dev/sys/platform/04:02:7/aml-ethernet/Designware-MAC/ethernet"
//
// VirtIo
// "/dev/sys/platform/pt/PC00/bus/00:1e.0/00_1e_0/virtio-net/network-device"
//
// Since there is no real standard for topological paths, when no bus path can be found,
// the function attempts to return one that is unlikely to conflict with any existing path
// by assuming a bus path of ff:ff:ff, and decrementing from there. This permits
// generating unique, well-formed names in cases where a matching path component can't be
// found, while also being relatively recognizable as exceptional.
fn get_normalized_bus_path_for_topo_path(topological_path: &str) -> String {
    static PATH_UNIQ_MARKER: AtomicU32 = AtomicU32::new(0xffffff);
    topological_path
        .split("/")
        .find(|pc| {
            pc.len() >= 7 && pc.chars().all(|c| c.is_digit(16) || c == ':' || c == '.' || c == '_')
        })
        .and_then(|s| {
            Some(s.replace(&[':', '.', '_'], "").trim_end_matches(|c| c == '0').to_string())
        })
        .unwrap_or_else(|| format!("{:01$x}", PATH_UNIQ_MARKER.fetch_sub(1, Ordering::SeqCst), 6))
}

#[derive(Debug)]
pub struct InterfaceNamingConfig {
    naming_rules: Vec<NamingRule>,
    interfaces: HashMap<InterfaceNamingIdentifier, String>,
}

impl InterfaceNamingConfig {
    pub(crate) fn from_naming_rules(naming_rules: Vec<NamingRule>) -> InterfaceNamingConfig {
        InterfaceNamingConfig { naming_rules, interfaces: HashMap::new() }
    }

    /// Returns a stable interface name for the specified interface.
    pub(crate) fn generate_stable_name(
        &mut self,
        topological_path: &str,
        mac: &fidl_fuchsia_net_ext::MacAddress,
        device_class: DeviceClass,
    ) -> Result<(&str, InterfaceNamingIdentifier), NameGenerationError> {
        let interface_naming_id = generate_identifier(mac, topological_path);
        let info = DeviceInfoRef { topological_path, mac, device_class };

        // Interfaces that are named using the NormalizedMac naming rule are
        // named to avoid MAC address final octet collisions. When a device
        // with the same identifier is re-installed, re-attempt name generation
        // since the MAC identifiers used may have changed.
        match self.interfaces.remove(&interface_naming_id) {
            Some(name) => log::info!(
                "{name} already existed for this identifier\
            {interface_naming_id:?}. inserting a new one."
            ),
            None => {
                // This interface naming id will have a new entry
            }
        }

        let generated_name = self.generate_name(&info)?;
        if let Some(name) =
            self.interfaces.insert(interface_naming_id.clone(), generated_name.clone())
        {
            log::error!(
                "{name} was unexpectedly found for {interface_naming_id:?} \
            when inserting a new name"
            );
        }

        // Need to grab a reference to appease the borrow checker.
        let generated_name = match self.interfaces.get(&interface_naming_id) {
            Some(name) => Ok(name),
            None => Err(NameGenerationError::GenerationError(anyhow::format_err!(
                "expected to see name {generated_name} present since it was just added"
            ))),
        }?;

        Ok((generated_name, interface_naming_id))
    }

    fn generate_name(&self, info: &DeviceInfoRef<'_>) -> Result<String, NameGenerationError> {
        generate_name_from_naming_rules(&self.naming_rules, &self.interfaces, &info)
    }
}

/// An error observed when generating a new name.
#[derive(Debug)]
pub enum NameGenerationError {
    GenerationError(anyhow::Error),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub enum BusType {
    PCI,
    SDIO,
    USB,
    Unknown,
    VirtIo,
}

impl BusType {
    // Retrieve the list of composition rules that comprise the default name
    // for the interface based on BusType.
    // Example names for the following default rules:
    // * USB device: "ethx5"
    // * PCI/SDIO device: "wlans5009"
    fn get_default_name_composition_rules(&self) -> Vec<NameCompositionRule> {
        match *self {
            BusType::USB | BusType::Unknown => vec![
                NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::DeviceClass },
                NameCompositionRule::Static { value: String::from("x") },
                NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::NormalizedMac },
            ],
            BusType::PCI | BusType::SDIO | BusType::VirtIo => vec![
                NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::DeviceClass },
                NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::BusType },
                NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::BusPath },
            ],
        }
    }
}

impl std::fmt::Display for BusType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match *self {
            Self::PCI => "p",
            Self::SDIO => "s",
            Self::USB => "u",
            Self::Unknown => "unk",
            Self::VirtIo => "v",
        };
        write!(f, "{}", name)
    }
}

// Extract the `BusType` for a device given the topological path.
fn get_bus_type_for_topological_path(topological_path: &str) -> BusType {
    let p = topological_path;

    if p.contains("/PCI0") {
        // A USB bus will require a bridge over a PCI controller, so a
        // topological path for a USB bus should contain strings to represent
        // PCI and USB.
        if p.contains("/usb/") {
            return BusType::USB;
        }
        return BusType::PCI;
    } else if p.contains("/usb-peripheral/") {
        // On VIM3 targets, the USB bus does not require a bridge over a PCI
        // controller, so the bus path represents the USB type with a
        // different string.
        return BusType::USB;
    } else if p.contains("/sdio/") {
        return BusType::SDIO;
    } else if p.contains("/virtio-net/") {
        return BusType::VirtIo;
    }

    BusType::Unknown
}

fn deserialize_glob_pattern<'de, D>(deserializer: D) -> Result<glob::Pattern, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = String::deserialize(deserializer)?;
    glob::Pattern::new(&buf).map_err(serde::de::Error::custom)
}

/// The matching rules available for a `NamingRule`.
#[derive(Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum MatchingRule {
    BusTypes(Vec<BusType>),
    // TODO(https://fxbug.dev/42085144): Use a lightweight regex crate with the basic
    // regex features to allow for more configurations than glob.
    #[serde(deserialize_with = "deserialize_glob_pattern")]
    TopologicalPath(glob::Pattern),
    DeviceClasses(Vec<DeviceClass>),
    // Signals whether this rule should match any interface.
    Any(bool),
}

/// The matching rules available for a `ProvisoningRule`.
#[derive(Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(untagged)]
pub enum ProvisioningMatchingRule {
    // TODO(github.com/serde-rs/serde/issues/912): Use `other` once it supports
    // deserializing into non-unit variants. `untagged` can only be applied
    // to the entire enum, so `interface_name` is used as a field to ensure
    // stability across configuration matching rules.
    InterfaceName {
        #[serde(rename = "interface_name", deserialize_with = "deserialize_glob_pattern")]
        pattern: glob::Pattern,
    },
    Common(MatchingRule),
}

impl MatchingRule {
    fn does_interface_match(&self, info: &DeviceInfoRef<'_>) -> Result<bool, anyhow::Error> {
        match &self {
            MatchingRule::BusTypes(type_list) => {
                // Match the interface if the interface under comparison
                // matches any of the types included in the list.
                let bus_type = get_bus_type_for_topological_path(info.topological_path);
                Ok(type_list.contains(&bus_type))
            }
            MatchingRule::TopologicalPath(pattern) => {
                // Match the interface if the provided pattern finds any
                // matches in the interface under comparison's
                // topological path.
                Ok(pattern.matches(info.topological_path))
            }
            MatchingRule::DeviceClasses(class_list) => {
                // Match the interface if the interface under comparison
                // matches any of the types included in the list.
                Ok(class_list.contains(&info.device_class))
            }
            MatchingRule::Any(matches_any_interface) => Ok(*matches_any_interface),
        }
    }
}

impl ProvisioningMatchingRule {
    fn does_interface_match(
        &self,
        info: &DeviceInfoRef<'_>,
        interface_name: &str,
    ) -> Result<bool, anyhow::Error> {
        match &self {
            ProvisioningMatchingRule::InterfaceName { pattern } => {
                // Match the interface if the provided pattern finds any
                // matches in the interface under comparison's name.
                Ok(pattern.matches(interface_name))
            }
            ProvisioningMatchingRule::Common(matching_rule) => {
                // Handle the other `MatchingRule`s the same as the naming
                // policy matchers.
                matching_rule.does_interface_match(info)
            }
        }
    }
}

// TODO(https://fxbug.dev/42084785): Create dynamic naming rules
// A naming rule that uses device information to produce a component of
// the interface's name.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum DynamicNameCompositionRule {
    BusPath,
    BusType,
    DeviceClass,
    // A unique value seeded by the final octet of the interface's MAC address.
    NormalizedMac,
}

impl DynamicNameCompositionRule {
    // `true` when a rule can be re-tried to produce a different name.
    fn supports_retry(&self) -> bool {
        match *self {
            DynamicNameCompositionRule::BusPath
            | DynamicNameCompositionRule::BusType
            | DynamicNameCompositionRule::DeviceClass => false,
            DynamicNameCompositionRule::NormalizedMac => true,
        }
    }

    fn get_name(&self, info: &DeviceInfoRef<'_>, attempt_num: u8) -> Result<String, anyhow::Error> {
        Ok(match *self {
            DynamicNameCompositionRule::BusPath => {
                get_normalized_bus_path_for_topo_path(info.topological_path)
            }
            DynamicNameCompositionRule::BusType => {
                get_bus_type_for_topological_path(info.topological_path).to_string()
            }
            DynamicNameCompositionRule::DeviceClass => match info.device_class.into() {
                crate::InterfaceType::WlanClient => INTERFACE_PREFIX_WLAN,
                crate::InterfaceType::Ethernet => INTERFACE_PREFIX_ETHERNET,
                // Lowpan interfaces are not managed or installed by Netcfg.
                crate::InterfaceType::Lowpan => {
                    unreachable!("unexpected to install lowpan interface")
                }
                crate::InterfaceType::WlanAp => INTERFACE_PREFIX_AP,
                crate::InterfaceType::Blackhole => INTERFACE_PREFIX_BLACKHOLE,
            }
            .to_string(),
            DynamicNameCompositionRule::NormalizedMac => {
                let fidl_fuchsia_net_ext::MacAddress { octets } = info.mac;
                let mac_identifier =
                    get_mac_identifier_from_octets(octets, info.device_class.into(), attempt_num)?;
                format!("{mac_identifier:x}")
            }
        })
    }
}

// A rule that dictates a component of an interface's name. An interface's name
// is determined by extracting the name of each rule, in order, and
// concatenating the results.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "lowercase", tag = "type")]
pub enum NameCompositionRule {
    Static { value: String },
    Dynamic { rule: DynamicNameCompositionRule },
    // The default name composition rules based on the device's BusType.
    // Defined in `BusType::get_default_name_composition_rules`.
    Default,
}

/// A rule that dictates how interfaces that align with the property matching
/// rules should be named.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub struct NamingRule {
    /// A set of rules to check against an interface's properties. All rules
    /// must apply for the naming scheme to take effect.
    pub matchers: HashSet<MatchingRule>,
    /// The rules to apply to the interface to produce the interface's name.
    pub naming_scheme: Vec<NameCompositionRule>,
}

impl NamingRule {
    // An interface's name is determined by extracting the name of each rule,
    // in order, and concatenating the results. Returns an error if the
    // interface name cannot be generated.
    fn generate_name(
        &self,
        interfaces: &HashMap<InterfaceNamingIdentifier, String>,
        info: &DeviceInfoRef<'_>,
    ) -> Result<String, NameGenerationError> {
        // When a bus type cannot be found for a path, use the USB
        // default naming policy which uses a MAC address.
        let bus_type = get_bus_type_for_topological_path(&info.topological_path);

        // Expand any `Default` rules into the `Static` and `Dynamic` rules in a single vector.
        // If this was being consumed once, we could avoid the call to `collect`. However, since we
        // want to use it twice, we need to convert it to a form where the items can be itererated
        // over without consuming them.
        let expanded_rules = self
            .naming_scheme
            .iter()
            .map(|rule| {
                if let NameCompositionRule::Default = rule {
                    Either::Right(bus_type.get_default_name_composition_rules().into_iter())
                } else {
                    Either::Left(std::iter::once(rule.clone()))
                }
            })
            .flatten()
            .collect::<Vec<_>>();

        // Determine whether any rules present support retrying for a unique name.
        let should_reattempt_on_conflict = expanded_rules.iter().any(|rule| {
            if let NameCompositionRule::Dynamic { rule } = rule {
                rule.supports_retry()
            } else {
                false
            }
        });

        let mut attempt_num = 0u8;
        loop {
            let name = expanded_rules
                .iter()
                .map(|rule| match rule {
                    NameCompositionRule::Static { value } => Ok(value.clone()),
                    // Dynamic rules require the knowledge of `DeviceInfo` properties.
                    NameCompositionRule::Dynamic { rule } => rule
                        .get_name(info, attempt_num)
                        .map_err(NameGenerationError::GenerationError),
                    NameCompositionRule::Default => {
                        unreachable!(
                            "Default naming rules should have been pre-expanded. \
                             Nested default rules are not supported."
                        );
                    }
                })
                .collect::<Result<String, NameGenerationError>>()?;

            if interfaces.values().any(|existing_name| existing_name == &name) {
                if should_reattempt_on_conflict {
                    attempt_num += 1;
                    // Try to generate another name with the modified attempt number.
                    continue;
                }

                log::warn!(
                    "name ({name}) already used for an interface installed by netcfg. \
                 using name since it is possible that the interface using this name is no \
                 longer active"
                );
            }
            return Ok(name);
        }
    }

    // An interface must align with all specified `MatchingRule`s.
    fn does_interface_match(&self, info: &DeviceInfoRef<'_>) -> bool {
        self.matchers.iter().all(|rule| rule.does_interface_match(info).unwrap_or_default())
    }
}

// Find the first `NamingRule` that matches the device and attempt to
// construct a name from the provided `NameCompositionRule`s.
fn generate_name_from_naming_rules(
    naming_rules: &[NamingRule],
    interfaces: &HashMap<InterfaceNamingIdentifier, String>,
    info: &DeviceInfoRef<'_>,
) -> Result<String, NameGenerationError> {
    // TODO(https://fxbug.dev/42086002): Consider adding an option to the rules to allow
    // fallback rules when name generation fails.
    // Use the first naming rule that matches the interface to enforce consistent
    // interface names, even if there are other matching rules.
    let fallback_rule = fallback_naming_rule();
    let first_matching_rule =
        naming_rules.iter().find(|rule| rule.does_interface_match(&info)).unwrap_or(
            // When there are no `NamingRule`s that match the device,
            // use a fallback rule that has the Default naming scheme.
            &fallback_rule,
        );

    first_matching_rule.generate_name(interfaces, &info)
}

// Matches any device and uses the default naming rule.
fn fallback_naming_rule() -> NamingRule {
    NamingRule {
        matchers: HashSet::from([MatchingRule::Any(true)]),
        naming_scheme: vec![NameCompositionRule::Default],
    }
}

/// The provision action to take if the matchers are satisfied.
#[derive(Copy, Clone, Debug, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub struct ProvisioningAction {
    /// The type of the provisioning.
    pub provisioning: ProvisioningType,
    /// Where the netstack managed routes should be installed.
    pub netstack_managed_routes_designation: Option<NetstackManagedRoutesDesignation>,
}

/// Whether the interface should be provisioned locally by netcfg, or
/// delegated. Provisioning is the set of events that occurs after
/// interface enumeration, such as starting a DHCP client and assigning
/// an IP to the interface. Provisioning actions work to support
/// Internet connectivity.
#[derive(Copy, Clone, Debug, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub enum ProvisioningType {
    /// Netcfg will provision the interface
    #[default]
    Local,
    /// Netcfg will not provision the interface. The provisioning
    /// of the interface will occur elsewhere
    Delegated,
}

impl ProvisioningType {
    /// Whether the interface should be tracked in the network registry.
    ///
    /// Locally provisioned interfaces are tracked in the network registry because their
    /// lifetime is managed by netcfg. Delegated interfaces are not tracked in the network
    /// registry because their presence is communicated by the network-socket-proxy service.
    pub fn track_in_network_registry(&self) -> bool {
        match self {
            ProvisioningType::Local => true,
            ProvisioningType::Delegated => false,
        }
    }
}

/// Where the netstack managed routes should be stored.
///
/// Mirrors [`fnet_interfaces_admin::NetstackManagedRoutesDesignation`].
#[derive(Copy, Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum NetstackManagedRoutesDesignation {
    Main,
    InterfaceLocal,
}

impl From<NetstackManagedRoutesDesignation>
    for fnet_interfaces_admin::NetstackManagedRoutesDesignation
{
    fn from(value: NetstackManagedRoutesDesignation) -> Self {
        match value {
            NetstackManagedRoutesDesignation::Main => Self::Main(fnet_interfaces_admin::Empty),
            NetstackManagedRoutesDesignation::InterfaceLocal => {
                Self::InterfaceLocal(fnet_interfaces_admin::Empty)
            }
        }
    }
}

/// A rule that dictates how interfaces that align with the property matching
/// rules should be provisioned.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub struct ProvisioningRule {
    /// A set of rules to check against an interface's properties. All rules
    /// must apply for the provisioning action to take effect.
    pub matchers: HashSet<ProvisioningMatchingRule>,
    /// The provisioning policy that netcfg applies to a matching
    /// interface.
    #[serde(flatten)]
    pub action: ProvisioningAction,
}

// A ref version of `devices::DeviceInfo` to avoid the need to clone data
// unnecessarily. Devices without MAC are not supported yet, see
// `add_new_device` in `lib.rs`. This makes mac into a required field for
// ease of use.
pub(super) struct DeviceInfoRef<'a> {
    pub(super) device_class: DeviceClass,
    pub(super) mac: &'a fidl_fuchsia_net_ext::MacAddress,
    pub(super) topological_path: &'a str,
}

impl<'a> DeviceInfoRef<'a> {
    pub(super) fn interface_type(&self) -> crate::InterfaceType {
        let DeviceInfoRef { device_class, mac: _, topological_path: _ } = self;
        (*device_class).into()
    }

    pub(super) fn is_wlan_ap(&self) -> bool {
        let DeviceInfoRef { device_class, mac: _, topological_path: _ } = self;
        match device_class {
            DeviceClass::WlanAp => true,
            DeviceClass::WlanClient
            | DeviceClass::Virtual
            | DeviceClass::Ethernet
            | DeviceClass::Bridge
            | DeviceClass::Ppp
            | DeviceClass::Lowpan
            | DeviceClass::Blackhole => false,
        }
    }
}

impl ProvisioningRule {
    // An interface must align with all specified `MatchingRule`s.
    fn does_interface_match(&self, info: &DeviceInfoRef<'_>, interface_name: &str) -> bool {
        self.matchers
            .iter()
            .all(|rule| rule.does_interface_match(info, interface_name).unwrap_or_default())
    }
}

// Find the first `ProvisioningRule` that matches the device and get
// the associated `ProvisioningAction`. By default, use Local provisioning
// so that Netcfg will provision interfaces unless configuration
// indicates otherwise.
pub(crate) fn find_provisioning_action_from_provisioning_rules(
    provisioning_rules: &[ProvisioningRule],
    info: &DeviceInfoRef<'_>,
    interface_name: &str,
) -> ProvisioningAction {
    provisioning_rules
        .iter()
        .find_map(|rule| {
            if rule.does_interface_match(&info, &interface_name) { Some(rule.action) } else { None }
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use test_case::test_case;

    // This is a lossy conversion between `InterfaceType` and `DeviceClass`
    // that allows tests to use a `devices::DeviceInfo` struct instead of
    // handling the fields individually.
    fn device_class_from_interface_type(ty: crate::InterfaceType) -> DeviceClass {
        match ty {
            crate::InterfaceType::Ethernet => DeviceClass::Ethernet,
            crate::InterfaceType::Lowpan => DeviceClass::Lowpan,
            crate::InterfaceType::WlanClient => DeviceClass::WlanClient,
            crate::InterfaceType::WlanAp => DeviceClass::WlanAp,
            crate::InterfaceType::Blackhole => DeviceClass::Blackhole,
        }
    }

    // usb interfaces
    #[test_case(
        "/dev/sys/platform/pt/PCI0/bus/00:14.0/00:14.0/xhci/usb/004/004/ifc-000/ax88179/ethernet",
        [0x01, 0x01, 0x01, 0x01, 0x01, 0x01],
        crate::InterfaceType::WlanClient,
        "wlanx1";
        "usb_wlan"
    )]
    #[test_case(
        "/dev/sys/platform/pt/PCI0/bus/00:15.0/00:15.0/xhci/usb/004/004/ifc-000/ax88179/ethernet",
        [0x02, 0x02, 0x02, 0x02, 0x02, 0x02],
        crate::InterfaceType::Ethernet,
        "ethx2";
        "usb_eth"
    )]
    // pci interfaces
    #[test_case(
        "/dev/sys/platform/pt/PCI0/bus/00:14.0/00:14.0/ethernet",
        [0x03, 0x03, 0x03, 0x03, 0x03, 0x03],
        crate::InterfaceType::WlanClient,
        "wlanp0014";
        "pci_wlan"
    )]
    #[test_case(
        "/dev/sys/platform/pt/PCI0/bus/00:15.0/00:14.0/ethernet",
        [0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        crate::InterfaceType::Ethernet,
        "ethp0015";
        "pci_eth"
    )]
    // platform interfaces (ethernet jack and sdio devices)
    #[test_case(
        "/dev/sys/platform/05:00:6/aml-sd-emmc/sdio/broadcom-wlanphy",
        [0x05, 0x05, 0x05, 0x05, 0x05, 0x05],
        crate::InterfaceType::WlanClient,
        "wlans05006";
        "platform_wlan"
    )]
    #[test_case(
        "/dev/sys/platform/04:02:7/aml-ethernet/Designware-MAC/ethernet",
        [0x07, 0x07, 0x07, 0x07, 0x07, 0x07],
        crate::InterfaceType::Ethernet,
        "ethx7";
        "platform_eth"
    )]
    // unknown interfaces
    #[test_case(
        "/dev/sys/unknown",
        [0x08, 0x08, 0x08, 0x08, 0x08, 0x08],
        crate::InterfaceType::WlanClient,
        "wlanx8";
        "unknown_wlan1"
    )]
    #[test_case(
        "unknown",
        [0x09, 0x09, 0x09, 0x09, 0x09, 0x09],
        crate::InterfaceType::WlanClient,
        "wlanx9";
        "unknown_wlan2"
    )]
    #[test_case(
        "unknown",
        [0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a],
        crate::InterfaceType::WlanAp,
        "apxa";
        "unknown_ap"
    )]
    #[test_case(
        "/dev/sys/platform/pt/PC00/bus/00:1e.0/00_1e_0/virtio-net/network-device",
        [0x0b, 0x0b, 0x0b, 0x0b, 0x0b, 0x0b],
        crate::InterfaceType::Ethernet,
        "ethv001e";
        "virtio_attached_ethernet"
    )]
    // NB: name generation for blackhole interfaces is never expected to be invoked.
    #[test_case(
        "/dev/sys/platform/pt/PCI0/bus/00:15.0/00:15.0/xhci/usb/004/004/ifc-000/ax88179/ethernet",
        [0x0c, 0x0c, 0x0c, 0x0c, 0x0c, 0x0c],
        crate::InterfaceType::Blackhole,
        "blackholexc";
        "usb_blackhole")]
    fn test_generate_name(
        topological_path: &'static str,
        mac: [u8; 6],
        interface_type: crate::InterfaceType,
        want_name: &'static str,
    ) {
        let interface_naming_config = InterfaceNamingConfig::from_naming_rules(vec![]);
        let name = interface_naming_config
            .generate_name(&DeviceInfoRef {
                device_class: device_class_from_interface_type(interface_type),
                mac: &fidl_fuchsia_net_ext::MacAddress { octets: mac },
                topological_path,
            })
            .expect("failed to generate the name");
        assert_eq!(name, want_name);
    }

    struct StableNameTestCase {
        topological_path: &'static str,
        mac: [u8; 6],
        interface_type: crate::InterfaceType,
        want_name: &'static str,
        expected_size: usize,
    }

    // Base case. Interface should be added to config.
    #[test_case([StableNameTestCase {
        topological_path: "/dev/sys/platform/pt/PCI0/bus/00:14.0_/00:14.0/ethernet",
        mac: [0x01, 0x01, 0x01, 0x01, 0x01, 0x01],
        interface_type: crate::InterfaceType::WlanClient,
        want_name: "wlanp0014",
        expected_size: 1 }];
        "single_interface"
    )]
    // Test case that shares the same topo path and different MAC, but same
    // last octet. Expect to see second interface added with different name.
    #[test_case([StableNameTestCase {
        topological_path: "/dev/sys/platform/pt/PCI0/bus/00:14.0_/00:14.0/ethernet",
        mac: [0x01, 0x01, 0x01, 0x01, 0x01, 0x01],
        interface_type: crate::InterfaceType::WlanClient,
        want_name: "wlanp0014",
        expected_size: 1}, StableNameTestCase {
        topological_path: "/dev/sys/platform/pt/PCI0/bus/00:14.0_/00:14.0/ethernet",
        mac: [0xFE, 0x01, 0x01, 0x01, 0x01, 0x01],
        interface_type: crate::InterfaceType::WlanAp,
        want_name: "app0014",
        expected_size: 2 }];
        "two_interfaces_same_topo_path_different_mac"
    )]
    #[test_case([StableNameTestCase {
        topological_path: "/dev/sys/platform/pt/PCI0/bus/00:14.0_/00:14.0/ethernet",
        mac: [0x01, 0x01, 0x01, 0x01, 0x01, 0x01],
        interface_type: crate::InterfaceType::WlanClient,
        want_name: "wlanp0014",
        expected_size: 1}, StableNameTestCase {
        topological_path: "/dev/sys/platform/pt/PCI0/bus/01:00.0/01:00.0/iwlwifi-wlan-softmac/wlan-ethernet/ethernet",
        mac: [0xFE, 0x01, 0x01, 0x01, 0x01, 0x01],
        interface_type: crate::InterfaceType::Ethernet,
        want_name: "ethp01",
        expected_size: 2 }];
        "two_distinct_interfaces"
    )]
    // Test case that labels iwilwifi as ethernet, then changes the device
    // class to wlan. The test should detect that the device class doesn't
    // match the interface name, and overwrite with the new interface name
    // that does match.
    #[test_case([StableNameTestCase {
        topological_path: "/dev/sys/platform/pt/PCI0/bus/01:00.0/01:00.0/iwlwifi-wlan-softmac/wlan-ethernet/ethernet",
        mac: [0x01, 0x01, 0x01, 0x01, 0x01, 0x01],
        interface_type: crate::InterfaceType::Ethernet,
        want_name: "ethp01",
        expected_size: 1 }, StableNameTestCase {
        topological_path: "/dev/sys/platform/pt/PCI0/bus/01:00.0/01:00.0/iwlwifi-wlan-softmac/wlan-ethernet/ethernet",
        mac: [0x01, 0x01, 0x01, 0x01, 0x01, 0x01],
        interface_type: crate::InterfaceType::WlanClient,
        want_name: "wlanp01",
        expected_size: 1 }];
        "two_interfaces_different_device_class"
    )]
    fn test_generate_stable_name(test_cases: impl IntoIterator<Item = StableNameTestCase>) {
        let mut interface_naming_config = InterfaceNamingConfig::from_naming_rules(vec![]);

        // query an existing interface with the same topo path and a different mac address
        for (
            _i,
            StableNameTestCase { topological_path, mac, interface_type, want_name, expected_size },
        ) in test_cases.into_iter().enumerate()
        {
            let (name, _identifier) = interface_naming_config
                .generate_stable_name(
                    topological_path,
                    &fidl_fuchsia_net_ext::MacAddress { octets: mac },
                    device_class_from_interface_type(interface_type),
                )
                .expect("failed to get the interface name");
            assert_eq!(name, want_name);
            // Ensure the number of interfaces we expect are present.
            assert_eq!(interface_naming_config.interfaces.len(), expected_size);
        }
    }

    #[test]
    fn test_get_usb_255() {
        let topo_usb = "/dev/pci-00:14.0-fidl/xhci/usb/004/004/ifc-000/ax88179/ethernet";

        // test cases for 256 usb interfaces
        let mut config = InterfaceNamingConfig::from_naming_rules(vec![]);
        for n in 0u8..255u8 {
            let octets = [n, 0x01, 0x01, 0x01, 0x01, 00];

            let interface_naming_id =
                generate_identifier(&fidl_fuchsia_net_ext::MacAddress { octets }, topo_usb);

            let name = config
                .generate_name(&DeviceInfoRef {
                    device_class: device_class_from_interface_type(
                        crate::InterfaceType::WlanClient,
                    ),
                    mac: &fidl_fuchsia_net_ext::MacAddress { octets },
                    topological_path: topo_usb,
                })
                .expect("failed to generate the name");
            assert_eq!(name, format!("{}{:x}", "wlanx", n));
            assert_matches!(config.interfaces.insert(interface_naming_id, name), None);
        }

        let octets = [0x00, 0x00, 0x01, 0x01, 0x01, 00];
        assert!(
            config
                .generate_name(&DeviceInfoRef {
                    device_class: device_class_from_interface_type(
                        crate::InterfaceType::WlanClient
                    ),
                    mac: &fidl_fuchsia_net_ext::MacAddress { octets },
                    topological_path: topo_usb
                },)
                .is_err()
        );
    }

    #[test]
    fn test_get_usb_255_with_naming_rule() {
        let topo_usb = "/dev/pci-00:14.0-fidl/xhci/usb/004/004/ifc-000/ax88179/ethernet";

        let naming_rule = NamingRule {
            matchers: HashSet::new(),
            naming_scheme: vec![
                NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::NormalizedMac },
                NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::NormalizedMac },
            ],
        };

        // test cases for 256 usb interfaces
        let mut config = InterfaceNamingConfig::from_naming_rules(vec![naming_rule]);
        for n in 0u8..255u8 {
            let octets = [n, 0x01, 0x01, 0x01, 0x01, 00];
            let interface_naming_id =
                generate_identifier(&fidl_fuchsia_net_ext::MacAddress { octets }, topo_usb);

            let info = DeviceInfoRef {
                device_class: DeviceClass::Ethernet,
                mac: &fidl_fuchsia_net_ext::MacAddress { octets },
                topological_path: topo_usb,
            };

            let name = config.generate_name(&info).expect("failed to generate the name");
            // With only NormalizedMac as a NameCompositionRule, the name
            // should simply be the NormalizedMac itself.
            assert_eq!(name, format!("{n:x}{n:x}"));

            assert_matches!(config.interfaces.insert(interface_naming_id, name), None);
        }

        let octets = [0x00, 0x00, 0x01, 0x01, 0x01, 00];
        assert!(
            config
                .generate_name(&DeviceInfoRef {
                    device_class: DeviceClass::Ethernet,
                    mac: &fidl_fuchsia_net_ext::MacAddress { octets },
                    topological_path: topo_usb
                })
                .is_err()
        );
    }

    // Arbitrary values for devices::DeviceInfo for cases where DeviceInfo has
    // no impact on the test.
    fn default_device_info() -> DeviceInfoRef<'static> {
        DeviceInfoRef {
            device_class: DeviceClass::Ethernet,
            mac: &fidl_fuchsia_net_ext::MacAddress { octets: [0x1, 0x1, 0x1, 0x1, 0x1, 0x1] },
            topological_path: "",
        }
    }

    #[test_case(
        "/dev/sys/platform/pt/PCI0/bus/00:14.0_/00:14.0/ethernet",
        vec![BusType::PCI],
        BusType::PCI,
        true,
        "0014";
        "pci_match"
    )]
    #[test_case(
        "/dev/sys/platform/pt/PCI0/bus/00:14.0_/00:14.0/ethernet",
        vec![BusType::USB, BusType::SDIO],
        BusType::PCI,
        false,
        "0014";
        "pci_no_match"
    )]
    #[test_case(
        "/dev/sys/platform/pt/PCI0/bus/00:14.0/00:14.0/xhci/usb/004/004/ifc-000/ax88179/ethernet",
        vec![BusType::USB],
        BusType::USB,
        true,
        "0014";
        "pci_usb_match"
    )]
    #[test_case(
        "/dev/sys/platform/05:00:18/usb-phy-composite/aml_usb_phy/dwc2/dwc2_phy/dwc2/usb-peripheral/function-000/usb-cdc-netdev/network-device",
        vec![BusType::USB],
        BusType::USB,
        true,
        "050018";
        "dwc_usb_match"
    )]
    // Same topological path as the case for USB, but with
    // non-matching bus types. Ensure that even though PCI is
    // present in the topological path, it does not match a PCI
    // controller.
    #[test_case(
        "/dev/sys/platform/pt/PCI0/bus/00:14.0/00:14.0/xhci/usb/004/004/ifc-000/ax88179/ethernet",
        vec![BusType::PCI, BusType::SDIO],
        BusType::USB,
        false,
        "0014";
        "usb_no_match"
    )]
    #[test_case(
        "/dev/sys/platform/05:00:6/aml-sd-emmc/sdio/broadcom-wlanphy",
        vec![BusType::SDIO],
        BusType::SDIO,
        true,
        "05006";
        "sdio_match"
    )]
    #[test_case(
        "/dev/sys/platform/pt/PC00/bus/00:1e.0/00_1e_0/virtio-net/network-device",
        vec![BusType::VirtIo],
        BusType::VirtIo,
        true,
        "001e";
        "virtio_match_alternate_location"
    )]
    #[test_case(
        "/dev/sys/platform/pt/PC00/bus/<malformed>/00_1e_0/virtio-net/network-device",
        vec![BusType::VirtIo],
        BusType::VirtIo,
        true,
        "001e";
        "virtio_matches_underscore_path"
    )]
    #[test_case(
        "/dev/sys/platform/pt/PC00/bus/00:1e.1/00_1e_1/virtio-net/network-device",
        vec![BusType::VirtIo],
        BusType::VirtIo,
        true,
        "001e1";
        "virtio_match_alternate_no_trim"
    )]
    #[test_case(
        "/dev/sys/platform/pt/PC00/bus/<unrecognized_bus_path>/network-device",
        vec![BusType::Unknown],
        BusType::Unknown,
        true,
        "ffffff";
        "unknown_bus_match_unrecognized"
    )]
    fn test_interface_matching_and_naming_by_bus_properties(
        topological_path: &'static str,
        bus_types: Vec<BusType>,
        expected_bus_type: BusType,
        want_match: bool,
        want_name: &'static str,
    ) {
        let device_info = DeviceInfoRef {
            topological_path: topological_path,
            // `device_class` and `mac` have no effect on `BusType`
            // matching, so we use arbitrary values.
            ..default_device_info()
        };

        // Verify the `BusType` determined from the device's
        // topological path.
        let bus_type = get_bus_type_for_topological_path(&device_info.topological_path);
        assert_eq!(bus_type, expected_bus_type);

        // Create a matching rule for the provided `BusType` list.
        let matching_rule = MatchingRule::BusTypes(bus_types);
        let does_interface_match = matching_rule.does_interface_match(&device_info).unwrap();
        assert_eq!(does_interface_match, want_match);

        let name = get_normalized_bus_path_for_topo_path(&device_info.topological_path);
        assert_eq!(name, want_name);

        // Ensure that calling again will decrement this. It's unfortunate to need to encode this
        // in the test itself, but each test runs separately, so we can't rely on static storage
        // between test invocations.
        if want_name == "ffffff" {
            let name = get_normalized_bus_path_for_topo_path(&device_info.topological_path);
            assert_eq!(name, "fffffe");
        }
    }

    // Glob matches the number pattern of XX:XX in the path.
    #[test_case(
        "/dev/sys/platform/pt/PCI0/bus/00:14.0_/00:14.0/ethernet",
        r"*[0-9][0-9]:[0-9][0-9]*",
        true;
        "pattern_matches"
    )]
    #[test_case("pattern/will/match/anything", r"*", true; "pattern_matches_any")]
    // Glob checks for '00' after the colon but it will not find it.
    #[test_case(
        "/dev/sys/platform/pt/PCI0/bus/00:14.0_/00:14.0/ethernet",
        r"*[0-9][0-9]:00*",
        false;
        "no_matches"
    )]
    fn test_interface_matching_by_topological_path(
        topological_path: &'static str,
        glob_str: &'static str,
        want_match: bool,
    ) {
        let device_info = DeviceInfoRef {
            topological_path,
            // `device_class` and `mac` have no effect on `TopologicalPath`
            // matching, so we use arbitrary values.
            ..default_device_info()
        };

        // Create a matching rule for the provided glob expression.
        let matching_rule = MatchingRule::TopologicalPath(glob::Pattern::new(glob_str).unwrap());
        let does_interface_match = matching_rule.does_interface_match(&device_info).unwrap();
        assert_eq!(does_interface_match, want_match);
    }

    // Glob matches the default naming by MAC address.
    #[test_case(
        "ethx5",
        r"ethx[0-9]*",
        true;
        "pattern_matches"
    )]
    #[test_case("arbitraryname", r"*", true; "pattern_matches_any")]
    // Glob matches default naming by SDIO + bus path.
    #[test_case(
        "wlans1002",
        r"eths[0-9][0-9][0-9][0-9]*",
        false;
        "no_matches"
    )]
    fn test_interface_matching_by_interface_name(
        interface_name: &'static str,
        glob_str: &'static str,
        want_match: bool,
    ) {
        // Create a matching rule for the provided glob expression.
        let provisioning_matching_rule = ProvisioningMatchingRule::InterfaceName {
            pattern: glob::Pattern::new(glob_str).unwrap(),
        };
        let does_interface_match = provisioning_matching_rule
            .does_interface_match(&default_device_info(), interface_name)
            .unwrap();
        assert_eq!(does_interface_match, want_match);
    }

    #[test_case(
        DeviceClass::Ethernet,
        vec![DeviceClass::Ethernet],
        true;
        "eth_match"
    )]
    #[test_case(
        DeviceClass::Ethernet,
        vec![DeviceClass::WlanClient, DeviceClass::WlanAp],
        false;
        "eth_no_match"
    )]
    #[test_case(
        DeviceClass::WlanClient,
        vec![DeviceClass::WlanClient],
        true;
        "wlan_match"
    )]
    #[test_case(
        DeviceClass::WlanClient,
        vec![DeviceClass::Ethernet, DeviceClass::WlanAp],
        false;
        "wlan_no_match"
    )]
    #[test_case(
        DeviceClass::WlanAp,
        vec![DeviceClass::WlanAp],
        true;
        "ap_match"
    )]
    #[test_case(
        DeviceClass::WlanAp,
        vec![DeviceClass::Ethernet, DeviceClass::WlanClient],
        false;
        "ap_no_match"
    )]
    fn test_interface_matching_by_device_class(
        device_class: DeviceClass,
        device_classes: Vec<DeviceClass>,
        want_match: bool,
    ) {
        let device_info = DeviceInfoRef { device_class, ..default_device_info() };

        // Create a matching rule for the provided `DeviceClass` list.
        let matching_rule = MatchingRule::DeviceClasses(device_classes);
        let does_interface_match = matching_rule.does_interface_match(&device_info).unwrap();
        assert_eq!(does_interface_match, want_match);
    }

    // The device information should not have any impact on whether the
    // interface matches, but we use Ethernet and Wlan as base cases
    // to ensure that all interfaces are accepted or all interfaces
    // are rejected.
    #[test_case(
        DeviceClass::Ethernet,
        "/dev/pci-00:15.0-fidl/xhci/usb/004/004/ifc-000/ax88179/ethernet"
    )]
    #[test_case(DeviceClass::WlanClient, "/dev/pci-00:14.0/ethernet")]
    fn test_interface_matching_by_any_matching_rule(
        device_class: DeviceClass,
        topological_path: &'static str,
    ) {
        let device_info = DeviceInfoRef {
            device_class,
            mac: &fidl_fuchsia_net_ext::MacAddress { octets: [0x1, 0x1, 0x1, 0x1, 0x1, 0x1] },
            topological_path,
        };

        // Create a matching rule that should match any interface.
        let matching_rule = MatchingRule::Any(true);
        let does_interface_match = matching_rule.does_interface_match(&device_info).unwrap();
        assert!(does_interface_match);

        // Create a matching rule that should reject any interface.
        let matching_rule = MatchingRule::Any(false);
        let does_interface_match = matching_rule.does_interface_match(&device_info).unwrap();
        assert!(!does_interface_match);
    }

    #[test_case(
        DeviceInfoRef { device_class: DeviceClass::Ethernet, ..default_device_info() },
        vec![MatchingRule::DeviceClasses(vec![DeviceClass::WlanClient])],
        false;
        "false_single_rule"
    )]
    #[test_case(
        DeviceInfoRef { device_class: DeviceClass::Ethernet, ..default_device_info() },
        vec![MatchingRule::DeviceClasses(vec![DeviceClass::WlanClient]), MatchingRule::Any(true)],
        false;
        "false_one_rule_of_multiple"
    )]
    #[test_case(
        DeviceInfoRef { device_class: DeviceClass::Ethernet, ..default_device_info() },
        vec![MatchingRule::Any(true)],
        true;
        "true_single_rule"
    )]
    #[test_case(
        DeviceInfoRef { device_class: DeviceClass::Ethernet, ..default_device_info() },
        vec![MatchingRule::DeviceClasses(vec![DeviceClass::Ethernet]), MatchingRule::Any(true)],
        true;
        "true_multiple_rules"
    )]
    fn test_does_interface_match(
        info: DeviceInfoRef<'_>,
        matching_rules: Vec<MatchingRule>,
        want_match: bool,
    ) {
        let naming_rule =
            NamingRule { matchers: HashSet::from_iter(matching_rules), naming_scheme: Vec::new() };
        assert_eq!(naming_rule.does_interface_match(&info), want_match);
    }

    #[test_case(
        DeviceInfoRef { device_class: DeviceClass::Ethernet, ..default_device_info() },
        "",
        vec![
            ProvisioningMatchingRule::Common(
                MatchingRule::DeviceClasses(vec![DeviceClass::WlanClient])
            )
        ],
        false;
        "false_single_rule"
    )]
    #[test_case(
        DeviceInfoRef { device_class: DeviceClass::WlanClient, ..default_device_info() },
        "wlanx5009",
        vec![
            ProvisioningMatchingRule::InterfaceName {
                pattern: glob::Pattern::new("ethx*").unwrap()
            },
            ProvisioningMatchingRule::Common(MatchingRule::Any(true))
        ],
        false;
        "false_one_rule_of_multiple"
    )]
    #[test_case(
        DeviceInfoRef { device_class: DeviceClass::Ethernet, ..default_device_info() },
        "",
        vec![ProvisioningMatchingRule::Common(MatchingRule::Any(true))],
        true;
        "true_single_rule"
    )]
    #[test_case(
        DeviceInfoRef { device_class: DeviceClass::Ethernet, ..default_device_info() },
        "wlanx5009",
        vec![
            ProvisioningMatchingRule::Common(
                MatchingRule::DeviceClasses(vec![DeviceClass::Ethernet])
            ),
            ProvisioningMatchingRule::InterfaceName {
                pattern: glob::Pattern::new("wlanx*").unwrap()
            }
        ],
        true;
        "true_multiple_rules"
    )]
    fn test_does_interface_match_provisioning_rule(
        info: DeviceInfoRef<'_>,
        interface_name: &str,
        matching_rules: Vec<ProvisioningMatchingRule>,
        want_match: bool,
    ) {
        let provisioning_rule = ProvisioningRule {
            matchers: HashSet::from_iter(matching_rules),
            action: ProvisioningAction {
                provisioning: ProvisioningType::Local,
                ..Default::default()
            },
        };
        assert_eq!(provisioning_rule.does_interface_match(&info, interface_name), want_match);
    }

    #[test_case(
        vec![NameCompositionRule::Static { value: String::from("x") }],
        default_device_info(),
        "x";
        "single_static"
    )]
    #[test_case(
        vec![
            NameCompositionRule::Static { value: String::from("eth") },
            NameCompositionRule::Static { value: String::from("x") },
            NameCompositionRule::Static { value: String::from("100") },
        ],
        default_device_info(),
        "ethx100";
        "multiple_static"
    )]
    #[test_case(
        vec![NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::NormalizedMac }],
        DeviceInfoRef {
            mac: &fidl_fuchsia_net_ext::MacAddress { octets: [0x1, 0x1, 0x1, 0x1, 0x1, 0x1] },
            ..default_device_info()
        },
        "1";
        "normalized_mac"
    )]
    #[test_case(
        vec![
            NameCompositionRule::Static { value: String::from("eth") },
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::NormalizedMac },
        ],
        DeviceInfoRef {
            mac: &fidl_fuchsia_net_ext::MacAddress { octets: [0x1, 0x1, 0x1, 0x1, 0x1, 0x9] },
            ..default_device_info()
        },
        "eth9";
        "normalized_mac_with_static"
    )]
    #[test_case(
        vec![NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::DeviceClass }],
        DeviceInfoRef { device_class: DeviceClass::Ethernet, ..default_device_info() },
        "eth";
        "eth_device_class"
    )]
    #[test_case(
        vec![NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::DeviceClass }],
        DeviceInfoRef { device_class: DeviceClass::WlanClient, ..default_device_info() },
        "wlan";
        "wlan_device_class"
    )]
    #[test_case(
        vec![
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::DeviceClass },
            NameCompositionRule::Static { value: String::from("x") },
        ],
        DeviceInfoRef { device_class: DeviceClass::Ethernet, ..default_device_info() },
        "ethx";
        "device_class_with_static"
    )]
    #[test_case(
        vec![
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::DeviceClass },
            NameCompositionRule::Static { value: String::from("x") },
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::NormalizedMac },
        ],
        DeviceInfoRef {
            device_class: DeviceClass::WlanClient,
            mac: &fidl_fuchsia_net_ext::MacAddress { octets: [0x1, 0x1, 0x1, 0x1, 0x1, 0x8] },
            ..default_device_info()
        },
        "wlanx8";
        "device_class_with_static_with_normalized_mac"
    )]
    #[test_case(
        vec![
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::DeviceClass },
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::BusType },
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::BusPath },
        ],
        DeviceInfoRef {
            device_class: DeviceClass::Ethernet,
            topological_path: "/dev/sys/platform/pt/PCI0/bus/00:14.0_/00:14.0/ethernet",
            ..default_device_info()
        },
        "ethp0014";
        "device_class_with_pci_bus_type_with_bus_path"
    )]
    #[test_case(
        vec![
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::DeviceClass },
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::BusType },
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::BusPath },
        ],
        DeviceInfoRef {
            device_class: DeviceClass::Ethernet,
            topological_path: "/dev/sys/platform/pt/PCI0/bus/00:14.0/00:14.0/xhci/usb/004/004/ifc-000/ax88179/ethernet",
            ..default_device_info()
        },
        "ethu0014";
        "device_class_with_pci_usb_bus_type_with_bus_path"
    )]
    #[test_case(
        vec![
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::DeviceClass },
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::BusType },
            NameCompositionRule::Dynamic { rule: DynamicNameCompositionRule::BusPath },
        ],
        DeviceInfoRef {
            device_class: DeviceClass::Ethernet,
            topological_path: "/dev/sys/platform/05:00:18/usb-phy-composite/aml_usb_phy/dwc2/dwc2_phy/dwc2/usb-peripheral/function-000/usb-cdc-netdev/network-device",
            ..default_device_info()
        },
        "ethu050018";
        "device_class_with_dwc_usb_bus_type_with_bus_path"
    )]
    #[test_case(
        vec![NameCompositionRule::Default],
        DeviceInfoRef {
            device_class: DeviceClass::Ethernet,
            topological_path: "/dev/sys/platform/pt/PCI0/bus/00:14.0/00:14.0/xhci/usb/004/004/ifc-000/ax88179/ethernet",
            mac: &fidl_fuchsia_net_ext::MacAddress { octets: [0x1, 0x1, 0x1, 0x1, 0x1, 0x2] },
        },
        "ethx2";
        "default_usb_pci"
    )]
    #[test_case(
        vec![NameCompositionRule::Default],
        DeviceInfoRef {
            device_class: DeviceClass::Ethernet,
            topological_path: "/dev/sys/platform/05:00:18/usb-phy-composite/aml_usb_phy/dwc2/dwc2_phy/dwc2/usb-peripheral/function-000/usb-cdc-netdev/network-device",
            mac: &fidl_fuchsia_net_ext::MacAddress { octets: [0x1, 0x1, 0x1, 0x1, 0x1, 0x3] },
        },
        "ethx3";
        "default_usb_dwc"
    )]
    #[test_case(
        vec![NameCompositionRule::Default],
        DeviceInfoRef {
            device_class: DeviceClass::Ethernet,
            topological_path: "/dev/sys/platform/05:00:6/aml-sd-emmc/sdio/broadcom-wlanphy",
            ..default_device_info()
        },
        "eths05006";
        "default_sdio"
    )]
    fn test_naming_rules(
        composition_rules: Vec<NameCompositionRule>,
        info: DeviceInfoRef<'_>,
        expected_name: &'static str,
    ) {
        let naming_rule = NamingRule { matchers: HashSet::new(), naming_scheme: composition_rules };

        let name = naming_rule.generate_name(&HashMap::new(), &info);
        assert_eq!(name.unwrap(), expected_name.to_owned());
    }

    #[test]
    fn test_generate_name_from_naming_rule_interface_name_exists_no_reattempt() {
        let topo_usb = "/dev/pci-00:14.0-fidl/xhci/usb/004/004/ifc-000/ax88179/ethernet";

        let shared_interface_name = "x".to_owned();
        let mut interfaces = HashMap::new();
        assert_matches!(
            interfaces.insert(
                InterfaceNamingIdentifier {
                    mac: fidl_fuchsia_net_ext::MacAddress {
                        octets: [0x1, 0x1, 0x1, 0x1, 0x1, 0x1]
                    },
                    topological_path: topo_usb.to_string()
                },
                shared_interface_name.clone(),
            ),
            None
        );

        let naming_rule = NamingRule {
            matchers: HashSet::new(),
            naming_scheme: vec![NameCompositionRule::Static {
                value: shared_interface_name.clone(),
            }],
        };

        let name = naming_rule.generate_name(&interfaces, &default_device_info()).unwrap();
        assert_eq!(name, shared_interface_name);
    }

    // This test is different from `test_get_usb_255_with_naming_rule` as this
    // test increments the last byte, ensuring that the offset is reset prior
    // to each name being generated.
    #[test]
    fn test_generate_name_from_naming_rule_many_unique_macs() {
        let topo_usb = "/dev/pci-00:14.0-fidl/xhci/usb/004/004/ifc-000/ax88179/ethernet";

        let naming_rule = NamingRule {
            matchers: HashSet::new(),
            naming_scheme: vec![NameCompositionRule::Dynamic {
                rule: DynamicNameCompositionRule::NormalizedMac,
            }],
        };

        // test cases for 256 usb interfaces
        let mut interfaces = HashMap::new();

        for n in 0u8..255u8 {
            let octets = [0x01, 0x01, 0x01, 0x01, 0x01, n];
            let interface_naming_id =
                generate_identifier(&fidl_fuchsia_net_ext::MacAddress { octets }, topo_usb);
            let info = DeviceInfoRef {
                device_class: DeviceClass::Ethernet,
                mac: &fidl_fuchsia_net_ext::MacAddress { octets },
                topological_path: topo_usb,
            };

            let name =
                naming_rule.generate_name(&interfaces, &info).expect("failed to generate the name");
            assert_eq!(name, format!("{n:x}"));

            assert_matches!(interfaces.insert(interface_naming_id, name.clone()), None);
        }
    }

    #[test_case(true, "x"; "matches_first_rule")]
    #[test_case(false, "ethx1"; "fallback_default")]
    fn test_generate_name_from_naming_rules(match_first_rule: bool, expected_name: &'static str) {
        // Use an Ethernet device that is determined to have a USB bus type
        // from the topological path.
        let info = DeviceInfoRef {
            device_class: DeviceClass::Ethernet,
            mac: &fidl_fuchsia_net_ext::MacAddress { octets: [0x1, 0x1, 0x1, 0x1, 0x1, 0x1] },
            topological_path: "/dev/sys/platform/pt/PCI0/bus/00:14.0/00:14.0/xhci/usb/004/004/ifc-000/ax88179/ethernet",
        };
        let name = generate_name_from_naming_rules(
            &[
                NamingRule {
                    matchers: HashSet::from([MatchingRule::Any(match_first_rule)]),
                    naming_scheme: vec![NameCompositionRule::Static { value: String::from("x") }],
                },
                // Include an arbitrary rule that matches no interface
                // to ensure that it has no impact on the test.
                NamingRule {
                    matchers: HashSet::from([MatchingRule::Any(false)]),
                    naming_scheme: vec![NameCompositionRule::Static { value: String::from("y") }],
                },
            ],
            &HashMap::new(),
            &info,
        )
        .unwrap();
        assert_eq!(name, expected_name.to_owned());
    }

    #[test_case(true, ProvisioningType::Delegated; "matches_first_rule")]
    #[test_case(false, ProvisioningType::Local; "fallback_default")]
    fn test_find_provisioning_action_from_provisioning_rules(
        match_first_rule: bool,
        expected: ProvisioningType,
    ) {
        let provisioning_action = find_provisioning_action_from_provisioning_rules(
            &[ProvisioningRule {
                matchers: HashSet::from([ProvisioningMatchingRule::Common(MatchingRule::Any(
                    match_first_rule,
                ))]),
                action: ProvisioningAction {
                    provisioning: ProvisioningType::Delegated,
                    ..Default::default()
                },
            }],
            &DeviceInfoRef {
                device_class: DeviceClass::WlanClient,
                mac: &fidl_fuchsia_net_ext::MacAddress { octets: [0x1, 0x1, 0x1, 0x1, 0x1, 0x1] },
                topological_path: "",
            },
            "wlans5009",
        );
        assert_eq!(provisioning_action.provisioning, expected);
    }
}
