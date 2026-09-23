// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_PCI_DRIVERS_PCI_COMPOSITE_H_
#define SRC_DEVICES_PCI_DRIVERS_PCI_COMPOSITE_H_

#include <ddktl/device.h>

namespace pci {

struct CompositeInfo {
  uint32_t vendor_id;
  uint32_t device_id;
  uint32_t class_id;
  uint32_t subclass;
  uint32_t program_interface;
  uint32_t revision_id;

  uint32_t bus_id;
  uint32_t dev_id;
  uint32_t func_id;

  bool has_acpi;
};

ddk::CompositeNodeSpec CreateCompositeNodeSpec(const CompositeInfo& info);

}  // namespace pci

#endif  // SRC_DEVICES_PCI_DRIVERS_PCI_COMPOSITE_H_
