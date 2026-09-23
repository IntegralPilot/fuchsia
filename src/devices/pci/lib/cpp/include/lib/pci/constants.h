// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_PCI_LIB_CPP_INCLUDE_LIB_PCI_CONSTANTS_H_
#define SRC_DEVICES_PCI_LIB_CPP_INCLUDE_LIB_PCI_CONSTANTS_H_

#include <stdint.h>

namespace pci {

inline constexpr uint32_t kMaxBuses = 256;
inline constexpr uint32_t kMaxDevicesPerBus = 32;
inline constexpr uint32_t kMaxFunctionsPerDevice = 8;
inline constexpr uint32_t kMaxFunctionsPerBus = kMaxDevicesPerBus * kMaxFunctionsPerDevice;

inline constexpr uint32_t kConfigHeaderSize = 64;
inline constexpr uint32_t kBaseConfigSize = 256;
inline constexpr uint32_t kExtendedConfigSize = 4096;
inline constexpr uint32_t kEcamBytesPerBus = kExtendedConfigSize * kMaxFunctionsPerBus;

inline constexpr uint32_t kBarRegsPerBridge = 2;
inline constexpr uint32_t kMaxBarCount = 6;

inline constexpr uint32_t kMaxLegacyIrqPins = 4;
// Per PCI Spec 3.0 section 6.2.4 "Interrupt Line", a PCI function can only use one legacy IRQ pin.
inline constexpr uint32_t kLegacyInterruptCount = 1;
inline constexpr uint32_t kMaxMsiIrqs = 32;
inline constexpr uint32_t kMaxMsixIrqs = 2048;

inline constexpr uint16_t kInvalidVendorId = 0xFFFF;

// clang-format off

/**
 * The maximum possible number of standard capabilities for a PCI
 * device/function is 48.  This comes from the facts that...
 *
 * ++ There are 256 bytes in the standard configuration space.
 * ++ The first 64 bytes are used by the standard configuration header, leaving
 *    192 bytes for capabilities.
 * ++ Even though the capability header is only 2 bytes long, it must be aligned
 *    on a 4 byte boundary.  The means that one can pack (at most) 192 / 4 == 48
 *    properly aligned standard PCI capabilities.
 *
 * Similar logic may be applied to extended capabilities which must also be 4
 * byte aligned, but exist in the region after the standard configuration block.
 */
inline constexpr uint32_t kCapabilityAlignment     = 4u;
inline constexpr uint32_t kCapPtrMinValid           = kConfigHeaderSize;
inline constexpr uint32_t kCapPtrMaxValid           = kBaseConfigSize - kCapabilityAlignment;
inline constexpr uint32_t kPcieExtCapBasePtr        = kBaseConfigSize;
inline constexpr uint32_t kPcieExtCapPtrMinValid    = kBaseConfigSize;
inline constexpr uint32_t kPcieExtCapPtrMaxValid    = kExtendedConfigSize - kCapabilityAlignment;

/*
 * PCI BAR register masks
 */
inline constexpr uint32_t kBarIoTypeMask           = 0x00000001;
inline constexpr uint32_t kBarIoTypeMmio           = 0x00000000;
inline constexpr uint32_t kBarIoTypePio            = 0x00000001;
inline constexpr uint32_t kBarMmioTypeMask         = 0x00000006;
inline constexpr uint32_t kBarMmioType64Bit        = 0x00000004;
inline constexpr uint32_t kBarMmioPrefetchMask     = 0x00000008;
inline constexpr uint32_t kBarMmioAddrMask         = 0xFFFFFFF0;
inline constexpr uint32_t kBarPioAddrMask          = 0xFFFFFFFC;

// clang-format on

}  // namespace pci

#endif  // SRC_DEVICES_PCI_LIB_CPP_INCLUDE_LIB_PCI_CONSTANTS_H_
