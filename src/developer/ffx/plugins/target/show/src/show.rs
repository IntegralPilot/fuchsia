// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_target_show_args::TargetShow;
use schemars::JsonSchema;
use serde::Serialize;
use std::default::Default;
use std::io::Write;
use termion::{color, style};

/// Store, organize, and display hierarchical show information. Output may be
/// formatted for a human reader or structured as JSON for machine consumption.

const INDENT: usize = 4;

// LINT.IfChange

#[derive(Debug, JsonSchema, PartialEq, Serialize)]
pub struct TargetShowInfo {
    pub target: TargetData,
    pub board: BoardData,
    pub device: DeviceData,
    pub product: ProductData,
    pub update: UpdateData,
    pub build: BuildData,
}

/// Information about the target device.
#[derive(Debug, JsonSchema, PartialEq, Serialize)]
pub struct TargetData {
    /// Node name of the target device.
    pub name: String,
    /// SSH address of the target device.
    pub ssh_address: Option<AddressData>,
    /// True if the last reboot was graceful.
    pub last_reboot_graceful: bool,
    /// Reason for last reboot, if available.
    pub last_reboot_reason: Option<String>,
    /// Target device update in nanoseconds.
    pub uptime_nanos: i64,
}

/// Simplified address information.
#[derive(Debug, JsonSchema, PartialEq, Serialize)]
pub struct AddressData {
    pub host: String,
    pub port: u16,
}

/// Information about the hardware board of the target device.
#[derive(Debug, JsonSchema, PartialEq, Serialize)]
pub struct BoardData {
    /// Board name, if known.
    pub name: Option<String>,
    /// Board revision information, if known.
    pub revision: Option<String>,
    /// Instruction set, if known.
    pub instruction_set: Option<String>,
}

/// Information about the product level device.
#[derive(Debug, JsonSchema, PartialEq, Serialize)]
pub struct DeviceData {
    /// Serial number, if known.
    pub serial_number: Option<String>,
    /// SKU if known.
    pub retail_sku: Option<String>,
    /// Device configured for demo mode.
    pub retail_demo: Option<bool>,
    /// Device ID for use in feedback messages.
    pub device_id: Option<String>,
}

/// Product information
#[derive(Debug, JsonSchema, PartialEq, Serialize)]
pub struct ProductData {
    /// Type of audio amp, if known.
    pub audio_amplifier: Option<String>,
    /// Product build date.
    pub build_date: Option<String>,
    /// Product build name
    pub build_name: Option<String>,
    /// Product's color scheme description.
    pub colorway: Option<String>,
    /// Display information, if known.
    pub display: Option<String>,
    /// Size of EMMC storage.
    pub emmc_storage: Option<String>,
    /// Product Language.
    pub language: Option<String>,
    /// Regulatory domain designation.
    pub regulatory_domain: Option<String>,
    /// Supported locales.
    pub locale_list: Vec<String>,
    /// Manufacturer name, if known.
    pub manufacturer: Option<String>,
    /// Type of microphone.
    pub microphone: Option<String>,
    /// Product Model information.
    pub model: Option<String>,
    /// Product name.
    pub name: Option<String>,
    /// Size of NAND storage.
    pub nand_storage: Option<String>,
    /// Amount of RAM.
    pub memory: Option<String>,
    /// SKU of the board.
    pub sku: Option<String>,
}

/// OTA channel information.
#[derive(Debug, JsonSchema, PartialEq, Serialize)]
pub struct UpdateData {
    pub current_channel: String,
    pub next_channel: Option<String>,
}

/// Information about the Fuchsia build.
#[derive(Debug, JsonSchema, PartialEq, Serialize)]
pub struct BuildData {
    /// Build version, if known.
    pub version: Option<String>,
    /// Fuchsia product.
    pub product: Option<String>,
    /// Board targeted for this build.
    pub board: Option<String>,
    /// Integration commit date.
    pub commit: Option<String>,
}

// LINT.ThenChange(/src/testing/end_to_end/honeydew/honeydew/transports/ffx/types.py)

impl TargetShowInfo {
    pub(crate) fn to_show_entries(&self) -> Vec<ShowEntry> {
        let target = (&self.target).into();
        let board = (&self.board).into();
        let device = (&self.device).into();
        let product = (&self.product).into();
        let update = (&self.update).into();
        let build = (&self.build).into();

        vec![target, board, device, product, update, build]
    }
}

