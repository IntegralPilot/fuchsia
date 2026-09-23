// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_CONNECTION_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_CONNECTION_H_

#include <lib/magma_service/msd.h>
#include <lib/magma_service/msd_c.h>

namespace msd {

class CppConnection : public msd::Connection {
 public:
  static std::unique_ptr<CppConnection> Create(struct MsdDevice* device, uint64_t client_id);
  explicit CppConnection() = default;

  ~CppConnection() override;

  magma_status_t MsdMapBuffer(msd::Buffer& buffer, uint64_t gpu_va, uint64_t offset,
                              uint64_t length, uint64_t flags) override;

  void MsdReleaseBuffer(msd::Buffer& buffer, bool shutting_down) override;

  std::unique_ptr<msd::Context> MsdCreateContext() override;
  std::unique_ptr<msd::Context> MsdCreateContext2(uint64_t priority) override;
  void MsdSetNotificationCallback(NotificationHandler* handler) override {
    notification_handler_ = handler;
  }

  void ContextKilled();

 private:
  msd::NotificationHandler* notification_handler_ = nullptr;
  struct MsdConnection* connection_ = nullptr;
};
}  // namespace msd

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_CONNECTION_H_
