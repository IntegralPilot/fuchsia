// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::net::IpAddr;
use crate::yaml;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_yaml::Value;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
/// Config used by antlion for declaring testbeds and test parameters.
pub(crate) struct Config {
    #[serde(rename = "TestBeds")]
    pub testbeds: Vec<Testbed>,
    pub mobly_params: MoblyParams,
}

impl Config {
    /// Merge the given test parameters into all testbeds.
    pub fn merge_test_params(&mut self, test_params: Value) {
        for testbed in self.testbeds.iter_mut() {
            match testbed.test_params.as_mut() {
                Some(existing) => yaml::merge(existing, test_params.clone()),
                None => testbed.test_params = Some(test_params.clone()),
            }
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
/// Parameters consumed by Mobly.
pub(crate) struct MoblyParams {
    pub log_path: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
/// A group of interconnected devices to be used together during an antlion test.
pub(crate) struct Testbed {
    pub name: String,
    pub controllers: Controllers,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_params: Option<Value>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Controllers {
    #[serde(rename = "FuchsiaDevice", skip_serializing_if = "Vec::is_empty")]
    pub fuchsia_devices: Vec<Fuchsia>,
    #[serde(rename = "AccessPoint", skip_serializing_if = "Vec::is_empty")]
    pub access_points: Vec<AccessPoint>,
    #[serde(rename = "OpenWrtAP", skip_serializing_if = "Vec::is_empty")]
    pub openwrt_aps: Vec<AccessPoint>,
    #[serde(rename = "Attenuator", skip_serializing_if = "Vec::is_empty")]
    pub attenuators: Vec<Attenuator>,
    #[serde(rename = "PduDevice", skip_serializing_if = "Vec::is_empty")]
    pub pdus: Vec<Pdu>,
    #[serde(rename = "IPerfServer", skip_serializing_if = "Vec::is_empty")]
    pub iperf_servers: Vec<IPerfServer>,
}

#[derive(Clone, Debug, Serialize)]
/// A Fuchsia device, which can be consumed by either the Antlion controller defined in the
/// [antlion fuchsia_device.py] or the Honeydew controller defined in the
/// [honeydew fuchsia_device.py].
///
/// [antlion fuchsia_device.py]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/antlion/packages/antlion/controllers/fuchsia_device.py
/// [honeydew fuchsia_device.py]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/honeydew/honeydew/fuchsia_device/fuchsia_device.py
pub(crate) struct Fuchsia {
    pub name: String,
    pub ip: IpAddr,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_port: Option<u16>,
    /// Duplicate of ip / ssh_port, used for Honeydew
    pub device_ip_port: String,
    pub take_bug_report_on_fail: bool,
    pub ssh_binary_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_config: Option<PathBuf>,
    pub ffx_binary_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ffx_subtools_search_path: Option<PathBuf>,
    pub ssh_priv_key: PathBuf,
    #[serde(rename = "PduDevice", skip_serializing_if = "Option::is_none")]
    pub pdu_device: Option<PduRef>,
    pub hard_reboot_on_fail: bool,
    // Also include the config expected by Honeydew, so that these tests can run either with
    // Antlion or directly with Honeydew.
    pub honeydew_config: HoneydewConfig,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HoneydewConfig {
    pub transports: HoneydewTransports,
    pub affordances: HoneydewAffordances,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HoneydewTransports {
    pub ffx: HoneydewFfx,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HoneydewFfx {
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtools_search_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HoneydewAffordances {
    pub bluetooth: HoneydewAffordanceSpec,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HoneydewAffordanceSpec {
    pub implementation: String,
}

pub const HONEYDEW_IMPL_FUCHSIA_CONTROLLER: &'static str = "fuchsia-controller";

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Reference to a PDU device. Used to specify which port the attached device
/// maps to on the PDU.
pub(crate) struct PduRef {
    #[serde(default = "default_pdu_device")]
    pub device: String,
    #[serde(rename(serialize = "host"))]
    pub ip: IpAddr,
    pub port: u8,
}

fn default_pdu_device() -> String {
    "synaccess.np02b".to_string()
}

#[derive(Clone, Debug, Serialize)]
/// Declares an access point for use with antlion as defined by [access_point.py].
///
/// [access_point.py]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/antlion/packages/antlion/controllers/access_point.py
pub(crate) struct AccessPoint {
    pub wan_interface: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodename: Option<String>,
    pub ssh_config: SshConfig,
    #[serde(rename = "PduDevice", skip_serializing_if = "Option::is_none")]
    pub pdu_device: Option<PduRef>,
    #[serde(rename = "Attenuator", skip_serializing_if = "Option::is_none")]
    pub attenuators: Option<Vec<AttenuatorRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_regdb_bypass: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SshConfig {
    pub ssh_binary_path: PathBuf,
    pub host: IpAddr,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub user: String,
    pub identity_file: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
/// Reference to an attenuator device. Used to specify which ports the attached
/// devices' channels maps to on the attenuator.
pub(crate) struct AttenuatorRef {
    #[serde(rename = "Address")]
    pub address: IpAddr,
    #[serde(rename = "attenuator_ports_wifi_2g")]
    pub ports_2g: Vec<u8>,
    #[serde(rename = "attenuator_ports_wifi_5g")]
    pub ports_5g: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
/// Declares an attenuator for use with antlion as defined by [attenuator.py].
///
/// [access_point.py]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/antlion/packages/antlion/controllers/attenuator.py
pub(crate) struct Attenuator {
    pub model: String,
    pub instrument_count: u8,
    pub address: IpAddr,
    pub protocol: String,
    pub port: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// Declares a power distribution unit for use with antlion as defined by [pdu.py].
///
/// [pdu.py]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/antlion/packages/antlion/controllers/pdu.py
pub(crate) struct Pdu {
    pub device: String,
    pub host: IpAddr,
}

#[derive(Clone, Debug, Serialize)]
/// Declares an iPerf3 server for use with antlion as defined by [iperf_server.py].
///
/// [iperf_server.py]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/antlion/packages/antlion/controllers/iperf_server.py
pub(crate) struct IPerfServer {
    pub ssh_config: SshConfig,
    pub port: u16,
    pub test_interface: String,
    pub use_killall: bool,
}