impl From<&TargetData> for ShowEntry {
    fn from(value: &TargetData) -> Self {
        ShowEntry::group(
            "Target",
            "target",
            "",
            vec![
                ShowEntry::str_value_with_highlight(
                    "Name",
                    "name",
                    "Target name.",
                    &Some(value.name.clone()),
                ),
                ShowEntry::str_value_with_highlight(
                    "SSH Address",
                    "ssh_address",
                    "Interface address",
                    &value.ssh_address.as_ref().map(|addr| format!("{}:{}", addr.host, addr.port)),
                ),
                ShowEntry::str_value(
                    "Last Reboot Graceful",
                    "graceful",
                    "Whether the last reboot happened in a controlled manner.",
                    &Some(format!("{}", value.last_reboot_graceful)),
                ),
                ShowEntry::str_value(
                    "Last Reboot Reason",
                    "reason",
                    "Reason for the last reboot.",
                    &value.last_reboot_reason,
                ),
                ShowEntry::str_value(
                    "Uptime (ns)",
                    "uptime",
                    "How long the device was running prior to the last reboot.",
                    &Some(value.uptime_nanos.to_string()),
                ),
            ],
        )
    }
}

impl From<&BoardData> for ShowEntry {
    fn from(value: &BoardData) -> Self {
        ShowEntry::group(
            "Board",
            "board",
            "",
            vec![
                ShowEntry::str_value("Name", "name", "SOC board name.", &value.name),
                ShowEntry::str_value("Revision", "revision", "SOC revision.", &value.revision),
                ShowEntry::str_value(
                    "Instruction set",
                    "instruction set",
                    "Instruction set.",
                    &value.instruction_set,
                ),
            ],
        )
    }
}
impl From<&DeviceData> for ShowEntry {
    fn from(value: &DeviceData) -> Self {
        ShowEntry::group(
            "Device",
            "device",
            "",
            vec![
                ShowEntry::str_value(
                    "Serial number",
                    "serial_number",
                    "Unique ID for device.",
                    &value.serial_number,
                ),
                ShowEntry::str_value(
                    "Retail SKU",
                    "retail_sku",
                    "Stock Keeping Unit ID number.",
                    &value.retail_sku,
                ),
                ShowEntry::bool_value(
                    "Is retail demo",
                    "is_retail_demo",
                    "true if demonstration unit.",
                    &value.retail_demo,
                ),
                ShowEntry::str_value(
                    "Device ID",
                    "device_id",
                    "Feedback Device ID for target.",
                    &value.device_id,
                ),
            ],
        )
    }
}

impl From<&ProductData> for ShowEntry {
    fn from(value: &ProductData) -> Self {
        let mut locale_list = ShowEntry::new("Locale list", "locale_list", "Locales supported.");
        locale_list.value = Some(ShowValue::StringListValue(value.locale_list.clone()));

        ShowEntry::group(
            "Product",
            "product",
            "",
            vec![
                ShowEntry::str_value(
                    "Audio amplifier",
                    "audio_amplifier",
                    "Type of audio amp.",
                    &value.audio_amplifier,
                ),
                ShowEntry::str_value(
                    "Build date",
                    "build_date",
                    "When product was built.",
                    &value.build_date,
                ),
                ShowEntry::str_value(
                    "Build name",
                    "build_name",
                    "Reference name.",
                    &value.build_name,
                ),
                ShowEntry::str_value("Colorway", "colorway", "Colorway.", &value.colorway),
                ShowEntry::str_value("Display", "display", "Info about display.", &value.display),
                ShowEntry::str_value(
                    "EMMC storage",
                    "emmc_storage",
                    "Size of storage.",
                    &value.emmc_storage,
                ),
                ShowEntry::str_value("Language", "language", "language.", &value.language),
                ShowEntry::str_value(
                    "Regulatory domain",
                    "regulatory_domain",
                    "Domain designation.",
                    &value.regulatory_domain,
                ),
                locale_list,
                ShowEntry::str_value(
                    "Manufacturer",
                    "manufacturer",
                    "Manufacturer of product.",
                    &value.manufacturer,
                ),
                ShowEntry::str_value(
                    "Microphone",
                    "microphone",
                    "Type of microphone.",
                    &value.microphone,
                ),
                ShowEntry::str_value("Model", "model", "Model of the product.", &value.model),
                ShowEntry::str_value("Name", "name", "Name of the product.", &value.name),
                ShowEntry::str_value(
                    "NAND storage",
                    "nand_storage",
                    "Size of storage.",
                    &value.nand_storage,
                ),
                ShowEntry::str_value("Memory", "memory", "Amount of RAM.", &value.memory),
                ShowEntry::str_value("SKU", "sku", "SOC board name.", &value.sku),
            ],
        )
    }
}
impl From<&UpdateData> for ShowEntry {
    fn from(value: &UpdateData) -> Self {
        ShowEntry::group(
            "Update",
            "update",
            "",
            vec![
                ShowEntry::str_value(
                    "Current channel",
                    "current_channel",
                    "Channel that is currently in use.",
                    &Some(value.current_channel.clone()),
                ),
                ShowEntry::str_value(
                    "Next channel",
                    "next_channel",
                    "Channel used for the next update.",
                    &value.next_channel.clone(),
                ),
            ],
        )
    }
}

