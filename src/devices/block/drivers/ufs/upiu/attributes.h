// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_ATTRIBUTES_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_ATTRIBUTES_H_

#include <endian.h>

#include "query_request.h"

namespace ufs {

// UFS Specification Version 3.1, section 14.3 "Attributes".
enum class Attributes : uint8_t {
  bBootLunEn = 0x00,
  bCurrentPowerMode = 0x02,
  bActiveIccLevel = 0x03,
  bOutOfOrderDataEn = 0x04,
  bBackgroundOpStatus = 0x05,
  bPurgeStatus = 0x06,
  bMaxDataInSize = 0x07,
  bMaxDataOutSize = 0x08,
  dDynCapNeeded = 0x09,
  bRefClkFreq = 0x0a,
  bConfigDescrLock = 0x0b,
  bMaxNumOfRTT = 0x0c,
  wExceptionEventControl = 0x0d,
  wExceptionEventStatus = 0x0e,
  dSecondsPassed = 0x0f,
  wContextConf = 0x10,
  bDeviceFFUStatus = 0x14,
  bPSAState = 0x15,
  dPSADataSize = 0x16,
  bRefClkGatingWaitTime = 0x17,
  bDeviceCaseRoughTemperaure = 0x18,
  bDeviceTooHighTempBoundary = 0x19,
  bDeviceTooLowTempBoundary = 0x1a,
  bThrottlingStatus = 0x1b,
  bWBBufferFlushStatus = 0x1c,
  bAvailableWBBufferSize = 0x1d,
  bWBBufferLifeTimeEst = 0x1e,
  dCurrentWBBufferSize = 0x1f,
  bRefreshStatus = 0x2c,
  bRefreshFreq = 0x2d,
  bRefreshUnit = 0x2e,
  bRefreshMethod = 0x2f,
  kAttributeCount = 0x30,
};

inline constexpr Attributes kWriteOnceAttributes[] = {
    Attributes::bConfigDescrLock,  // write-once
    Attributes::bPSAState,         // can advance to LOCKED (write-once)
};

constexpr bool IsWriteOnce(Attributes attr) {
  for (Attributes write_once_attr : kWriteOnceAttributes) {
    if (attr == write_once_attr) {
      return true;
    }
  }
  return false;
}

// UFS Specification Version 3.1, section 14.3 "Attributes".
enum BackgroundOpStatus {
  kNotRequired = 0x00,
  kRequiredNotCritical = 0x01,
  kRequiredPerformanceImpact = 0x02,
  kCritical = 0x03,
};

struct ExceptionEventControl {
  uint16_t value;

  DEF_SUBBIT(value, 0, dyncap_event_en);
  DEF_SUBBIT(value, 1, syspool_event_en);
  DEF_SUBBIT(value, 2, urgent_bkops_en);
  DEF_SUBBIT(value, 3, too_high_temp_en);
  DEF_SUBBIT(value, 4, too_low_temp_en);
  DEF_SUBBIT(value, 5, writebooster_event_en);
  DEF_SUBBIT(value, 6, performance_throttling_en);
};

struct ExceptionEventStatus {
  uint16_t value;

  DEF_SUBBIT(value, 0, dyncap_needed);
  DEF_SUBBIT(value, 1, syspool_exhausted);
  DEF_SUBBIT(value, 2, urgent_bkops);
  DEF_SUBBIT(value, 3, too_high_temp);
  DEF_SUBBIT(value, 4, too_low_temp);
  DEF_SUBBIT(value, 5, writebooster_flush_needed);
  DEF_SUBBIT(value, 6, performance_throttling_support);
};

constexpr uint8_t kExceededWriteBoosterBufferLifeTime = 0x0b;

// UFS Specification Version 3.1, section 6.4 "Reference Clock".
enum AttributeReferenceClock {
  k19_2MHz = 0x0,
  k26MHz = 0x1,
  k38_4MHz = 0x2,
  kObsolete = 0x3,
};

class ReadAttributeUpiu : public QueryReadRequestUpiu {
 public:
  explicit ReadAttributeUpiu(Attributes type, uint8_t index = 0)
      : QueryReadRequestUpiu(QueryOpcode::kReadAttribute, static_cast<uint8_t>(type), index) {}
};

class WriteAttributeUpiu : public QueryWriteRequestUpiu {
 public:
  explicit WriteAttributeUpiu(Attributes type, uint32_t value, uint8_t index = 0)
      : QueryWriteRequestUpiu(QueryOpcode::kWriteAttribute, static_cast<uint8_t>(type), index) {
    if (value) {
      GetData<QueryRequestUpiuData>()->value = htobe32(value);
    }
  }
};

class AttributeResponseUpiu : public QueryResponseUpiu {
 public:
  uint32_t GetAttribute() { return betoh32(GetData<QueryResponseUpiuData>()->value); }
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_ATTRIBUTES_H_
