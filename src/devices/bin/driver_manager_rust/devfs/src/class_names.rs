// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use phf::{phf_map, phf_set};

pub struct ServiceEntry {
    pub state: State,
    pub service_name: &'static str,
    pub member_name: &'static str,
}

pub enum State {
    Devfs,
    DevfsAndService,
}

// LINT.IfChange
pub static CLASS_NAME_TO_SERVICE: phf::Map<&'static str, ServiceEntry> = phf_map! {
    "acpi" => ServiceEntry { state: State::Devfs, service_name: "", member_name: "" },
    "adc" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.adc.Service",
        member_name: "device",
    },
    "audio-composite" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.audio.CompositeConnectorService",
        member_name: "composite_connector",
    },
    "audio-input" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.audio.StreamConfigConnectorInputService",
        member_name: "stream_config_connector",
    },
    "audio-output" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.audio.StreamConfigConnectorOutputService",
        member_name: "stream_config_connector",
    },
    "battery" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.power.battery.InfoService",
        member_name: "device",
    },
    "block-partition" => ServiceEntry { state: State::Devfs, service_name: "", member_name: "" },
    "block" => ServiceEntry { state: State::Devfs, service_name: "", member_name: "" },
    "block-volume" => ServiceEntry { state: State::Devfs, service_name: "", member_name: "" },
    "bt-emulator" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.bluetooth.EmulatorService",
        member_name: "device",
    },
    "bt-hci" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.bluetooth.Service",
        member_name: "vendor",
    },
    "clock-impl" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.clock.measure.Service",
        member_name: "measurer",
    },
    "codec" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.audio.CodecConnectorService",
        member_name: "codec_connector",
    },
    "cpu-ctrl" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.cpu.ctrl.Service",
        member_name: "device",
    },
    "dai" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.audio.DaiConnectorService",
        member_name: "dai_connector",
    },
    "devfs_service_test" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.services.test.Device",
        member_name: "control",
    },
    "display-coordinator" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.display.Service",
        member_name: "provider",
    },
    "goldfish-address-space" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.goldfish.AddressSpaceService",
        member_name: "device",
    },
    "goldfish-control" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.goldfish.ControlService",
        member_name: "device",
    },
    "goldfish-pipe" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.goldfish.ControllerService",
        member_name: "device",
    },
    "goldfish-sync" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.goldfish.SyncService",
        member_name: "device",
    },
    "gpio" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.pin.DebugService",
        member_name: "device",
    },
    "gpu-dependency-injection" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.gpu.magma.DependencyInjectionService",
        member_name: "device",
    },
    "gpu-performance-counters" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.gpu.magma.PerformanceCounterService",
        member_name: "access",
    },
    "gpu" => ServiceEntry {
        state: State::Devfs,
        service_name: "fuchsia.gpu.magma.Service",
        member_name: "device",
    },
    "hrtimer" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.hrtimer.Service",
        member_name: "device",
    },
    "i2c" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.i2c.Service",
        member_name: "device",
    },
    "input-report" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.input.report.Service",
        member_name: "input_device",
    },
    "input" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.input.Service",
        member_name: "controller",
    },
    "light" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.light.LightService",
        member_name: "light",
    },
    "media-codec" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.mediacodec.Service",
        member_name: "device",
    },
    "midi" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.midi.Service",
        member_name: "controller",
    },
    "nand" => ServiceEntry { state: State::Devfs, service_name: "", member_name: "" },
    "network" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.network.Service",
        member_name: "device",
    },
    "ot-radio" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.lowpan.spinel.Service",
        member_name: "device_setup",
    },
    "power-sensor" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.power.sensor.Service",
        member_name: "device",
    },
    "power" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.powersource.Service",
        member_name: "source",
    },
    "radar" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.radar.Service",
        member_name: "device",
    },
    "registers" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.registers.Service",
        member_name: "device",
    },
    "rtc" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.rtc.Service",
        member_name: "device",
    },
    "sdio" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.sdio.DriverService",
        member_name: "device",
    },
    "securemem" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.securemem.Service",
        member_name: "device",
    },
    "serial" => ServiceEntry { state: State::Devfs, service_name: "", member_name: "" },
    "skip-block" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.skipblock.Service",
        member_name: "skipblock",
    },
    "spi" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.spi.ControllerService",
        member_name: "device",
    },
    "tee" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.tee.Service",
        member_name: "device_connector",
    },
    "temperature" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.temperature.Service",
        member_name: "device",
    },
    "test" => ServiceEntry { state: State::Devfs, service_name: "", member_name: "" },
    "test-asix-function" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.ax88179.Service",
        member_name: "hooks",
    },
    "thermal" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.thermal.Service",
        member_name: "device",
    },
    "tpm" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.tpm.Service",
        member_name: "device",
    },
    "trippoint" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.trippoint.Service",
        member_name: "trippoint",
    },
    "usb-device" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.usb.device.Service",
        member_name: "device",
    },
    "usb-tester" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.usb.tester.Service",
        member_name: "device",
    },
    "virtual-bus-test" => ServiceEntry {
        state: State::DevfsAndService,
        service_name: "fuchsia.hardware.usb.virtualbustest.Service",
        member_name: "device",
    },
};

pub static CLASSES_THAT_ASSUME_ORDERING: phf::Set<&'static str> = phf_set! {
    "adc",
    "block",
    "goldfish-address-space",
    "goldfish-control",
    "goldfish-pipe",
    "ot-radio",
    "temperature",
    "thermal",
};

pub static CLASSES_THAT_ALLOW_TOPOLOGICAL_PATH: phf::Set<&'static str> = phf_set! {
    "block",
    "devfs_service_test",
    "network",
};
// LINT.ThenChange(//src/devices/bin/driver_manager/devfs/class_names.h)
