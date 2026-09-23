// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "query.h"

#include <endian.h>
#include <fidl/fuchsia.hardware.ufs/cpp/wire_types.h>
#include <lib/fidl/cpp/wire/vector_view.h>

#include <cstdlib>
#include <span>
#include <type_traits>

#include <safemath/safe_conversions.h>

#include "src/lib/files/file.h"

namespace fufs = fuchsia_hardware_ufs::wire;
namespace {
struct DescriptorField {
  std::string name;
  uint32_t offset;
  uint8_t size;
};

// UFS version 3.1, 14.1.4.2 Device Descriptor
const struct DescriptorField device_desc[] = {
    {.name = "bLength", .offset = 0x0, .size = 1},
    {.name = "bDescriptorIDN", .offset = 0x1, .size = 1},
    {.name = "bDevice", .offset = 0x2, .size = 1},
    {.name = "bDeviceClass", .offset = 0x3, .size = 1},
    {.name = "bDeviceSubClass", .offset = 0x4, .size = 1},
    {.name = "bProtocol", .offset = 0x5, .size = 1},
    {.name = "bNumberLU", .offset = 0x6, .size = 1},
    {.name = "bNumberWLU", .offset = 0x7, .size = 1},
    {.name = "bBootEnable", .offset = 0x8, .size = 1},
    {.name = "bDescrAccessEn", .offset = 0x9, .size = 1},
    {.name = "bInitPowerMode", .offset = 0xA, .size = 1},
    {.name = "bHighPriorityLUN", .offset = 0xB, .size = 1},
    {.name = "bSecureRemovalType", .offset = 0xC, .size = 1},
    {.name = "bSecurityLU", .offset = 0xD, .size = 1},
    {.name = "bBackgroundOpsTermLat", .offset = 0xE, .size = 1},
    {.name = "bInitActiveICCLevel", .offset = 0xF, .size = 1},
    {.name = "wSpecVersion", .offset = 0x10, .size = 2},
    {.name = "wManufactureDate", .offset = 0x12, .size = 2},
    {.name = "iManufacturerName", .offset = 0x14, .size = 1},
    {.name = "iProductName", .offset = 0x15, .size = 1},
    {.name = "iSerialNumber", .offset = 0x16, .size = 1},
    {.name = "iOemID", .offset = 0x17, .size = 1},
    {.name = "wManufacturerID", .offset = 0x18, .size = 2},
    {.name = "bUD0BaseOffset", .offset = 0x1A, .size = 1},
    {.name = "bUDConfigPLength", .offset = 0x1B, .size = 1},
    {.name = "bDeviceRTTCap", .offset = 0x1C, .size = 1},
    {.name = "wPeriodicRTCUpdate", .offset = 0x1D, .size = 2},
    {.name = "bUFSFeaturesSupport", .offset = 0x1F, .size = 1},
    {.name = "bFFUTimeout", .offset = 0x20, .size = 1},
    {.name = "bQueueDepth", .offset = 0x21, .size = 1},
    {.name = "wDeviceVersion", .offset = 0x22, .size = 2},
    {.name = "bNumSecureWPArea", .offset = 0x24, .size = 1},
    {.name = "dPSAMaxDataSize", .offset = 0x25, .size = 4},
    {.name = "bPSAStateTimeout", .offset = 0x29, .size = 1},
    {.name = "iProductRevisionLevel", .offset = 0x2A, .size = 1},
    {.name = "Reserved", .offset = 0x2B, .size = 5},
    {.name = "Reserved", .offset = 0x30, .size = 16},
    // UFS Host Performance Booster (HPB) Extension Standard (0x40 to 0x42)
    {.name = "wHPBVersion", .offset = 0x40, .size = 2},
    {.name = "bHPBControl", .offset = 0x42, .size = 1},
    {.name = "Reserved", .offset = 0x43, .size = 12},
    {.name = "dExtendedUFSFeaturesSupport", .offset = 0x4F, .size = 4},
    {.name = "bWriteBoosterBufferPreserveUserSpaceEn", .offset = 0x53, .size = 1},
    {.name = "bWriteBoosterBufferType", .offset = 0x54, .size = 1},
    {.name = "dNumSharedWriteBoosterBufferAllocUnits", .offset = 0x55, .size = 4},
};

// UFS version 3.1, 14.1.4.3 Configuration Descriptor
// Configuration Descr. Header and Device Descr.Conf.Parameters (INDEX=00h)
const struct DescriptorField device_desc_config_params[] = {
    {.name = "bLength", .offset = 0x00, .size = 1},
    {.name = "bDescriptorIDN", .offset = 0x01, .size = 1},
    {.name = "bConfDescContinue", .offset = 0x02, .size = 1},
    {.name = "bBootEnable", .offset = 0x03, .size = 1},
    {.name = "bDescrAccessEn", .offset = 0x04, .size = 1},
    {.name = "bInitPowerMode", .offset = 0x05, .size = 1},
    {.name = "bHighPriorityLUN", .offset = 0x06, .size = 1},
    {.name = "bSecureRemovalType", .offset = 0x07, .size = 1},
    {.name = "bInitActiveICCLevel", .offset = 0x08, .size = 1},
    {.name = "wPeriodicRTCUpdate", .offset = 0x09, .size = 2},
    // UFS Host Performance Booster (HPB) Extension Standard (0xB)
    {.name = "bHPBControl", .offset = 0x0B, .size = 1},
    {.name = "bRPMBRegionEnable", .offset = 0x0C, .size = 1},
    {.name = "bRPMBRegion1Size", .offset = 0x0D, .size = 1},
    {.name = "bRPMBRegion2Size", .offset = 0x0E, .size = 1},
    {.name = "bRPMBRegion3Size", .offset = 0x0F, .size = 1},
    {.name = "bWriteBoosterBufferPreserveUserSpaceEn", .offset = 0x10, .size = 1},
    {.name = "bWriteBoosterBufferType", .offset = 0x11, .size = 1},
    {.name = "dNumSharedWriteBoosterBufferAllocUnits", .offset = 0x12, .size = 4},
};

// Configuration Descr. Header with INDEX = 01h/02h/03h
const struct DescriptorField config_desc_header[] = {
    {.name = "bLength", .offset = 0x00, .size = 1},
    {.name = "bDescriptorIDN", .offset = 0x01, .size = 1},
    {.name = "bConfDescContinue", .offset = 0x02, .size = 1},
    {.name = "Reserved", .offset = 0x03, .size = 19},
};

// Unit Descriptor configurable parameters
const struct DescriptorField unit_desc_config_params[] = {
    {.name = "bLUEnable", .offset = 0x00, .size = 1},
    {.name = "bBootLunID", .offset = 0x01, .size = 1},
    {.name = "bLUWriteProtect", .offset = 0x02, .size = 1},
    {.name = "bMemoryType", .offset = 0x03, .size = 1},
    {.name = "dNumAllocUnits", .offset = 0x04, .size = 4},
    {.name = "bDataReliability", .offset = 0x08, .size = 1},
    {.name = "bLogicalBlockSize", .offset = 0x09, .size = 1},
    {.name = "bProvisioningType", .offset = 0x0A, .size = 1},
    {.name = "wContextCapabilities", .offset = 0x0B, .size = 2},
    {.name = "Reserved", .offset = 0x0D, .size = 3},
    // UFS Host Performance Booster (HPB) Extension Standard (0x10 to 0x14)
    {.name = "wLUMaxActiveHPBRegions", .offset = 0x10, .size = 2},
    {.name = "wHPBPinnedRegionStartIdx", .offset = 0x12, .size = 2},
    {.name = "wNumHPBPinnedRegions", .offset = 0x14, .size = 2},
    {.name = "dLUNumWriteBoosterBufferAllocUnits", .offset = 0x16, .size = 4},
};

// UFS version 3.1, 14.1.4.4 Geometry Descriptor
const struct DescriptorField geometry_desc[] = {
    {.name = "bLength", .offset = 0x00, .size = 1},
    {.name = "bDescriptorIDN", .offset = 0x01, .size = 1},
    {.name = "bMediaTechnology", .offset = 0x02, .size = 1},
    {.name = "Reserved", .offset = 0x03, .size = 1},
    {.name = "qTotalRawDeviceCapacity", .offset = 0x04, .size = 8},
    {.name = "bMaxNumberLU", .offset = 0x0C, .size = 1},
    {.name = "dSegmentSize", .offset = 0x0D, .size = 4},
    {.name = "bAllocationUnitSize", .offset = 0x11, .size = 1},
    {.name = "bMinAddrBlockSize", .offset = 0x12, .size = 1},
    {.name = "bOptimalReadBlockSize", .offset = 0x13, .size = 1},
    {.name = "bOptimalWriteBlockSize", .offset = 0x14, .size = 1},
    {.name = "bMaxInBufferSize", .offset = 0x15, .size = 1},
    {.name = "bMaxOutBufferSize", .offset = 0x16, .size = 1},
    {.name = "bRPMB_ReadWriteSize", .offset = 0x17, .size = 1},
    {.name = "bDynamicCapacityResourcePolicy", .offset = 0x18, .size = 1},
    {.name = "bDataOrdering", .offset = 0x19, .size = 1},
    {.name = "bMaxContexIDNumber", .offset = 0x1A, .size = 1},
    {.name = "bSysDataTagUnitSize", .offset = 0x1B, .size = 1},
    {.name = "bSysDataTagResSize", .offset = 0x1C, .size = 1},
    {.name = "bSupportedSecRTypes", .offset = 0x1D, .size = 1},
    {.name = "wSupportedMemoryTypes", .offset = 0x1E, .size = 2},
    {.name = "dSystemCodeMaxNAllocU", .offset = 0x20, .size = 4},
    {.name = "wSystemCodeCapAdjFac", .offset = 0x24, .size = 2},
    {.name = "dNonPersistMaxNAllocU", .offset = 0x26, .size = 4},
    {.name = "wNonPersistCapAdjFac", .offset = 0x2A, .size = 2},
    {.name = "dEnhanced1MaxNAllocU", .offset = 0x2C, .size = 4},
    {.name = "wEnhanced1CapAdjFac", .offset = 0x30, .size = 2},
    {.name = "dEnhanced2MaxNAllocU", .offset = 0x32, .size = 4},
    {.name = "wEnhanced2CapAdjFac", .offset = 0x36, .size = 2},
    {.name = "dEnhanced3MaxNAllocU", .offset = 0x38, .size = 4},
    {.name = "wEnhanced3CapAdjFac", .offset = 0x3C, .size = 2},
    {.name = "dEnhanced4MaxNAllocU", .offset = 0x3E, .size = 4},
    {.name = "wEnhanced4CapAdjFac", .offset = 0x42, .size = 2},
    {.name = "dOptimalLogicalBlockSize", .offset = 0x44, .size = 4},
    // UFS Host Performance Booster (HPB) Extension Standard (0x48 to 0x4B)
    {.name = "bHPBRegionSize", .offset = 0x48, .size = 1},
    {.name = "bHPBNumberLU", .offset = 0x49, .size = 1},
    {.name = "bHPBSubRegionSize", .offset = 0x4A, .size = 1},
    {.name = "wDeviceMaxActiveHPBRegions", .offset = 0x4B, .size = 2},
    {.name = "Reserved", .offset = 0x4D, .size = 2},
    {.name = "dWriteBoosterBufferMaxNAllocUnits", .offset = 0x4F, .size = 4},
    {.name = "bDeviceMaxWriteBoosterLUs", .offset = 0x53, .size = 1},
    {.name = "bWriteBoosterBufferCapAdjFac", .offset = 0x54, .size = 1},
    {.name = "bSupportedWriteBoosterBufferUserSpaceReductionTypes", .offset = 0x55, .size = 1},
    {.name = "bSupportedWriteBoosterBufferTypes", .offset = 0x56, .size = 1},
};

// UFS version 3.1, 14.1.4.5 Unit Descriptor
const struct DescriptorField unit_desc[] = {
    {.name = "bLength", .offset = 0x00, .size = 1},
    {.name = "bDescriptorIDN", .offset = 0x01, .size = 1},
    {.name = "bUnitIndex", .offset = 0x02, .size = 1},
    {.name = "bLUEnable", .offset = 0x03, .size = 1},
    {.name = "bBootLunID", .offset = 0x04, .size = 1},
    {.name = "bLUWriteProtect", .offset = 0x05, .size = 1},
    {.name = "bLUQueueDepth", .offset = 0x06, .size = 1},
    {.name = "bPSASensitive", .offset = 0x07, .size = 1},
    {.name = "bMemoryType", .offset = 0x08, .size = 1},
    {.name = "bDataReliability", .offset = 0x09, .size = 1},
    {.name = "bLogicalBlockSize", .offset = 0x0A, .size = 1},
    {.name = "qLogicalBlockCount", .offset = 0x0B, .size = 8},
    {.name = "dEraseBlockSize", .offset = 0x13, .size = 4},
    {.name = "bProvisioningType", .offset = 0x17, .size = 1},
    {.name = "qPhyMemResourceCount", .offset = 0x18, .size = 8},
    {.name = "wContextCapabilities", .offset = 0x20, .size = 2},
    {.name = "bLargeUnitGranularity_M1", .offset = 0x22, .size = 1},
    // UFS Host Performance Booster (HPB) Extension Standard (0x23 to 0x27)
    {.name = "wLUMaxActiveHPBRegions", .offset = 0x23, .size = 2},
    {.name = "wHPBPinnedRegionStartIdx", .offset = 0x25, .size = 2},
    {.name = "wNumHPBPinnedRegions", .offset = 0x27, .size = 2},
    {.name = "dLUNumWriteBoosterBufferAllocUnits", .offset = 0x29, .size = 4},
};

// UFS version 3.1, 14.1.4.6 RPMB Unit Descriptor
const struct DescriptorField rpmb_unit_desc[] = {
    {.name = "bLength", .offset = 0x00, .size = 1},
    {.name = "bDescriptorIDN", .offset = 0x01, .size = 1},
    {.name = "bUnitIndex", .offset = 0x02, .size = 1},
    {.name = "bLUEnable", .offset = 0x03, .size = 1},
    {.name = "bBootLunID", .offset = 0x04, .size = 1},
    {.name = "bLUWriteProtect", .offset = 0x05, .size = 1},
    {.name = "bLUQueueDepth", .offset = 0x06, .size = 1},
    {.name = "bPSASensitive", .offset = 0x07, .size = 1},
    {.name = "bMemoryType", .offset = 0x08, .size = 1},
    {.name = "bRPMBRegionEnable", .offset = 0x09, .size = 1},
    {.name = "bLogicalBlockSize", .offset = 0x0A, .size = 1},
    {.name = "qLogicalBlockCount", .offset = 0x0B, .size = 8},
    {.name = "bRPMBRegion0Size", .offset = 0x13, .size = 1},
    {.name = "bRPMBRegion1Size", .offset = 0x14, .size = 1},
    {.name = "bRPMBRegion2Size", .offset = 0x15, .size = 1},
    {.name = "bRPMBRegion3Size", .offset = 0x16, .size = 1},
    {.name = "bProvisioningType", .offset = 0x17, .size = 1},
    {.name = "qPhyMemResourceCount", .offset = 0x18, .size = 8},
    {.name = "Reserved", .offset = 0x20, .size = 3},
};

// UFS version 3.1, 14.1.4.7 Power Parameters Descriptor
const struct DescriptorField power_parameters_desc[] = {
    {.name = "bLength", .offset = 0x00, .size = 1},
    {.name = "bDescriptorIDN", .offset = 0x01, .size = 1},
    {.name = "wActiveICCLevelsVCC", .offset = 0x02, .size = 32},
    {.name = "wActiveICCLevelsVCCQ", .offset = 0x22, .size = 32},
    {.name = "wActiveICCLevelsVCCQ2", .offset = 0x42, .size = 32},
};

// UFS version 3.1, 14.1.4.8 Interconnect Descriptor
const struct DescriptorField interconnect_desc[] = {
    {.name = "bLength", .offset = 0x00, .size = 1},
    {.name = "bDescriptorIDN", .offset = 0x01, .size = 1},
    {.name = "bcdUniproVersion", .offset = 0x02, .size = 2},
    {.name = "bcdMphyVersion", .offset = 0x04, .size = 2},
};

// UFS version 3.1, 14.1.4.9 ~ 13 String Descriptor
const struct DescriptorField string_desc[] = {
    {.name = "bLength", .offset = 0x00, .size = 1},
    {.name = "bDescriptorIDN", .offset = 0x01, .size = 1},
    {.name = "UC", .offset = 0x02, .size = 126},
};

// String supports up to 126 UNICODE characters.
constexpr uint32_t kStringValueMaxlen = 126;

// UFS version 3.1, 14.1.4.14 Device Health Descriptor
const struct DescriptorField device_health_desc[] = {
    {.name = "bLength", .offset = 0x00, .size = 1},
    {.name = "bDescriptorIDN", .offset = 0x01, .size = 1},
    {.name = "bPreEOLInfo", .offset = 0x02, .size = 1},
    {.name = "bDeviceLifeTimeEstA", .offset = 0x03, .size = 1},
    {.name = "bDeviceLifeTimeEstB", .offset = 0x04, .size = 1},
    {.name = "VendorPropInfo", .offset = 0x05, .size = 32},
    {.name = "dRefreshTotalCount", .offset = 0x25, .size = 4},
    {.name = "dRefreshProgress", .offset = 0x29, .size = 4},
};

enum AccessProperties {
  READ = (1 << 0),
  WRITE_ONLY = (1 << 1),
  READ_ONLY = (1 << 2),
  WRITE_ONCE = (1 << 3),
  PERSISTENT = (1 << 4),
  VOLATILE = (1 << 5),
  SET_ONLY = (1 << 6),
  POWER_ON_RESET = (1 << 7),
  INVALID = 0,
};

struct FlagField {
  std::string name;
  uint32_t access_property;
};

const struct FlagField kFlags[] = {
    {.name = "Reserved", .access_property = INVALID},
    {.name = "fDeviceInit", .access_property = (READ | SET_ONLY)},
    {.name = "fPermanentWPEn", .access_property = (READ | WRITE_ONCE)},
    {.name = "fPowerOnWPEn", .access_property = (READ | POWER_ON_RESET)},
    {.name = "fBackgroundOpsEn", .access_property = (READ | VOLATILE)},
    {.name = "fDeviceLifeSpanModeEn", .access_property = (READ | VOLATILE)},
    {.name = "fPurgeEnable", .access_property = (WRITE_ONLY | VOLATILE)},
    {.name = "fRefreshEnable", .access_property = (WRITE_ONLY | VOLATILE)},
    {.name = "fPhyResourceRemoval", .access_property = (READ | PERSISTENT)},
    {.name = "fBusyRTC", .access_property = READ_ONLY},
    {.name = "Reserved", .access_property = INVALID},
    {.name = "Reserved", .access_property = INVALID},
    {.name = "Reserved", .access_property = INVALID},
    {.name = "Reserved", .access_property = INVALID},
    {.name = "fWriteBoosterEn", .access_property = (READ | VOLATILE)},
    {.name = "fWBBufferFlushEn", .access_property = (READ | VOLATILE)},
    {.name = "fWBBufferFlushDuringHibernate", .access_property = (READ | VOLATILE)},
    {.name = "Reserved", .access_property = INVALID},
};

struct AttributeField {
  std::string name;
  uint32_t access_property;
  uint32_t size;
};

const struct AttributeField kAttrs[] = {
    {.name = "bBootLunEn", .access_property = (READ | PERSISTENT), .size = 1},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "bCurrentPowerMode", .access_property = READ_ONLY, .size = 1},
    {.name = "bActiveICCLevel", .access_property = (READ | VOLATILE), .size = 1},
    {.name = "bOutOfOrderDataEn", .access_property = (READ | WRITE_ONCE), .size = 1},
    {.name = "bBackgroundOpStatus", .access_property = READ_ONLY, .size = 1},
    {.name = "bPurgeStatus", .access_property = READ_ONLY, .size = 1},
    {.name = "bMaxDataInSize", .access_property = (READ | PERSISTENT), .size = 1},
    {.name = "bMaxDataOutSize", .access_property = (READ | PERSISTENT), .size = 1},
    {.name = "dDynCapNeeded", .access_property = READ_ONLY, .size = 4},
    {.name = "bRefClkFreq", .access_property = (READ | PERSISTENT), .size = 1},
    {.name = "bConfigDescrLock", .access_property = (READ | WRITE_ONCE), .size = 1},
    {.name = "bMaxNumOfRTT", .access_property = (READ | PERSISTENT), .size = 1},
    {.name = "wExceptionEventControl", .access_property = (READ | VOLATILE), .size = 2},
    {.name = "wExceptionEventStatus", .access_property = READ_ONLY, .size = 2},
    {.name = "dSecondsPassed", .access_property = (WRITE_ONLY | VOLATILE), .size = 4},
    {.name = "wContextConf", .access_property = (READ | VOLATILE), .size = 2},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "bDeviceFFUStatus", .access_property = READ_ONLY, .size = 1},
    {.name = "bPSAState", .access_property = (READ | PERSISTENT), .size = 1},
    {.name = "dPSADataSize", .access_property = (READ | PERSISTENT), .size = 4},
    {.name = "bRefClkGatingWaitTime", .access_property = READ_ONLY, .size = 1},
    {.name = "bDeviceCaseRoughTemperaure", .access_property = READ_ONLY, .size = 1},
    {.name = "bDeviceTooHighTempBoundary", .access_property = READ_ONLY, .size = 1},
    {.name = "bDeviceTooLowTempBoundary", .access_property = READ_ONLY, .size = 1},
    {.name = "bThrottlingStatus", .access_property = READ_ONLY, .size = 1},
    {.name = "bWriteBoosterBufferFlushStatus", .access_property = READ_ONLY, .size = 1},
    {.name = "bAvailableWriteBoosterBufferSize", .access_property = READ_ONLY, .size = 1},
    {.name = "bWriteBoosterBufferLifeTimeEst", .access_property = READ_ONLY, .size = 1},
    {.name = "dCurrentWriteBoosterBufferSize", .access_property = READ_ONLY, .size = 4},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "Reserved", .access_property = INVALID, .size = 0},
    {.name = "bRefreshStatus", .access_property = READ_ONLY, .size = 1},
    {.name = "bRefreshFreq", .access_property = (READ | PERSISTENT), .size = 1},
    {.name = "bRefreshUnit", .access_property = (READ | PERSISTENT), .size = 1},
    {.name = "bRefreshMethod", .access_property = (READ | PERSISTENT), .size = 1},
};

