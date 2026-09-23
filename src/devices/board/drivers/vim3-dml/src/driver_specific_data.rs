// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use fidl_fuchsia_driver_metadata as fdr;
use fidl_fuchsia_hardware_clockimpl as clockimpl;
use fidl_fuchsia_hardware_sdmmc;

/// Maps child device names (defined in DML) to helper functions that construct and
/// serialize their driver-specific metadata.
pub static VIM3_DRIVER_METADATA: dml_config::parser::DriverSpecificMetadata = phf::phf_map! {
    "adc-buttons" => &[
        ("fuchsia.buttons.AdcButtonsMetadata", get_adc_buttons_metadata),
    ],
    "gpio-buttons" => &[
        ("fuchsia.buttons.GpioButtonsMetadata", get_gpio_buttons_metadata),
    ],
    "power-controller" => &[
        ("fuchsia.hardware.power.DomainMetadata", get_power_domain_metadata),
    ],
    "usb-phy-ffe09000" => &[
        ("fuchsia.hardware.usb.phy.Metadata", get_aml_usb_phy_metadata),
    ],
    "temperature-sensor-ff634800" => &[
        ("fuchsia.hardware.trippoint.TripDeviceMetadata", get_cpu_thermal_metadata),
    ],
    "temperature-sensor-ff634c00" => &[
        ("fuchsia.hardware.trippoint.TripDeviceMetadata", get_ddr_thermal_metadata),
    ],
    "nna-ff100000" => &[
        ("0", get_aml_nna_metadata),
    ],
    "pwm_a-regulator" => &[
        ("fuchsia.hardware.vreg.VregMetadata", get_pwm_a_regulator_metadata),
    ],
    "pwm_a0_d-regulator" => &[
        ("fuchsia.hardware.vreg.VregMetadata", get_pwm_a0_d_regulator_metadata),
    ],
    "usb-ff400000" => &[
        ("fuchsia.hardware.usb.dwc2.Metadata", get_dwc2_metadata),
    ],
    "cpu-controller-0" => &[
        ("fuchsia.hardware.amlogic.metadata.CpuMetadata", get_cpu_metadata),
    ],
    "bt-uart-ffd24000" => &[
        ("fuchsia.hardware.serial.SerialPortInfo", get_bt_uart_metadata),
    ],
    "wifi" => &[
        ("fuchsia.wlan.broadcom.WifiConfig", get_wifi_metadata),
    ],
    "pwm-ffd1b000" => &[
        ("fuchsia.hardware.pwm.PwmChannelsMetadata", get_pwm_metadata),
    ],
    "clock-controller-ff63c000" => &[
        ("fuchsia.hardware.clockimpl.InitMetadata", get_clock_init_metadata),
    ],
    "i2c-1c000" => &[
        ("fuchsia.hardware.i2c.businfo.I2CBusMetadata", get_empty_i2c_metadata),
    ],
    "mmc-ffe07000" => &[
        ("fuchsia.hardware.sdmmc.SdmmcMetadata", get_emmc_metadata),
    ],
    "mmc-ffe05000" => &[
        ("fuchsia.hardware.sdmmc.SdmmcMetadata", get_sdcard_metadata),
    ],
    "mmc-ffe03000" => &[
        ("fuchsia.hardware.sdmmc.SdmmcMetadata", get_sdio_metadata),
    ],
};

/// Returns the SRAM base address for the NNA.
/// This matches the value in `src/devices/board/drivers/vim3-devicetree/visitors/vim3-nna.cc`.
fn get_aml_nna_metadata() -> anyhow::Result<Vec<u8>> {
    // A311D_NNA_SRAM_BASE
    let sram_base: u64 = 0xFF000000;
    Ok(sram_base.to_le_bytes().to_vec())
}

fn get_pwm_a_regulator_metadata() -> anyhow::Result<Vec<u8>> {
    let metadata = fidl_fuchsia_hardware_vreg::VregMetadata {
        name: Some("vreg-pwm-big".to_string()),
        min_voltage_uv: Some(690000),
        voltage_step_uv: Some(1000),
        num_steps: Some(361),
        ..Default::default()
    };
    fidl::persist(&metadata).context("Failed to serialize pwm_a regulator metadata")
}

fn get_pwm_a0_d_regulator_metadata() -> anyhow::Result<Vec<u8>> {
    let metadata = fidl_fuchsia_hardware_vreg::VregMetadata {
        name: Some("vreg-pwm-little".to_string()),
        min_voltage_uv: Some(690000),
        voltage_step_uv: Some(1000),
        num_steps: Some(361),
        ..Default::default()
    };
    fidl::persist(&metadata).context("Failed to serialize pwm_a0_d regulator metadata")
}

