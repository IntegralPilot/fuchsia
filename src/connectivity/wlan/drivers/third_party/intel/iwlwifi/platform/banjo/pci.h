// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file contains copies of banjo definitions that were auto generated from
// fuchsia.hardware.pci. Since banjo is being deprecated, we are making a local copy of
// defines that the driver relies upon. fxbug.dev/104598 is the tracking bug to remove the usage
// of platforms/banjo/*.h files.

// WARNING: DO NOT ADD MORE DEFINITIONS TO THIS FILE

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_BANJO_PCI_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_BANJO_PCI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

__BEGIN_CDECLS
typedef struct pci_interrupt_modes pci_interrupt_modes_t;
typedef uint8_t pci_interrupt_mode_t;
#define PCI_INTERRUPT_MODE_DISABLED UINT8_C(0)
#define PCI_INTERRUPT_MODE_LEGACY UINT8_C(1)
#define PCI_INTERRUPT_MODE_LEGACY_NOACK UINT8_C(2)
#define PCI_INTERRUPT_MODE_MSI UINT8_C(3)
#define PCI_INTERRUPT_MODE_MSI_X UINT8_C(4)
#define PCI_INTERRUPT_MODE_COUNT UINT8_C(5)
#define PCI_CONFIG_SUBSYSTEM_ID UINT16_C(0x2e)

// Returned by |GetInterruptModes|. Contains the number of interrupts supported
// by a given PCI device interrupt mode. 0 is returned for a mode if
// unsupported.
struct pci_interrupt_modes {
  // |True| if the device supports a legacy interrupt.
  bool has_legacy;
  // The number of Message-Signaled interrupted supported. Will be in the
  // range of [0, 0x8) depending on device support.
  uint8_t msi_count;
  // The number of MSI-X interrupts supported. Will be in the range of [0,
  // 0x800), depending on device and platform support.
  uint16_t msix_count;
};

// Device specific information from a device's configuration header.
// PCI Local Bus Specification v3, chapter 6.1.
typedef struct pci_device_info pci_device_info_t;
struct pci_device_info {
  // Device identification information.
  uint16_t vendor_id;
  uint16_t device_id;
  uint8_t base_class;
  uint8_t sub_class;
  uint8_t program_interface;
  uint8_t revision_id;
  // Information pertaining to the device's location in the bus topology.
  uint8_t bus_id;
  uint8_t dev_id;
  uint8_t func_id;
  uint8_t padding1;
};
__END_CDECLS

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_BANJO_PCI_H_