void PrintDescriptorField(cpp20::span<uint8_t> data_buffer,
                          cpp20::span<const DescriptorField> fields) {
  printf("%-55s   %-8s  %-8s\n", "[Name]", "[Offset]", "[Value]");
  for (const auto& field : fields) {
    printf("%-55s  |  0x%02X  |  ", field.name.c_str(), field.offset);
    switch (field.size) {
      case 1:  // Byte
        printf("0x%02X \n", data_buffer[field.offset]);
        break;
      case 2: {  // Word
        uint16_t value;
        std::memcpy(&value, &data_buffer[field.offset], sizeof(value));
        value = be16toh(value);
        printf("0x%04X \n", value);
        break;
      }
      case 4: {  // Dword
        uint32_t value;
        std::memcpy(&value, &data_buffer[field.offset], sizeof(value));
        value = be32toh(value);
        printf("0x%08X \n", value);
        break;
      }
      case 8: {  // Qword
        uint64_t value;
        std::memcpy(&value, &data_buffer[field.offset], sizeof(value));
        value = be64toh(value);
        printf("0x%016lX \n", value);
        break;
      }
      default: {
        std::span<uint8_t> sub_buffer = data_buffer.subspan(field.offset, field.size);
        printf("0x");
        for (uint8_t byte : sub_buffer) {
          printf("%02X", byte);
        }
        printf(" \n");
        break;
      }
    }
  }
}

