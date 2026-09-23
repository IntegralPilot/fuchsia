// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "cpp_device.h"

#include <lib/magma/util/macros.h>

#include <bit>
#include <cstdint>
#include <mutex>

#include "cpp_connection.h"
#include "sdk/lib/magma_common/include/lib/magma/magma_common_defs.h"

namespace msd {

// static
void DeviceCallback::Release(DeviceCallback* callback) {
  callback->owner_->ReleaseCallback(callback);
}

class SetPowerStateCallback : public DeviceCallback {
 public:
  using Completer = fit::callback<void(magma_status_t)>;

  explicit SetPowerStateCallback(CppDevice* owner, Completer completer)
      : DeviceCallback(owner), completer_(std::move(completer)) {}

  uintptr_t GetContext() { return std::bit_cast<uintptr_t>(this); }

  static void Callback(uintptr_t context, magma_status_t status) {
    SetPowerStateCallback* callback = std::bit_cast<SetPowerStateCallback*>(context);
    callback->assert_valid();

    callback->completer_(status);

    DeviceCallback::Release(callback);
  }

 private:
  Completer completer_;
};

//////////////////////////////////////////////////////////////////////////////////////////////////

CppDevice::~CppDevice() { msd_device_release(device_); }

magma_status_t CppDevice::MsdQuery(uint64_t id, zx::vmo* result_buffer_out, uint64_t* result_out) {
  return msd_device_query(device_, id,
                          result_buffer_out ? result_buffer_out->reset_and_get_address() : nullptr,
                          result_out);
}

magma_status_t CppDevice::MsdGetIcdList(std::vector<MsdIcdInfo>* icd_info_out) {
  ZX_DEBUG_ASSERT(icd_info_out);

  std::vector<MsdIcdInfo> icd_info = {MsdIcdInfo{
      .component_url = "fuchsia-pkg://fuchsia.com/vulkan_freedreno#meta/vulkan.cm",
      .support_flags = ICD_SUPPORT_FLAG_VULKAN,
  }};

  *icd_info_out = std::move(icd_info);

  return MAGMA_STATUS_OK;
}

std::unique_ptr<Connection> CppDevice::MsdOpen(msd_client_id_t client_id) {
  return CppConnection::Create(device_, client_id);
}

void CppDevice::MsdSetPowerState(int64_t power_state,
                                 fit::callback<void(magma_status_t)> completer) {
  auto callback = std::make_unique<SetPowerStateCallback>(this, std::move(completer));
  {
    std::lock_guard<std::mutex> lock(device_callback_mutex_);

    msd_device_set_power_state(device_, power_state, SetPowerStateCallback::Callback,
                               callback->GetContext());

    device_callback_set_.emplace(callback.get(), std::move(callback));
  }
}

void CppDevice::ReleaseCallback(DeviceCallback* callback) {
  {
    std::lock_guard<std::mutex> lock(device_callback_mutex_);

    auto iter = device_callback_set_.find(callback);

    if (iter != device_callback_set_.end()) {
      device_callback_set_.erase(iter);
      return;
    }
  }

  MAGMA_DASSERT(false);  // callback not found
}

}  // namespace msd
