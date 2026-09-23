// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Serialize};

/// Represents a GCE custom image object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_disk: Option<RawDisk>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guest_os_features: Vec<GuestOsFeature>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RawDisk {
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GuestOsFeature {
    #[serde(rename = "type")]
    pub feature_type: String,
}

/// Represents an operation returned by asynchronous GCE API calls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<OperationError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct OperationError {
    #[serde(default)]
    pub errors: Vec<OperationErrorItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct OperationErrorItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Represents a GCE virtual machine instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<AttachedDisk>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_interfaces: Vec<NetworkInterface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_link: Option<String>,
}

impl Instance {
    /// Extracts the internal primary IPv4 address, if assigned.
    pub fn internal_ip(&self) -> Option<&str> {
        self.network_interfaces.first().and_then(|nic| nic.network_ip.as_deref())
    }

    /// Extracts the external public IPv4 address, if assigned.
    pub fn external_ip(&self) -> Option<&str> {
        self.network_interfaces
            .first()
            .and_then(|nic| nic.access_configs.first())
            .and_then(|cfg| cfg.nat_ip.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AttachedDisk {
    #[serde(default)]
    pub boot: bool,
    #[serde(default)]
    pub auto_delete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialize_params: Option<AttachedDiskInitializeParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AttachedDiskInitializeParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_size_gb: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInterface {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(rename = "networkIP", alias = "networkIp", skip_serializing_if = "Option::is_none")]
    pub network_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access_configs: Vec<AccessConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nic_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccessConfig {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub access_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "natIP", alias = "natIp", skip_serializing_if = "Option::is_none")]
    pub nat_ip: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    #[serde(default)]
    pub items: Vec<MetadataItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MetadataItem {
    pub key: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub value: String,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// Represents the list response from GET /compute/v1/projects/{project}/zones/{zone}/instances.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct InstanceList {
    #[serde(default)]
    pub items: Vec<Instance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

/// Represents serial port output returned by GCE REST API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SerialPortOutput {
    #[serde(default)]
    pub contents: String,
    #[serde(default, deserialize_with = "deserialize_string_as_i64")]
    pub start: i64,
    #[serde(default, deserialize_with = "deserialize_string_as_i64")]
    pub next: i64,
}

/// Deserializes an int64 field encoded as a decimal JSON string.
///
/// In the Google Compute Engine REST API, 64-bit integers (`start` and `next`)
/// are returned as JSON decimal strings (e.g. `"1024"`) per protobuf-to-JSON mapping
/// rules to prevent precision loss in JavaScript clients.
fn deserialize_string_as_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.parse::<i64>().map_err(serde::de::Error::custom)
}

/// Represents a GCE firewall rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FirewallRule {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_ranges: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<FirewallAllowed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FirewallAllowed {
    #[serde(rename = "IPProtocol")]
    pub ip_protocol: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
}

/// Result returned by `ffx gce start` in machine-readable output format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct StartResult {
    pub name: String,
    pub project: String,
    pub zone: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_port: Option<u16>,
}