void PrintConfigDescriptor(cpp20::span<uint8_t> data_buffer, uint8_t index, uint8_t base_offset,
                           uint8_t length) {
  const DescriptorField* fields = index == 0 ? device_desc_config_params : config_desc_header;
  size_t fields_size =
      index == 0 ? std::size(device_desc_config_params) : std::size(config_desc_header);

  // Print Configuration Descriptor header fields
  PrintDescriptorField(data_buffer, {fields, fields_size});

  // Print the Unit Descriptors managed by this Configuration Descriptor.
  // Index specifies the LU range: 00h (LU 0-7), 01h (LU 8-15), 02h (LU 16-23), 03h (LU 24-31).
  for (int i = 0; i < 8; ++i) {
    cpp20::span<uint8_t> sub_buffer = data_buffer.subspan(base_offset, length);
    PrintDescriptorField(sub_buffer, {unit_desc_config_params, std::size(unit_desc_config_params)});
    base_offset += length;
  }
}

void PrintPowerDescriptor(cpp20::span<uint8_t> data_buffer) {
  constexpr size_t vcc_base_offset = 2;
  PrintDescriptorField(data_buffer.subspan(0, vcc_base_offset),
                       {power_parameters_desc, vcc_base_offset});

  for (size_t i = vcc_base_offset; i < std::size(power_parameters_desc); ++i) {
    const DescriptorField& field = power_parameters_desc[i];
    for (uint32_t off = field.offset, index = 0; off < field.offset + field.size;
         off += 2, index++) {
      std::string namestr = field.name + "[" + std::to_string(index) + "]";
      printf("%-55s  |  0x%02X  |  ", namestr.c_str(), off);
      uint16_t value = be16toh(*(reinterpret_cast<const uint16_t*>(&data_buffer[off])));
      printf("0x%04X (%u)\n", value, value);
    }
  }
}

