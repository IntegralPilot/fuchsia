// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_IWLWIFI_LOG_CPP_INCLUDE_DFV2_WLAN_DRIVERS_LOG_INSTANCE_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_IWLWIFI_LOG_CPP_INCLUDE_DFV2_WLAN_DRIVERS_LOG_INSTANCE_H_

#include <lib/driver/logging/cpp/logger.h>
#include <stdint.h>

namespace wlan::drivers::log {

class Instance {
 public:
  // Driver should call this once at startup before any logging calls are made.
  // Not thread safe
  static void Init(uint32_t filter, fdf::Logger* logger);
  static bool IsFilterOn(uint32_t filter);
  static fdf::Logger* GetLogger();
  static void Reset();

 private:
  static Instance& get();

  uint32_t filter_{};
  fdf::Logger* logger_ = nullptr;
};

}  // namespace wlan::drivers::log

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_IWLWIFI_LOG_CPP_INCLUDE_DFV2_WLAN_DRIVERS_LOG_INSTANCE_H_