/// Result returned by `ffx gce stop` in machine-readable output format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct StopResult {
    pub name: String,
    pub project: String,
    pub zone: String,
    pub action: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_deserialize_instance_list() {
        let json_str = r#"{
            "items": [
                {
                    "name": "fuchsia-vm-1",
                    "machineType": "zones/us-central1-a/machineTypes/n2-standard-4",
                    "status": "RUNNING",
                    "networkInterfaces": [
                        {
                            "networkIP": "10.128.0.2",
                            "accessConfigs": [
                                {
                                    "natIP": "35.200.100.50"
                                }
                            ]
                        }
                    ]
                }
            ]
        }"#;

        let list: InstanceList = serde_json::from_str(json_str).expect("parsed instance list");
        assert_eq!(list.items.len(), 1);
        let inst = &list.items[0];
        assert_eq!(inst.name.as_deref(), Some("fuchsia-vm-1"));
        assert_eq!(inst.status.as_deref(), Some("RUNNING"));
        assert_eq!(inst.internal_ip(), Some("10.128.0.2"));
        assert_eq!(inst.external_ip(), Some("35.200.100.50"));
    }

    #[fuchsia::test]
    fn test_empty_instance_list() {
        let json_str = r#"{}"#;
        let list: InstanceList = serde_json::from_str(json_str).expect("parsed empty list");
        assert!(list.items.is_empty());
    }

    #[fuchsia::test]
    fn test_serial_port_output_deserialization() {
        let json_str = r#"{"contents": "world", "start": "10", "next": "20"}"#;
        let spo: SerialPortOutput = serde_json::from_str(json_str).unwrap();
        assert_eq!(spo.contents, "world");
        assert_eq!(spo.start, 10);
        assert_eq!(spo.next, 20);
    }

    #[fuchsia::test]
    fn test_serial_port_output_rejects_numeric_literal() {
        let json_int = r#"{"contents": "hello", "start": 0, "next": 10}"#;
        assert!(serde_json::from_str::<SerialPortOutput>(json_int).is_err());
    }

    #[fuchsia::test]
    fn test_instance_ip_extraction() {
        let instance = Instance {
            name: Some("test-vm".to_string()),
            network_interfaces: vec![NetworkInterface {
                network_ip: Some("10.128.0.5".to_string()),
                access_configs: vec![AccessConfig {
                    nat_ip: Some("34.120.10.20".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        assert_eq!(instance.internal_ip(), Some("10.128.0.5"));
        assert_eq!(instance.external_ip(), Some("34.120.10.20"));
    }

    #[fuchsia::test]
    fn test_operation_deserialization() {
        let json = r#"{
            "id": "123456789",
            "name": "operation-123",
            "status": "DONE",
            "targetLink": "https://www.googleapis.com/compute/v1/projects/my-proj/zones/us-central1-a/instances/test-vm"
        }"#;

        let op: Operation = serde_json::from_str(json).expect("deserialize operation");
        assert_eq!(op.name.as_deref(), Some("operation-123"));
        assert_eq!(op.status.as_deref(), Some("DONE"));
    }

    #[fuchsia::test]
    fn test_firewall_rule_serialization() {
        let rule = FirewallRule {
            name: "allow-ssh-ingress-default".to_string(),
            network: Some("global/networks/default".to_string()),
            source_ranges: vec!["172.253.30.0/23".to_string()],
            allowed: vec![FirewallAllowed {
                ip_protocol: "tcp".to_string(),
                ports: vec!["22".to_string()],
            }],
            direction: Some("INGRESS".to_string()),
            priority: Some(1000),
        };

        let json = serde_json::to_string(&rule).expect("serialize firewall rule");
        assert!(json.contains("\"IPProtocol\":\"tcp\""));
        assert!(json.contains("\"172.253.30.0/23\""));
        assert!(json.contains("\"direction\":\"INGRESS\""));
        let parsed: FirewallRule = serde_json::from_str(&json).expect("deserialize firewall rule");
        assert_eq!(parsed, rule);
    }

    #[fuchsia::test]
    fn test_metadata_item_deserialization() {
        let with_val: MetadataItem =
            serde_json::from_str(r#"{"key": "serial-port-enable", "value": "1"}"#).unwrap();
        assert_eq!(with_val.key, "serial-port-enable");
        assert_eq!(with_val.value, "1");

        let missing_val: MetadataItem =
            serde_json::from_str(r#"{"key": "serial-port-enable"}"#).unwrap();
        assert_eq!(missing_val.key, "serial-port-enable");
        assert_eq!(missing_val.value, "");

        let null_val: MetadataItem =
            serde_json::from_str(r#"{"key": "serial-port-enable", "value": null}"#).unwrap();
        assert_eq!(null_val.key, "serial-port-enable");
        assert_eq!(null_val.value, "");
    }

    #[fuchsia::test]
    fn test_start_result_serialization() {
        let result = StartResult {
            name: "test-vm".to_string(),
            project: "test-proj".to_string(),
            zone: "us-central1-a".to_string(),
            status: "RUNNING".to_string(),
            internal_ip: Some("10.128.0.2".to_string()),
            external_ip: Some("34.120.10.20".to_string()),
            ssh_port: Some(12345),
        };

        let json = serde_json::to_string(&result).expect("serialize start result");
        assert!(json.contains("\"internalIp\":\"10.128.0.2\""));
        assert!(json.contains("\"sshPort\":12345"));
        let parsed: StartResult = serde_json::from_str(&json).expect("deserialize start result");
        assert_eq!(parsed, result);
    }

    #[fuchsia::test]
    fn test_stop_result_serialization() {
        let result = StopResult {
            name: "test-vm".to_string(),
            project: "test-proj".to_string(),
            zone: "us-central1-a".to_string(),
            action: "stopped".to_string(),
        };

        let json = serde_json::to_string(&result).expect("serialize stop result");
        assert!(json.contains("\"action\":\"stopped\""));
        let parsed: StopResult = serde_json::from_str(&json).expect("deserialize stop result");
        assert_eq!(parsed, result);
    }

    #[fuchsia::test]
    fn test_image_serialization_roundtrip() {
        let image = Image {
            name: Some("fuchsia-test-img".to_string()),
            raw_disk: Some(RawDisk {
                source: "https://storage.googleapis.com/b/disk.tar.gz".to_string(),
            }),
            guest_os_features: vec![GuestOsFeature {
                feature_type: "VIRTIO_SCSI_MULTIQUEUE".to_string(),
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&image).expect("serialize image");
        assert!(json.contains("\"rawDisk\":{\"source\":"));
        assert!(json.contains("\"guestOsFeatures\":[{\"type\":\"VIRTIO_SCSI_MULTIQUEUE\"}]"));
        let parsed: Image = serde_json::from_str(&json).expect("deserialize image");
        assert_eq!(parsed, image);
    }

    #[fuchsia::test]
    fn test_operation_deserialization_with_error() {
        let json = r#"{
            "id": "123",
            "status": "DONE",
            "error": {
                "errors": [
                    {"code": "RESOURCE_ALREADY_EXISTS", "message": "Image already exists"}
                ]
            }
        }"#;
        let op: Operation = serde_json::from_str(json).expect("deserialize operation with error");
        assert!(op.error.is_some());
        assert_eq!(op.error.unwrap().errors[0].code.as_deref(), Some("RESOURCE_ALREADY_EXISTS"));
    }
}