void PrintStringDescriptor(cpp20::span<uint8_t> data_buffer) {
  constexpr size_t str_base_offset = 2;
  PrintDescriptorField(data_buffer.subspan(0, str_base_offset), {string_desc, str_base_offset});

  size_t desc_size = data_buffer[0];
  if (desc_size < str_base_offset)
    return;
  size_t str_size = desc_size - str_base_offset;
  const DescriptorField& field = string_desc[str_base_offset];

  // Print Unicode characters individually
  for (uint32_t off = field.offset, index = 0; off < str_size; off += 2, index++) {
    std::string namestr = field.name + "[" + std::to_string(index) + "]";
    printf("%-55s  |  0x%04X  |  ", namestr.c_str(), off);
    uint16_t char_value = be16toh(*(reinterpret_cast<const uint16_t*>(&data_buffer[off])));
    printf("0x%04X \n", char_value);
  }
}

void PrintDescriptor(fufs::DescriptorType type, cpp20::span<uint8_t> data_buffer, uint8_t index) {
  switch (type) {
    case fufs::DescriptorType::kDevice:
      PrintDescriptorField(data_buffer, {device_desc, std::size(device_desc)});
      break;
    case fufs::DescriptorType::kUnit: {
      const DescriptorField* fields = index == 0xc4 ? rpmb_unit_desc : unit_desc;
      size_t fields_size = index == 0xc4 ? std::size(rpmb_unit_desc) : std::size(unit_desc);
      PrintDescriptorField(data_buffer, {fields, fields_size});
    } break;
    case fufs::DescriptorType::kInterconnect:
      PrintDescriptorField(data_buffer, {interconnect_desc, std::size(interconnect_desc)});
      break;
    case fufs::DescriptorType::kString:
      PrintStringDescriptor(data_buffer);
      break;
    case fufs::DescriptorType::kGeometry:
      PrintDescriptorField(data_buffer, {geometry_desc, std::size(geometry_desc)});
      break;
    case fufs::DescriptorType::kPower:
      PrintPowerDescriptor(data_buffer);
      break;
    case fufs::DescriptorType::kDeviceHealth:
      PrintDescriptorField(data_buffer, {device_health_desc, std::size(device_health_desc)});
      break;
    default:
      break;
  }
}

