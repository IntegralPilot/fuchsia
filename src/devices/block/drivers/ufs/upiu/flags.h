// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_FLAGS_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_FLAGS_H_

#include "query_request.h"

namespace ufs {

// UFS Specification Version 3.1, section 14.2 "Flags".
enum class Flags : uint8_t {
  fReserved = 0x0,
  fDeviceInit = 0x1,
  fPermanentWPEn,
  fPowerOnWPEn,
  fBackgroundOpsEn,
  fDeviceLifeSpanModeEn,
  fPurgeEnable,
  fRefreshEnable,
  fPhyResourceRemoval = 0x8,
  fBusyRTC,
  fPermanentlyDisableFwUpdate = 0xb,
  fWriteBoosterEn = 0xe,
  fWBBufferFlushEn = 0xf,
  fWBBufferFlushDuringHibernate = 0x10,
  kFlagCount = 0x11,
};

inline constexpr Flags kWriteOnceFlags[] = {
    Flags::fPermanentWPEn,               // write-once
    Flags::fPermanentlyDisableFwUpdate,  // write-once
    Flags::fPhyResourceRemoval,          // physically removes capacity
};

constexpr bool IsWriteOnce(Flags flag) {
  for (Flags write_once_flag : kWriteOnceFlags) {
    if (flag == write_once_flag) {
      return true;
    }
  }
  return false;
}

class ReadFlagUpiu : public QueryReadRequestUpiu {
 public:
  explicit ReadFlagUpiu(Flags type)
      : QueryReadRequestUpiu(QueryOpcode::kReadFlag, static_cast<uint8_t>(type)) {}
};

class SetFlagUpiu : public QueryWriteRequestUpiu {
 public:
  explicit SetFlagUpiu(Flags type)
      : QueryWriteRequestUpiu(QueryOpcode::kSetFlag, static_cast<uint8_t>(type)) {}
};

class ClearFlagUpiu : public QueryWriteRequestUpiu {
 public:
  explicit ClearFlagUpiu(Flags type)
      : QueryWriteRequestUpiu(QueryOpcode::kClearFlag, static_cast<uint8_t>(type)) {}
};

class ToggleFlagUpiu : public QueryWriteRequestUpiu {
 public:
  explicit ToggleFlagUpiu(Flags type)
      : QueryWriteRequestUpiu(QueryOpcode::kToggleFlag, static_cast<uint8_t>(type)) {}
};

class FlagResponseUpiu : public QueryResponseUpiu {
 public:
  uint8_t GetFlag() { return GetData<QueryResponseUpiuData>()->flag_value; }
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_FLAGS_H_
