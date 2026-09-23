// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_NVME_FAKE_FAKE_ADMIN_COMMANDS_H_
#define SRC_DEVICES_BLOCK_DRIVERS_NVME_FAKE_FAKE_ADMIN_COMMANDS_H_

#include "src/devices/block/drivers/nvme/commands.h"
#include "src/devices/block/drivers/nvme/fake/fake-controller.h"
#include "src/devices/block/drivers/nvme/nvme.h"
#include "src/devices/block/drivers/nvme/queue-pair.h"

namespace fake_nvme {

// Default implementations of admin commands, that are sufficient to get a basic test passing.
class FakeAdminCommands {
 public:
  constexpr static const char* kSerialNumber = "12345678";
  constexpr static const char* kModelNumber = "PL4T-1234";
  constexpr static const char* kFirmwareRev = "7.4.2.1";

  explicit FakeAdminCommands(FakeController& controller);

 private:
  void Identify(nvme::Submission& submission, const nvme::TransactionData& data,
                nvme::Completion& completion);

  void ConfigureIoSubmissionQueue(nvme::Submission& submission, const nvme::TransactionData& data,
                                  nvme::Completion& completion);
  void ConfigureIoCompletionQueue(nvme::Submission& submission, const nvme::TransactionData& data,
                                  nvme::Completion& completion);
  void DeleteIoQueue(nvme::Submission& submission, const nvme::TransactionData& data,
                     nvme::Completion& completion);
  void SetFeatures(nvme::Submission& submission, const nvme::TransactionData& data,
                   nvme::Completion& completion);
  void GetFeatures(nvme::Submission& submission, const nvme::TransactionData& data,
                   nvme::Completion& completion);

  FakeController& controller_;
};

}  // namespace fake_nvme

#endif  // SRC_DEVICES_BLOCK_DRIVERS_NVME_FAKE_FAKE_ADMIN_COMMANDS_H_
