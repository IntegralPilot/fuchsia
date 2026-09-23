// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_DESCRIPTORS_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_DESCRIPTORS_H_

#include <endian.h>

#include "query_request.h"
#include "safemath/safe_conversions.h"

namespace ufs {

// UFS Specification Version 3.1, section 14.1 "UFS Descriptors".
// All descriptors use big-endian byte ordering.
enum class DescriptorType : uint8_t {
  kDevice = 0x00,
  kConfiguration = 0x01,
  kUnit = 0x02,
  kInterconnect = 0x04,
  kString = 0x05,
  kGeometry = 0x07,
  kPower = 0x08,
  kDeviceHealth = 0x09,
  kDescriptorCount = 0x0a,
};

// UFS Specification Version 3.1, section 14.1.4.2 "Device Descriptor".
// DeviceDescriptor use big-endian byte ordering.
struct DeviceDescriptor {
  uint8_t bLength;
  uint8_t bDescriptorIDN;
  uint8_t bDevice;
  uint8_t bDeviceClass;
  uint8_t bDeviceSubClass;
  uint8_t bProtocol;
  uint8_t bNumberLU;
  uint8_t bNumberWLU;
  uint8_t bBootEnable;
  uint8_t bDescrAccessEn;
  uint8_t bInitPowerMode;
  uint8_t bHighPriorityLUN;
  uint8_t bSecureRemovalType;
  uint8_t bSecurityLU;
  uint8_t bBackgroundOpsTermLat;
  uint8_t bInitActiveICCLevel;
  // 0x10
  uint16_t wSpecVersion;
  uint16_t wManufactureDate;
  uint8_t iManufacturerName;
  uint8_t iProductName;
  uint8_t iSerialNumber;
  uint8_t iOemID;
  uint16_t wManufacturerID;
  uint8_t bUD0BaseOffset;
  uint8_t bUDConfigPLength;
  uint8_t bDeviceRTTCap;
  uint16_t wPeriodicRTCUpdate;
  uint8_t bUfsFeaturesSupport;
  // 0x20
  uint8_t bFFUTimeout;
  uint8_t bQueueDepth;
  uint16_t wDeviceVersion;
  uint8_t bbNumSecureWPArea;
  uint32_t dPSAMaxDataSize;
  uint8_t bPSAStateTimeout;
  uint8_t iProductRevisionLevel;
  uint8_t Reserved[5];  // 0x2a
  // 0x30
  uint8_t ReservedUME[16];
  // UFS 3.1 Start: 0x40
  uint8_t ReservedHpb[3];
  uint8_t Reserved2[12];
  uint32_t dExtendedUfsFeaturesSupport;
  uint8_t bWriteBoosterBufferPreserveUserSpaceEn;
  uint8_t bWriteBoosterBufferType;
  uint32_t dNumSharedWriteBoosterBufferAllocUnits;
} __PACKED;
static_assert(sizeof(DeviceDescriptor) == 89, "DeviceDescriptor struct must be 89 bytes");

// Accessing the bits inside DeviceDescriptor::dExtendedUfsFeaturesSupport causes an unaligned
// access error, so we need to copy it to a separate struct and access it.
struct ExtendedUfsFeaturesSupport {
  uint32_t value;

