// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_DEVICE_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_DEVICE_H_

#include <lib/fit/thread_safety.h>
#include <lib/magma_service/msd.h>

#include <functional>
#include <memory>
#include <mutex>
#include <thread>
#include <unordered_map>
#include <unordered_set>
#include <vector>

#include "magma_system_connection.h"
#include "primary_fidl_server.h"

namespace msd {
class MagmaSystemBuffer;
class MagmaSystemSemaphore;

class MagmaSystemDevice final : public internal::PrimaryFidlServerHolder::ConnectionOwnerDelegate,
                                public MagmaSystemConnection::Owner {
 public:
  // The msd::Driver instance must outlive the MagmaSystemDevice
  static std::unique_ptr<MagmaSystemDevice> Create(msd::Driver* driver,
                                                   std::unique_ptr<msd::Device> msd_device) {
    if (!msd_device) {
      return nullptr;
    }
    return std::make_unique<MagmaSystemDevice>(driver, std::move(msd_device));
  }

  explicit MagmaSystemDevice(msd::Driver* driver, std::unique_ptr<msd::Device> msd_dev)
      : driver_(driver), msd_dev_(std::move(msd_dev)) {
    connection_set_ =
        std::make_unique<std::unordered_set<std::shared_ptr<internal::PrimaryFidlServerHolder>>>();
  }

  ~MagmaSystemDevice();

  // Opens a connection to the device. On success, returns the connection handle
  // to be passed to the client.
  std::unique_ptr<msd::internal::PrimaryFidlServer> Open(
      msd_client_id_t client_id, fidl::ServerEnd<fuchsia_gpu_magma::Primary> primary,
      fidl::ServerEnd<fuchsia_gpu_magma::Notification> notification, MagmaClientType client_type);

  msd::Device* msd_dev() { return msd_dev_.get(); }
  msd::Driver* driver() override { return driver_; }

  // Returns the device id. 0 is invalid.
  uint32_t GetDeviceId() override;

  // Called on driver thread
  void Shutdown();

  // Called on driver thread.  |device_handle| may be used by the connection thread for
  // initialization/configuration but should not be retained.
  void StartConnectionThread(std::unique_ptr<msd::internal::PrimaryFidlServer> fidl_server,
                             fit::function<void(const char*)> set_thread_priority);

  void ConnectionClosed(std::shared_ptr<internal::PrimaryFidlServerHolder> server,
                        bool* need_detach_out) override;

  void DumpStatus(uint32_t dump_type) { msd_dev()->MsdDumpStatus(dump_type); }

  magma::Status Query(uint64_t id, magma_handle_t* buffer_out, uint64_t* value_out);

  magma::Status Query(uint64_t id, uint64_t* value_out) { return Query(id, nullptr, value_out); }

  magma_status_t GetIcdList(std::vector<MsdIcdInfo>* icd_list_out);

  void SetMemoryPressureLevel(MagmaMemoryPressureLevel level);

  void SetPowerState(int64_t power_state, fit::callback<void(magma_status_t)> completer);

  void set_perf_count_access_token_id(uint64_t id) { perf_count_access_token_id_ = id; }
  uint64_t perf_count_access_token_id() const override { return perf_count_access_token_id_; }

 private:
  msd::Driver* driver_;
  std::unique_ptr<msd::Device> msd_dev_;
  uint64_t perf_count_access_token_id_ = 0u;

  std::unique_ptr<std::unordered_set<std::shared_ptr<internal::PrimaryFidlServerHolder>>>
      connection_set_ FIT_GUARDED(connection_list_mutex_);
  std::mutex connection_list_mutex_;
};

}  // namespace msd

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_DEVICE_H_