impl From<&BuildData> for ShowEntry {
    fn from(value: &BuildData) -> Self {
        ShowEntry::group(
            "Build",
            "build",
            "",
            vec![
                ShowEntry::str_value("Version", "version", "Build version.", &value.version),
                ShowEntry::str_value("Product", "product", "Product config.", &value.product),
                ShowEntry::str_value("Board", "board", "Board config.", &value.board),
                ShowEntry::str_value("Commit", "commit", "Integration Commit Date", &value.commit),
            ],
        )
    }
}

/// Show entry values.
#[derive(Debug, JsonSchema, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ShowValue {
    BoolValue(bool),
    StringValue(String),
    StringListValue(Vec<String>),
}

impl std::fmt::Display for ShowValue {
    fn fmt<'a>(&self, f: &mut std::fmt::Formatter<'a>) -> std::fmt::Result {
        match self {
            ShowValue::BoolValue(value) => write!(f, "{:?}", value),
            ShowValue::StringValue(value) => write!(f, "{:?}", value),
            ShowValue::StringListValue(value) => write!(f, "{:?}", value),
        }
    }
}

/// A node in a hierarchy of show information or groupings.
#[derive(Default, Debug, JsonSchema, PartialEq, Serialize)]
pub struct ShowEntry {
    pub title: String,
    pub label: String,
    pub description: String,
    pub value: Option<ShowValue>,
    pub child: Vec<ShowEntry>,
    #[serde(skip_serializing)]
    #[schemars(skip)]
    pub highlight: bool,
}

impl ShowEntry {
    pub fn new(title: &str, label: &str, description: &str) -> Self {
        Self {
            title: title.to_string(),
            label: label.to_string(),
            description: description.to_string(),
            ..Default::default()
        }
    }

    // Create a Group ShowEntry.
    pub fn group(human_name: &str, id_name: &str, desc: &str, value: Vec<ShowEntry>) -> ShowEntry {
        let mut entry = ShowEntry::new(human_name, id_name, desc);
        entry.child = value;
        entry
    }

    // Create a Boolean ShowEntry.
    pub fn bool_value(
        human_name: &str,
        id_name: &str,
        desc: &str,
        value: &Option<bool>,
    ) -> ShowEntry {
        let mut entry = ShowEntry::new(human_name, id_name, desc);
        entry.value = value.map(|v| ShowValue::BoolValue(v));
        entry
    }

    // Create a string ShowEntry.
    pub fn str_value(
        human_name: &str,
        id_name: &str,
        desc: &str,
        value: &Option<String>,
    ) -> ShowEntry {
        let mut entry = ShowEntry::new(human_name, id_name, desc);
        entry.value = value.as_ref().map(|v| ShowValue::StringValue(v.to_string()));
        entry
    }

    // Create a string ShowEntry with color.
    pub fn str_value_with_highlight(
        human_name: &str,
        id_name: &str,
        desc: &str,
        value: &Option<String>,
    ) -> ShowEntry {
        let mut entry = ShowEntry::new(human_name, id_name, desc);
        entry.value = value.as_ref().map(|v| ShowValue::StringValue(v.to_string()));
        entry.highlight = true;
        entry
    }
}

/// Write output for easy reading by humans.
fn output_list<W: Write>(
    indent: usize,
    showes: &Vec<ShowEntry>,
    args: &TargetShow,
    writer: &mut W,
) -> std::io::Result<()> {
    for show in showes {
        let mut label = "".to_string();
        if args.label && !show.label.is_empty() {
            label = format!(" ({})", show.label);
        }
        let mut desc = "".to_string();
        if args.desc && !show.description.is_empty() {
            desc = format!(" # {}", show.description);
        }
        match &show.value {
            Some(value) => {
                if show.highlight {
                    writeln!(
                        *writer,
                        "{: <7$}{}{}: {}{}{}{}",
                        "",
                        show.title,
                        label,
                        color::Fg(color::Green),
                        value,
                        style::Reset,
                        desc,
                        indent
                    )?;
                } else {
                    writeln!(
                        *writer,
                        "{: <5$}{}{}: {}{}",
                        "", show.title, label, value, desc, indent
                    )?;
                }
            }
            None => writeln!(*writer, "{: <4$}{}{}: {}", "", show.title, label, desc, indent)?,
        }
        output_list(indent + INDENT, &show.child, args, writer)?;
    }
    Ok(())
}