inline bool CheckRequiredOption(const std::unordered_map<uint32_t, OptionValue>& options, char opt,
                                const char* long_opt) {
  if (!options.contains(opt)) {
    fprintf(stderr, "error: missing required option -%c/--%s\n", opt, long_opt);
    return false;
  }
  return true;
}

template <typename T>
int ExecuteOperation(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                     const std::unordered_map<uint32_t, OptionValue>& options,
                     int (*handler)(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>&, T,
                                    const std::unordered_map<uint32_t, OptionValue>&)) {
  if (!CheckRequiredOption(options, 't', "type")) {
    return EXIT_FAILURE;
  }

  fidl::Arena arena;
  auto id = fufs::Identifier::Builder(arena);
  id.index(0).selector(0);

  auto set_option_if_valid = [&](char opt, auto setter) -> bool {
    if (options.contains(opt)) {
      auto value = std::get<uint32_t>(options.at(opt));
      if (safemath::IsValueInRangeForNumericType<uint8_t>(value)) {
        setter(static_cast<uint8_t>(value));
      } else {
        fprintf(stderr, "error: invalid argument: '-%c'\n", opt);
        return false;
      }
    }
    return true;
  };

  if (!set_option_if_valid('i', [&](uint8_t v) { id.index(v); })) {
    return EXIT_FAILURE;
  }

  if (!set_option_if_valid('s', [&](uint8_t v) { id.selector(v); })) {
    return EXIT_FAILURE;
  }
  uint8_t type_value = 0;
  if (!set_option_if_valid('t', [&](uint8_t v) { type_value = v; })) {
    return EXIT_FAILURE;
  }

  if constexpr (std::is_same_v<T, fufs::Descriptor>) {
    fufs::DescriptorType desc_type(type_value);
    if (desc_type.IsUnknown()) {
      fprintf(stderr, "error: invalid type idn.\n");
      return EXIT_FAILURE;
    }
    auto desc = fufs::Descriptor::Builder(arena).type(desc_type).identifier(id.Build()).Build();
    return handler(client, desc, options);
  } else if constexpr (std::is_same_v<T, fufs::Flag>) {
    fufs::FlagType flag_type(type_value);
    if (flag_type.IsUnknown()) {
      fprintf(stderr, "error: invalid type idn.\n");
      return EXIT_FAILURE;
    }
    auto flag = fufs::Flag::Builder(arena).type(flag_type).identifier(id.Build()).Build();
    return handler(client, flag, options);
  } else if constexpr (std::is_same_v<T, fufs::Attribute>) {
    fufs::AttributeType attr_type(type_value);
    if (attr_type.IsUnknown()) {
      fprintf(stderr, "error: invalid type idn.\n");
      return EXIT_FAILURE;
    }
    auto attr = fufs::Attribute::Builder(arena).type(attr_type).identifier(id.Build()).Build();
    return handler(client, attr, options);
  }

  return EXIT_FAILURE;
}