fn get_dwc2_metadata() -> anyhow::Result<Vec<u8>> {
    use fidl_fuchsia_hardware_usb_dwc2::{DmaBurstLen, Metadata};
    let mut tx_fifo_sizes = [0; 15];
    tx_fifo_sizes[0] = 128;
    tx_fifo_sizes[1] = 4;
    tx_fifo_sizes[2] = 128;
    tx_fifo_sizes[3] = 128;

    let metadata = Metadata {
        dma_burst_len: DmaBurstLen::Incr8,
        usb_turnaround_time: 9,
        rx_fifo_size: 256,
        nptx_fifo_size: 32,
        tx_fifo_sizes,
    };
    fidl::persist(&metadata).context("Failed to serialize dwc2 metadata")
}

fn get_cpu_metadata() -> anyhow::Result<Vec<u8>> {
    use fidl_fuchsia_hardware_amlogic_metadata::{CpuMetadata, OperatingPoint, PerformanceDomain};

    let performance_domains = vec![
        PerformanceDomain {
            id: 1,
            core_count: 4,
            relative_performance: 255,
            name: "big".to_string(),
        },
        PerformanceDomain {
            id: 2,
            core_count: 2,
            relative_performance: 160,
            name: "little".to_string(),
        },
    ];

    let operating_points = vec![
        OperatingPoint { freq_hz: 1000000000, volt_uv: 731000, pd_id: 1 },
        OperatingPoint { freq_hz: 1200000000, volt_uv: 751000, pd_id: 1 },
        OperatingPoint { freq_hz: 1398000000, volt_uv: 771000, pd_id: 1 },
        OperatingPoint { freq_hz: 1512000000, volt_uv: 771000, pd_id: 1 },
        OperatingPoint { freq_hz: 1608000000, volt_uv: 781000, pd_id: 1 },
        OperatingPoint { freq_hz: 1704000000, volt_uv: 791000, pd_id: 1 },
        OperatingPoint { freq_hz: 1800000000, volt_uv: 831000, pd_id: 1 },
        OperatingPoint { freq_hz: 1908000000, volt_uv: 861000, pd_id: 1 },
        OperatingPoint { freq_hz: 2016000000, volt_uv: 911000, pd_id: 1 },
        OperatingPoint { freq_hz: 2208000000, volt_uv: 1011000, pd_id: 1 },
        OperatingPoint { freq_hz: 1000000000, volt_uv: 761000, pd_id: 2 },
        OperatingPoint { freq_hz: 1200000000, volt_uv: 781000, pd_id: 2 },
        OperatingPoint { freq_hz: 1398000000, volt_uv: 811000, pd_id: 2 },
        OperatingPoint { freq_hz: 1512000000, volt_uv: 861000, pd_id: 2 },
        OperatingPoint { freq_hz: 1608000000, volt_uv: 901000, pd_id: 2 },
        OperatingPoint { freq_hz: 1704000000, volt_uv: 951000, pd_id: 2 },
        OperatingPoint { freq_hz: 1800000000, volt_uv: 1001000, pd_id: 2 },
    ];

    let metadata = CpuMetadata {
        performance_domains: Some(performance_domains),
        operating_points: Some(operating_points),
        ..Default::default()
    };

    fidl::persist(&metadata).context("Failed to serialize cpu metadata")
}

fn get_adc_buttons_metadata() -> anyhow::Result<Vec<u8>> {
    use fidl_fuchsia_buttons::{AdcButtonConfig, AdcButtonsMetadata, Button, ButtonConfig};
    use fidl_fuchsia_input_report::ConsumerControlButton;

    let metadata = AdcButtonsMetadata {
        polling_rate_usec: Some(20000),
        buttons: Some(vec![Button {
            types: Some(vec![ConsumerControlButton::Function]),
            button_config: Some(ButtonConfig::Adc(AdcButtonConfig {
                channel_idx: Some(2),
                release_threshold: Some(1000),
                press_threshold: Some(70),
                ..Default::default()
            })),
            ..Default::default()
        }]),
        ..Default::default()
    };

    fidl::persist(&metadata).context("Failed to serialize adc buttons metadata")
}