  DEF_SUBBIT(value, 0, field_firmware_update_support);
  DEF_SUBBIT(value, 1, production_state_awareness_support);
  DEF_SUBBIT(value, 2, device_life_span_support);
  DEF_SUBBIT(value, 3, refresh_operation_support);
  DEF_SUBBIT(value, 4, too_high_temperature_support);
  DEF_SUBBIT(value, 5, too_low_temperature_support);
  DEF_SUBBIT(value, 6, extended_temperature_support);
  DEF_SUBBIT(value, 7, reserved_for_hpb_extension_specification);
  DEF_SUBBIT(value, 8, writebooster_support);
  DEF_SUBBIT(value, 9, performance_throttling_support);
  DEF_SUBBIT(value, 10, advanced_rpmb_support);
  DEF_SUBBIT(value, 14, barrier_support);
  DEF_SUBBIT(value, 15, clear_error_history_functionality_support);
  DEF_SUBBIT(value, 16, ext_iid_support);
  DEF_SUBBIT(value, 17, reserved_for_fbo_extension_specification);
};

// UFS Specification Version 3.1, section 14.1.4.3 "Configuration Descriptor".
// ConfigurationDescriptor use big-endian byte ordering.
struct UnitDescriptorConfigurableParameters {
  uint8_t bLUEnable;
  uint8_t bBootLunID;
  uint8_t bLUWriteProtect;
  uint8_t bMemoryType;
  uint32_t dNumAllocUnits;
  uint8_t bDataReliability;
  uint8_t bLogicalBlockSize;
  uint8_t bProvisioningType;
  uint16_t wContextCapabilities;
  union {
    struct {
      uint8_t Reserved[3];
      uint8_t ReservedHpb[6];
    } __PACKED;
    uint16_t wZoneBufferAllocUnits;
  };
  uint32_t dLUNumWriteBoosterBufferAllocUnits;
} __PACKED;
static_assert(sizeof(UnitDescriptorConfigurableParameters) == 27,
              "UnitDescriptorConfigurableParameters struct must be 27 bytes");

constexpr uint32_t kConfigurationDesceiptorLuNum = 8;

struct ConfigurationDescriptor {
  uint8_t bLength;
  uint8_t bDescriptorIDN;
  uint8_t bConfDescContinue;
  uint8_t bBootEnable;
  uint8_t bDescrAccessEn;
  uint8_t bInitPowerMode;
  uint8_t bHighPriorityLUN;
  uint8_t bSecureRemovalType;
  uint8_t bInitActiveICCLevel;
  uint16_t wPeriodicRTCUpdate;
  uint8_t Reserved;
  uint8_t bRPMBRegionEnable;
  uint8_t bRPMBRegion1Size;
  uint8_t bRPMBRegion2Size;
  uint8_t bRPMBRegion3Size;
  uint8_t bWriteBoosterBufferPreserveUserSpaceEn;
  uint8_t bWriteBoosterBufferType;
  uint32_t dNumSharedWriteBoosterBufferAllocUnits;
  // 0x16
  UnitDescriptorConfigurableParameters UnitConfigParams[kConfigurationDesceiptorLuNum];
} __PACKED;
static_assert(sizeof(ConfigurationDescriptor) == (22 + 27 * 8),
              "ConfigurationDescriptor struct must be 238 bytes");

// UFS Specification Version 3.1, section 14.1.4.4 "Geometry Descriptor".
// GeometryDescriptor use big-endian byte ordering.
struct GeometryDescriptor {
  uint8_t bLength;
  uint8_t bDescriptorIDN;
  uint8_t bMediaTechnology;
  uint8_t Reserved;
  uint64_t qTotalRawDeviceCapacity;
  uint8_t bMaxNumberLU;
  uint32_t dSegmentSize;
  // 0x11
  uint8_t bAllocationUnitSize;
  uint8_t bMinAddrBlockSize;
  uint8_t bOptimalReadBlockSize;
  uint8_t bOptimalWriteBlockSize;
  uint8_t bMaxInBufferSize;
  uint8_t bMaxOutBufferSize;
  uint8_t bRPMB_ReadWriteSize;
  uint8_t bDynamicCapacityResourcePolicy;
  uint8_t bDataOrdering;
  uint8_t bMaxContexIDNumber;
  uint8_t bSysDataTagUnitSize;
  uint8_t bSysDataTagResSize;
  uint8_t bSupportedSecRTypes;
  uint16_t wSupportedMemoryTypes;
  // 0x20
  uint32_t dSystemCodeMaxNAllocU;
  uint16_t wSystemCodeCapAdjFac;
  uint32_t dNonPersistMaxNAllocU;
  uint16_t wNonPersistCapAdjFac;
  uint32_t dEnhanced1MaxNAllocU;
  // 0x30
  uint16_t wEnhanced1CapAdjFac;
  uint32_t dEnhanced2MaxNAllocU;
  uint16_t wEnhanced2CapAdjFac;
  uint32_t dEnhanced3MaxNAllocU;
  uint16_t wEnhanced3CapAdjFac;
  uint32_t dEnhanced4MaxNAllocU;
  // 0x42
  uint16_t wEnhanced4CapAdjFac;
  uint32_t dOptimalLogicalBlockSize;
  // UFS 3.1 Start: 0x48
  uint8_t ReservedHpb[5];
  uint8_t Reserved2[2];
  uint32_t dWriteBoosterBufferMaxNAllocUnits;
  uint8_t bDeviceMaxWriteBoosterLUs;
  uint8_t bWriteBoosterBufferCapAdjFac;
  uint8_t bSupportedWriteBoosterBufferUserSpaceReductionTypes;
  uint8_t bSupportedWriteBoosterBufferTypes;
} __PACKED;
static_assert(sizeof(GeometryDescriptor) == 87, "GeometryDescriptor struct must be 87 bytes");

// UFS Specification Version 3.1, section 14.1.4.5 "Unit Descriptor".
// UnitDescriptor use big-endian byte ordering.
struct UnitDescriptor {
  uint8_t bLength;
  uint8_t bDescriptorIDN;
  uint8_t bUnitIndex;
  uint8_t bLUEnable;
  uint8_t bBootLunID;
  uint8_t bLUWriteProtect;
  uint8_t bLUQueueDepth;
  uint8_t bPSASensitive;
  uint8_t bMemoryType;
  uint8_t bDataReliability;
  uint8_t bLogicalBlockSize;
  uint64_t qLogicalBlockCount;
  // 0x13
  uint32_t dEraseBlockSize;
  uint8_t bProvisioningType;
  uint64_t qPhyMemResourceCount;
  // 0x20
  uint16_t wContextCapabilities;
  uint8_t bLargeUnitGranularity_M1;
  // UFS 3.1 Start: 0x23
  uint8_t ReservedHpb[6];
  uint32_t dLUNumWriteBoosterBufferAllocUnits;
} __PACKED;
static_assert(sizeof(UnitDescriptor) == 45, "UnitDescriptor struct must be 45 bytes");

enum LUWriteProtect {
  kNoWriteProtect = 0x00,
  kPowerOnWriteProtect = 0x01,
  kPermanentWriteProtect = 0x02,
};

// UFS Specification Version 3.1, section 14.1.4.6 "RPMB Unit Descriptor".
// RpmbUnitDescriptor use big-endian byte ordering.
struct RpmbUnitDescriptor {
  uint8_t bLength;
  uint8_t bDescriptorIDN;
  uint8_t bUnitIndex;
  uint8_t bLUEnable;
  uint8_t bBootLunID;
  uint8_t bLUWriteProtect;
  uint8_t bLUQueueDepth;
  uint8_t bPSASensitive;
  uint8_t bMemoryType;
  uint8_t Reserved;
  uint8_t bLogicalBlockSize;
  uint64_t qLogicalBlockCount;
  // 0x13
  uint32_t dEraseBlockSize;
  uint8_t bProvisioningType;
  uint64_t qPhyMemResourceCount;
  // 0x20
  uint8_t Reserved1[3];
} __PACKED;
static_assert(sizeof(RpmbUnitDescriptor) == 35, "RpmbUnitDescriptor struct must be 35 bytes");

// UFS Specification Version 3.1, section 14.1.4.7 "Power Parameters Descriptor".
// PowerParametersDescriptor use big-endian byte ordering.
struct PowerParametersDescriptor {
  uint8_t bLength;
  uint8_t bDescriptorIDN;
  uint16_t wActiveICCLevelsVCC[16];
  uint16_t wActiveICCLevelsVCCQ[16];
  uint16_t wActiveICCLevelsVCCQ2[16];
} __PACKED;
static_assert(sizeof(PowerParametersDescriptor) == 98,
              "PowerParametersDescriptor struct must be 98 bytes");

// UFS Specification Version 3.1, section 14.1.4.8 "Interconnect Descriptor".
// InterconnectDescriptor use big-endian byte ordering.
struct InterconnectDescriptor {
  uint8_t bLength;
  uint8_t bDescriptorIDN;
  uint16_t bcdUniproVersion;
  uint16_t bcdMphyVersion;
} __PACKED;
static_assert(sizeof(InterconnectDescriptor) == 6, "InterconnectDescriptor struct must be 6 bytes");

// UFS Specification Version 3.1, section 14.1.4.9 ~ 13 "String Descriptor".
// StringDescriptor use big-endian byte ordering.
struct StringDescriptor {
  uint8_t bLength;
  uint8_t bDescriptorIDN;
  uint16_t UC[126];
} __PACKED;
static_assert(sizeof(StringDescriptor) == 254, "StringDescriptor struct must be 254 bytes");

// UFS Specification Version 3.1, section 14.1.4.14 "Device Health Descriptor".
// DeviceHealthDescriptor use big-endian byte ordering.
struct DeviceHealthDescriptor {
  uint8_t bLength;
  uint8_t bDescriptorIDN;
  uint8_t bPreEOLInfo;
  uint8_t bDeviceLifeTimeEstA;
  uint8_t bDeviceLifeTimeEstB;
  uint8_t VendorPropInfo[32];
  uint32_t dRefreshTotalCount;
  uint32_t dRefreshProgress;
} __PACKED;
static_assert(sizeof(DeviceHealthDescriptor) == 45,
              "DeviceHealthDescriptor struct must be 45 bytes");

inline size_t GetDescriptorSize(DescriptorType type) {
  switch (type) {
    case DescriptorType::kDevice:
      return sizeof(DeviceDescriptor);
    case DescriptorType::kConfiguration:
      return sizeof(ConfigurationDescriptor);
    case DescriptorType::kUnit:
      return sizeof(UnitDescriptor);
    case DescriptorType::kInterconnect:
      return sizeof(InterconnectDescriptor);
    case DescriptorType::kString:
      return sizeof(StringDescriptor);
    case DescriptorType::kGeometry:
      return sizeof(GeometryDescriptor);
    case DescriptorType::kPower:
      return sizeof(PowerParametersDescriptor);
    case DescriptorType::kDeviceHealth:
      return sizeof(DeviceHealthDescriptor);
    default:
      return 0;
  }
}

class ReadDescriptorUpiu : public QueryReadRequestUpiu {
 public:
  explicit ReadDescriptorUpiu(DescriptorType type, uint8_t index = 0, uint16_t length = 0)
      : QueryReadRequestUpiu(QueryOpcode::kReadDescriptor, static_cast<uint8_t>(type), index) {
    GetData<QueryRequestUpiuData>()->length = htobe16(length ? length : GetDescriptorSize(type));
  }
};

class WriteDescriptorUpiu : public QueryWriteRequestUpiu {
 public:
  explicit WriteDescriptorUpiu(DescriptorType type, void* descriptor_data, uint8_t index = 0)
      : QueryWriteRequestUpiu(QueryOpcode::kWriteDescriptor, static_cast<uint8_t>(type), index) {
    uint16_t length = safemath::checked_cast<uint16_t>(GetDescriptorSize(type));
    GetData<QueryRequestUpiuData>()->length = htobe16(length);

    // Fill data segment
    ZX_ASSERT_MSG(length <= sizeof(GetData<QueryRequestUpiuData>()->command_data),
                  "Tried to copy %u bytes to a buffer of size %zu.", length,
                  sizeof(GetData<QueryRequestUpiuData>()->command_data));
    CustomMemCpy(GetData<QueryRequestUpiuData>()->command_data.data(), descriptor_data, length);
    GetData<QueryRequestUpiuData>()->header.data_segment_length = htobe16(length);
  }
};

class DescriptorResponseUpiu : public QueryResponseUpiu {
 public:
  template <typename T>
  T& GetDescriptor() {
    return reinterpret_cast<T&>(GetData<QueryRequestUpiuData>()->command_data[0]);
  }
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_DESCRIPTORS_H_