/// Write output in English for easy reading by users.
pub fn output_for_human<W: Write>(
    showes: &TargetShowInfo,
    args: &TargetShow,
    writer: &mut W,
) -> std::io::Result<()> {
    let entries = showes.to_show_entries();
    output_list(0, &entries, args, writer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_output() {
        assert_eq!(format!("{}", ShowValue::BoolValue(false)), "false");
        assert_eq!(format!("{}", ShowValue::BoolValue(true)), "true");
        assert_eq!(format!("{}", ShowValue::StringValue("abc".to_string())), "\"abc\"");
        assert_eq!(format!("{}", ShowValue::StringValue("ab\"c".to_string())), "\"ab\\\"c\"");
        assert_eq!(
            format!("{}", ShowValue::StringListValue(vec!["abc".to_string(), "def".to_string()])),
            "[\"abc\", \"def\"]"
        );
    }

    #[test]
    fn test_output_list() {
        let input = vec![ShowEntry::new("Test", "the_test", "A test.")];
        let mut result = Vec::new();
        output_list(7, &input, &TargetShow::default(), &mut result).unwrap();
        assert_eq!(result, b"       Test: \n");
    }

    #[test]
    fn test_output_list_with_child() {
        let input = vec![ShowEntry::group(
            "Test",
            "the_test",
            "A test.",
            vec![ShowEntry::new("Prop", "a_prop", "Some data.")],
        )];
        let mut result = Vec::new();
        output_list(0, &input, &TargetShow::default(), &mut result).unwrap();
        assert_eq!(result, b"Test: \n    Prop: \n");
    }

    #[test]
    fn test_product_to_showentry() {
        let data = ProductData {
            audio_amplifier: Some("fake_audio_amplifier".to_string()),
            build_date: Some("fake_build_date".to_string()),
            build_name: Some("fake_build_name".to_string()),
            colorway: Some("fake_colorway".to_string()),
            display: None,
            emmc_storage: None,
            language: None,
            regulatory_domain: None,
            locale_list: vec![],
            manufacturer: None,
            microphone: None,
            model: None,
            name: None,
            nand_storage: None,
            memory: None,
            sku: None,
        };

        let result: ShowEntry = (&data).into();
        assert_eq!(result.title, "Product");
        assert_eq!(result.child[0].title, "Audio amplifier");
        assert_eq!(
            result.child[0].value,
            Some(ShowValue::StringValue("fake_audio_amplifier".to_string()))
        );
        assert_eq!(result.child[1].title, "Build date");
        assert_eq!(
            result.child[1].value,
            Some(ShowValue::StringValue("fake_build_date".to_string()))
        );
        assert_eq!(result.child[2].title, "Build name");
        assert_eq!(
            result.child[2].value,
            Some(ShowValue::StringValue("fake_build_name".to_string()))
        );
        assert_eq!(result.child[3].title, "Colorway");
        assert_eq!(
            result.child[3].value,
            Some(ShowValue::StringValue("fake_colorway".to_string()))
        );
    }

    #[test]
    fn test_board_to_showentry() {
        let data = BoardData {
            name: Some("fake_name".to_string()),
            revision: Some("fake_revision".to_string()),
            instruction_set: None,
        };
        let result: ShowEntry = (&data).into();
        assert_eq!(result.title, "Board");
        assert_eq!(result.child[0].title, "Name");
        assert_eq!(result.child[0].value, Some(ShowValue::StringValue("fake_name".to_string())));
        assert_eq!(result.child[1].title, "Revision");
        assert_eq!(
            result.child[1].value,
            Some(ShowValue::StringValue("fake_revision".to_string()))
        );
    }

    #[test]
    fn test_device_to_showentry() {
        let data = DeviceData {
            serial_number: Some("fake_serial".to_string()),
            retail_sku: Some("fake_sku".to_string()),
            retail_demo: Some(false),
            device_id: None,
        };
        let result: ShowEntry = (&data).into();
        assert_eq!(result.title, "Device");
        assert_eq!(result.child[0].title, "Serial number");
        assert_eq!(result.child[0].value, Some(ShowValue::StringValue("fake_serial".to_string())));
        assert_eq!(result.child[1].title, "Retail SKU");
        assert_eq!(result.child[1].value, Some(ShowValue::StringValue("fake_sku".to_string())));
        assert_eq!(result.child[2].title, "Is retail demo");
        assert_eq!(result.child[2].value, Some(ShowValue::BoolValue(false)))
    }

    #[test]
    fn test_update_to_showentry() {
        let data = UpdateData {
            current_channel: "fake_channel".to_string(),
            next_channel: Some("fake_target".to_string()),
        };
        let result: ShowEntry = (&data).into();
        assert_eq!(result.title, "Update");
        assert_eq!(result.child[0].title, "Current channel");
        assert_eq!(result.child[0].value, Some(ShowValue::StringValue("fake_channel".to_string())));
        assert_eq!(result.child[1].title, "Next channel");
        assert_eq!(result.child[1].value, Some(ShowValue::StringValue("fake_target".to_string())));
    }
}