fn get_gpio_buttons_metadata() -> anyhow::Result<Vec<u8>> {
    use fidl_fuchsia_buttons::{
        DirectGpioButton, GpioButtonConfig, GpioButtonId, GpioButtonType, GpioButtonsMetadata,
        GpioConfig, GpioFlag, GpioType, InterruptGpio,
    };

    let metadata = GpioButtonsMetadata {
        buttons: Some(vec![GpioButtonConfig {
            type_: Some(GpioButtonType::Direct(DirectGpioButton::default())),
            gpio_a_index: Some(0),
            id: Some(GpioButtonId::Power),
            ..Default::default()
        }]),
        gpios: Some(vec![GpioConfig {
            type_: Some(GpioType::Interrupt(InterruptGpio::default())),
            flags: Some(GpioFlag::INVERTED),
            ..Default::default()
        }]),
        ..Default::default()
    };

    fidl::persist(&metadata).context("Failed to serialize gpio buttons metadata")
}

fn get_power_domain_metadata() -> anyhow::Result<Vec<u8>> {
    use fidl_fuchsia_hardware_power::{Domain, DomainMetadata};
    let metadata = DomainMetadata {
        domains: Some(vec![
            Domain { id: Some(0), ..Default::default() },
            Domain { id: Some(1), ..Default::default() },
        ]),
        ..Default::default()
    };
    fidl::persist(&metadata).context("Failed to serialize power domain metadata")
}

fn get_wifi_metadata() -> anyhow::Result<Vec<u8>> {
    use fidl_fuchsia_wlan_broadcom::{CcEntry, IovarCommand, IovarEntry, WifiConfig};

    let cc_codes = vec![
        "WW", "AU", "CA", "US", "GB", "BE", "BG", "CZ", "DK", "DE", "EE", "IE", "GR", "ES", "FR",
        "HR", "IT", "CY", "LV", "LT", "LU", "HU", "MT", "NL", "AT", "PL", "PT", "RO", "SI", "SK",
        "FI", "SE", "EL", "IS", "LI", "TR", "CH", "NO", "JP", "",
    ];

    let cc_table =
        cc_codes.into_iter().map(|cc| CcEntry { cc_abbr: cc.to_string(), cc_rev: 0 }).collect();

    let metadata = WifiConfig {
        oob_irq_mode: 8,
        clm_needed: false,
        iovar_table: vec![IovarEntry::Command(IovarCommand { cmd: 86, val: 0 })],
        cc_table,
    };

    fidl::persist(&metadata).context("Failed to serialize wifi metadata")
}

fn get_bt_uart_metadata() -> anyhow::Result<Vec<u8>> {
    use fidl_fuchsia_hardware_serial::{Class, SerialPortInfo};

    let metadata =
        SerialPortInfo { serial_class: Class::BluetoothHci, serial_vid: 6, serial_pid: 3 };

    fidl::persist(&metadata).context("Failed to serialize bt uart metadata")
}

