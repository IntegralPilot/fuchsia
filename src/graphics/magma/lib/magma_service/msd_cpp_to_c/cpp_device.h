// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_DEVICE_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_DEVICE_H_

#include <lib/fit/thread_safety.h>
#include <lib/magma/util/macros.h>
#include <lib/magma_service/msd.h>
#include <lib/magma_service/msd_c.h>
#include <lib/zx/vmo.h>

#include <memory>
#include <mutex>
#include <unordered_map>

namespace msd {

class CppDevice;

class DeviceCallback {
 public:
  // DeviceCallbacks are contained by the owner CppDevice.
  explicit DeviceCallback(CppDevice* owner) : owner_(owner) {}

  virtual ~DeviceCallback() = default;

  void assert_valid() const { MAGMA_DASSERT(magic_ == kMagic); }

  static void Release(DeviceCallback* callback);

 private:
  CppDevice* owner_;
  static constexpr uint32_t kMagic = 0xabcd1234;
  uint32_t magic_ = kMagic;
};

class CppDevice : public Device {
 public:
  explicit CppDevice(struct MsdDevice* device) : device_(device) {}

  ~CppDevice();

  // Device implementation
  magma_status_t MsdQuery(uint64_t id, zx::vmo* result_buffer_out, uint64_t* result_out) override;
  magma_status_t MsdGetIcdList(std::vector<MsdIcdInfo>* icd_info_out) override;
  void MsdSetPowerState(int64_t power_state,
                        fit::callback<void(magma_status_t)> completer) override;
  std::unique_ptr<Connection> MsdOpen(msd_client_id_t client_id) override;

  // Other implementation
  void ReleaseCallback(DeviceCallback* callback);

 private:
  struct MsdDevice* device_;
  std::mutex device_callback_mutex_;
  std::unordered_map<DeviceCallback*, std::unique_ptr<DeviceCallback>> device_callback_set_
      FIT_GUARDED(device_callback_mutex_);
};
}  // namespace msd

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_DEVICE_H_