template <typename T>
int HandleFidlResult(const fidl::WireResult<T>& result, const char* error_message) {
  if (!result.ok()) {
    fprintf(stderr, "%s (FIDL result error : %s) \n", error_message, result.status_string());
    return EXIT_FAILURE;
  }

  const fit::result response = result.value();
  if (response.is_error()) {
    fprintf(stderr, "error: query error code : %d\n",
            static_cast<uint32_t>(response.error_value()));
    return EXIT_FAILURE;
  }

  return EXIT_SUCCESS;
}

int HandleConfigDescriptor(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                           fufs::Descriptor desc) {
  fidl::Arena arena;

  // Read device descriptor
  auto device_desc = fufs::Descriptor::Builder(arena).type(fufs::DescriptorType::kDevice).Build();
  const fidl::WireResult result = client->ReadDescriptor(device_desc);
  if (HandleFidlResult(result, "error: failed to read descriptor.") != EXIT_SUCCESS) {
    return EXIT_FAILURE;
  }

  // Define the offsets for bUD0BaseOffset and bUDConfigPLength in the device descriptor
  constexpr uint32_t kUD0BaseOffset = 0x1a;
  constexpr uint32_t kUDConfigPLength = 0x1b;

  const fit::result response = result.value();
  // Read the bUD0BaseOffset value from the device descriptor data
  uint8_t base_offset = response->data.at(kUD0BaseOffset);
  // Read the bUDConfigPLength value from the device descriptor data
  uint8_t length = response->data.at(kUDConfigPLength);

  // Read config descriptor
  {
    const fidl::WireResult result = client->ReadDescriptor(desc);
    if (HandleFidlResult(result, "error: failed to read descriptor.") != EXIT_SUCCESS) {
      return EXIT_FAILURE;
    }
  }

  PrintConfigDescriptor(response->data.get(), desc.identifier().index(), base_offset, length);

  return EXIT_SUCCESS;
}