fn get_aml_usb_phy_metadata() -> anyhow::Result<Vec<u8>> {
    use fidl_fuchsia_hardware_usb_phy::{
        AmlogicPhyType, Metadata, Mode, ProtocolVersion, UsbPhyMode,
    };

    let metadata = Metadata {
        phy_type: Some(AmlogicPhyType::G12B),
        usb_phy_modes: Some(vec![
            UsbPhyMode {
                protocol: Some(ProtocolVersion::Usb20),
                dr_mode: Some(Mode::Host),
                is_otg_capable: Some(false),
                ..Default::default()
            },
            UsbPhyMode {
                protocol: Some(ProtocolVersion::Usb20),
                dr_mode: Some(Mode::Peripheral),
                is_otg_capable: Some(true),
                ..Default::default()
            },
            UsbPhyMode {
                protocol: Some(ProtocolVersion::Usb30),
                dr_mode: Some(Mode::Host),
                is_otg_capable: Some(false),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };

    fidl::persist(&metadata).context("Failed to serialize aml usb phy metadata")
}

fn get_cpu_thermal_metadata() -> anyhow::Result<Vec<u8>> {
    use fidl_fuchsia_hardware_trippoint::TripDeviceMetadata;
    let metadata = TripDeviceMetadata { critical_temp_celsius: 101.0 };
    fidl::persist(&metadata).context("Failed to serialize cpu thermal metadata")
}

fn get_ddr_thermal_metadata() -> anyhow::Result<Vec<u8>> {
    use fidl_fuchsia_hardware_trippoint::TripDeviceMetadata;
    let metadata = TripDeviceMetadata { critical_temp_celsius: 110.0 };
    fidl::persist(&metadata).context("Failed to serialize ddr thermal metadata")
}

fn get_clock_init_metadata() -> anyhow::Result<Vec<u8>> {
    use clockimpl::{DisableType, EnableType, InitCall, InitStep};
    let metadata = clockimpl::InitMetadata {
        steps: vec![
            InitStep {
                id: Some(0x20002),
                call: Some(InitCall::Disable(DisableType {})),
                ..Default::default()
            },
            InitStep {
                id: Some(0x20002),
                call: Some(InitCall::RateHz(768_000_000)),
                ..Default::default()
            },
            InitStep {
                id: Some(0x20002),
                call: Some(InitCall::Enable(EnableType {})),
                ..Default::default()
            },
            // Configure PCIE_PLL (0x20001) to 100MHz and enable it
            InitStep {
                id: Some(0x20001),
                call: Some(InitCall::Disable(DisableType {})),
                ..Default::default()
            },
            InitStep {
                id: Some(0x20001),
                call: Some(InitCall::RateHz(100_000_000)),
                ..Default::default()
            },
            InitStep {
                id: Some(0x20001),
                call: Some(InitCall::Enable(EnableType {})),
                ..Default::default()
            },
            // Enable USB Gate (0x10008)
            InitStep {
                id: Some(0x10008),
                call: Some(InitCall::Enable(EnableType {})),
                ..Default::default()
            },
            // Enable USB1 to DDR Gate (0x10009)
            InitStep {
                id: Some(0x10009),
                call: Some(InitCall::Enable(EnableType {})),
                ..Default::default()
            },
            InitStep {
                id: Some(0x1000d),
                call: Some(InitCall::Enable(EnableType {})),
                ..Default::default()
            },
        ],
    };
    fidl::persist(&metadata).context("Failed to serialize clock init metadata")
}

fn get_emmc_metadata() -> anyhow::Result<Vec<u8>> {
    let metadata = fidl_fuchsia_hardware_sdmmc::SdmmcMetadata {
        max_frequency: Some(120_000_000),
        speed_capabilities: Some(fidl_fuchsia_hardware_sdmmc::SdmmcHostPrefs::DISABLE_HS400),
        use_fidl: Some(true),
        ..Default::default()
    };
    fidl::persist(&metadata).context("Failed to serialize eMMC metadata")
}

fn get_sdcard_metadata() -> anyhow::Result<Vec<u8>> {
    let metadata = fidl_fuchsia_hardware_sdmmc::SdmmcMetadata {
        max_frequency: Some(50_000_000),
        removable: Some(true),
        use_fidl: Some(false),
        ..Default::default()
    };
    fidl::persist(&metadata).context("Failed to serialize SD card metadata")
}

fn get_sdio_metadata() -> anyhow::Result<Vec<u8>> {
    let metadata = fidl_fuchsia_hardware_sdmmc::SdmmcMetadata {
        max_frequency: Some(100_000_000),
        use_fidl: Some(false),
        ..Default::default()
    };
    fidl::persist(&metadata).context("Failed to serialize SDIO metadata")
}
fn get_empty_i2c_metadata() -> anyhow::Result<Vec<u8>> {
    let dictionary = fdr::Dictionary {
        entries: Some(vec![
            fdr::DictionaryEntry {
                key: "controller_id".to_string(),
                value: fdr::DictionaryValue::Int64(2),
            },
            fdr::DictionaryEntry {
                key: "channels._count".to_string(),
                value: fdr::DictionaryValue::Int64(0),
            },
        ]),
        ..Default::default()
    };
    fidl::persist(&dictionary).context("Failed to serialize empty i2c metadata")
}

fn get_pwm_metadata() -> anyhow::Result<Vec<u8>> {
    let mut entries = vec![fdr::DictionaryEntry {
        key: "channels._count".to_string(),
        value: fdr::DictionaryValue::Int64(10),
    }];

    for i in 0..10 {
        entries.push(fdr::DictionaryEntry {
            key: format!("channels.{}.channel", i),
            value: fdr::DictionaryValue::Int64(i as i64),
        });
        let period = if i == 0 || i == 9 { 1250 } else { 0 };
        entries.push(fdr::DictionaryEntry {
            key: format!("channels.{}.period_ns", i),
            value: fdr::DictionaryValue::Int64(period),
        });
    }

    let dictionary = fdr::Dictionary { entries: Some(entries), ..Default::default() };
    fidl::persist(&dictionary).context("Failed to serialize pwm metadata")
}
