// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;

use super::{AssignedUuid, Uuid};

#[rustfmt::skip]
// Generated with a magic regexp: %s/ - uuid: \(......\)\n   name: \(.\+\)\n   id: \(.\+\)\n/(\1, "\2", "\3"),\r/g
// With a tweak for "CO2 Concentration"

#[rustfmt::skip]
pub static CHARACTERISTIC_UUIDS: std::sync::LazyLock<HashMap<Uuid, AssignedUuid>> = std::sync::LazyLock::new(|| {
    assigned_uuid_map!(
          (0x2900, "Characteristic Extended Properties", "org.bluetooth.descriptor.gatt.characteristic_extended_properties"),
          (0x2901, "Characteristic User Description", "org.bluetooth.descriptor.gatt.characteristic_user_description"),
          (0x2902, "Client Characteristic Configuration", "org.bluetooth.descriptor.gatt.client_characteristic_configuration"),
          (0x2903, "Server Characteristic Configuration", "org.bluetooth.descriptor.gatt.server_characteristic_configuration"),
          (0x2904, "Characteristic Presentation Format", "org.bluetooth.descriptor.gatt.characteristic_presentation_format"),
          (0x2905, "Characteristic Aggregate Format", "org.bluetooth.descriptor.gatt.characteristic_aggregate_format"),
          (0x2906, "Valid Range", "org.bluetooth.descriptor.valid_range"),
          (0x2907, "External Report Reference", "org.bluetooth.descriptor.external_report_reference"),
          (0x2908, "Report Reference", "org.bluetooth.descriptor.report_reference"),
          (0x2909, "Number of Digitals", "org.bluetooth.descriptor.number_of_digitals"),
          (0x290A, "Value Trigger Setting", "org.bluetooth.descriptor.value_trigger_setting"),
          (0x290B, "Environmental Sensing Configuration", "org.bluetooth.descriptor.es_configuration"),
          (0x290C, "Environmental Sensing Measurement", "org.bluetooth.descriptor.es_measurement"),
          (0x290D, "Environmental Sensing Trigger Setting", "org.bluetooth.descriptor.es_trigger_setting"),
          (0x290E, "Time Trigger Setting", "org.bluetooth.descriptor.time_trigger_setting"),
          (0x290F, "Complete BR-EDR Transport Block Data", "org.bluetooth.descriptor.complete_br_edr_transport_block_data"),
          (0x2910, "Observation Schedule", "org.bluetooth.descriptor.observation_schedule"),
          (0x2911, "Valid Range and Accuracy", "org.bluetooth.descriptor.valid_range_accuracy"),
    )
});