bool CreateStringDescriptor(const std::string& value, std::vector<uint8_t>& write_desc) {
  size_t value_len = value.length();
  if (value_len > kStringValueMaxlen) {
    fprintf(stderr, "error : value is too long.");
    return false;
  }

  // Set the length and type of the descriptor
  write_desc[0] = static_cast<uint8_t>((value_len * 2) + 2);
  write_desc[1] = static_cast<uint8_t>(fufs::DescriptorType::kString);

  // Copy the string data into the descriptor buffer in UTF-16 encoding
  std::u16string utf16_value(value.begin(), value.end());
  for (size_t i = 0, j = 2; i < value_len; ++i, j += 2) {
    char16_t ch = utf16_value[i];
    // Upper byte of the UTF-16 character
    write_desc[j] = static_cast<uint8_t>((ch >> 8) & 0xFF);
    // Lower byte of the UTF-16 character
    write_desc[j + 1] = static_cast<uint8_t>(ch & 0xFF);
  }

  return true;
}

}  // namespace

int HandleReadDescriptor(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                         const std::unordered_map<uint32_t, OptionValue>& options) {
  return ExecuteOperation<fufs::Descriptor>(
      client, options, [](auto& client, auto desc, auto& options) {
        if (desc.type() == fufs::DescriptorType::kConfiguration) {
          return HandleConfigDescriptor(client, desc);
        }

        const fidl::WireResult result = client->ReadDescriptor(desc);
        if (HandleFidlResult(result, "error: failed to read descriptor.") != EXIT_SUCCESS) {
          return EXIT_FAILURE;
        }

        std::vector<uint8_t> buffer(result.value()->data.begin(), result.value()->data.end());
        PrintDescriptor(desc.type(), buffer, desc.identifier().index());

        return EXIT_SUCCESS;
      });
}

int HandleWriteDescriptor(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                          const std::unordered_map<uint32_t, OptionValue>& options) {
  return ExecuteOperation<fufs::Descriptor>(
      client, options, [](auto& client, auto desc, auto& options) {
        if (desc.type() != fufs::DescriptorType::kConfiguration &&
            desc.type() != fufs::DescriptorType::kString) {
          fprintf(stderr, "The descriptor is not writable.\n");
          return EXIT_FAILURE;
        }

        std::vector<uint8_t> write_desc(fufs::kMaxDescriptorSize);
        if (desc.type() == fufs::DescriptorType::kString) {
          if (!CheckRequiredOption(options, 'v', "value")) {
            return EXIT_FAILURE;
          }

          std::string value = std::get<std::string>(options.at('v'));
          if (!CreateStringDescriptor(value, write_desc)) {
            return EXIT_FAILURE;
          }
        } else if (desc.type() == fufs::DescriptorType::kConfiguration) {
          if (!CheckRequiredOption(options, 'f', "file")) {
            return EXIT_FAILURE;
          }

          std::string file_path = std::get<std::string>(options.at('f'));
          if (!(files::ReadFileToVector(file_path, &write_desc))) {
            fprintf(stderr, "error : Cannnot read file : %s\n", file_path.c_str());
            return EXIT_FAILURE;
          };
        }

        const fidl::WireResult result =
            client->WriteDescriptor(desc, fidl::VectorView<uint8_t>::FromExternal(write_desc));

        if (HandleFidlResult(result, "error: failed to read descriptor.") != EXIT_SUCCESS) {
          return EXIT_FAILURE;
        }

        return EXIT_SUCCESS;
      });
}

int HandleReadFlag(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                   const std::unordered_map<uint32_t, OptionValue>& options) {
  return ExecuteOperation<fufs::Flag>(client, options, [](auto& client, auto flag, auto& options) {
    uint8_t type_id = static_cast<uint8_t>(flag.type());
    if (kFlags[type_id].access_property & WRITE_ONLY) {
      fprintf(stderr, "The flag is write only.\n");
      return EXIT_FAILURE;
    }

    const fidl::WireResult result = client->ReadFlag(flag);
    if (HandleFidlResult(result, "error: failed to read flag.") != EXIT_SUCCESS) {
      return EXIT_FAILURE;
    }

    const fit::result response = result.value();
    printf("%s : %d\n", kFlags[type_id].name.c_str(), response->value);

    return EXIT_SUCCESS;
  });
}

int HandleSetFlag(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                  const std::unordered_map<uint32_t, OptionValue>& options) {
  return ExecuteOperation<fufs::Flag>(client, options, [](auto& client, auto flag, auto& options) {
    uint8_t type_id = static_cast<uint8_t>(flag.type());
    if (kFlags[type_id].access_property & READ_ONLY) {
      fprintf(stderr, "The flag is read only.\n");
      return EXIT_FAILURE;
    }

    const fidl::WireResult result = client->SetFlag(flag);
    if (HandleFidlResult(result, "error: failed to set flag.") != EXIT_SUCCESS) {
      return EXIT_FAILURE;
    }

    const fit::result response = result.value();
    printf("%s : %d\n", kFlags[type_id].name.c_str(), response->value);

    return EXIT_SUCCESS;
  });
}

int HandleClearFlag(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                    const std::unordered_map<uint32_t, OptionValue>& options) {
  return ExecuteOperation<fufs::Flag>(client, options, [](auto& client, auto flag, auto& options) {
    uint8_t type_id = static_cast<uint8_t>(flag.type());
    if (kFlags[type_id].access_property & READ_ONLY) {
      fprintf(stderr, "The flag is read only.\n");
      return EXIT_FAILURE;
    }

    const fidl::WireResult result = client->ClearFlag(flag);
    if (HandleFidlResult(result, "error: failed to clear flag.") != EXIT_SUCCESS) {
      return EXIT_FAILURE;
    }

    const fit::result response = result.value();
    printf("%s : %d\n", kFlags[type_id].name.c_str(), response->value);

    return EXIT_SUCCESS;
  });
}

int HandleToggleFlag(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                     const std::unordered_map<uint32_t, OptionValue>& options) {
  return ExecuteOperation<fufs::Flag>(client, options, [](auto& client, auto flag, auto& options) {
    uint8_t type_id = static_cast<uint8_t>(flag.type());
    if (kFlags[type_id].access_property & READ_ONLY) {
      fprintf(stderr, "The flag is read only.\n");
      return EXIT_FAILURE;
    }

    const fidl::WireResult result = client->ToggleFlag(flag);
    if (HandleFidlResult(result, "error: failed to toogle flag.") != EXIT_SUCCESS) {
      return EXIT_FAILURE;
    }

    const fit::result response = result.value();
    printf("%s : %d\n", kFlags[type_id].name.c_str(), response->value);

    return EXIT_SUCCESS;
  });
}

int HandleReadAttribute(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                        const std::unordered_map<uint32_t, OptionValue>& options) {
  return ExecuteOperation<fufs::Attribute>(
      client, options, [](auto& client, auto attr, auto& options) {
        uint8_t type_id = static_cast<uint8_t>(attr.type());
        if (kAttrs[type_id].access_property & WRITE_ONLY) {
          fprintf(stderr, "The attribute is write only.\n");
          return EXIT_FAILURE;
        }

        const fidl::WireResult result = client->ReadAttribute(attr);
        if (HandleFidlResult(result, "error: failed to read attribute.") != EXIT_SUCCESS) {
          return EXIT_FAILURE;
        }
        const fit::result response = result.value();
        printf("value : 0x%04x\n", response->value);

        return EXIT_SUCCESS;
      });
}

int HandleWriteAttribute(const fidl::WireSyncClient<fuchsia_hardware_ufs::Ufs>& client,
                         const std::unordered_map<uint32_t, OptionValue>& options) {
  return ExecuteOperation<fufs::Attribute>(
      client, options, [](auto& client, auto attr, auto& options) {
        uint8_t type_id = static_cast<uint8_t>(attr.type());
        if (kAttrs[type_id].access_property & READ_ONLY) {
          fprintf(stderr, "The attribute is read only.\n");
          return EXIT_FAILURE;
        }

        if (!options.contains('v')) {
          fprintf(stderr, "error: The value is missing for the write attribute.\n");
          return EXIT_FAILURE;
        }

        uint32_t attr_value = std::get<uint32_t>(options.at('v'));
        const fidl::WireResult result = client->WriteAttribute(attr, attr_value);
        if (HandleFidlResult(result, "error: failed to wrtie attribute.") != EXIT_SUCCESS) {
          return EXIT_FAILURE;
        }

        return EXIT_SUCCESS;
      });
}
