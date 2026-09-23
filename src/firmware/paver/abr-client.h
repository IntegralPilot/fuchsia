// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_PAVER_ABR_CLIENT_H_
#define SRC_FIRMWARE_PAVER_ABR_CLIENT_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.paver/cpp/wire.h>
#include <lib/abr/abr.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>

#include <memory>
#include <vector>

#include <fbl/unique_fd.h>

#include "src/firmware/paver/block-devices.h"
#include "src/firmware/paver/config.h"
#include "src/firmware/paver/partition-client.h"
#include "src/firmware/paver/paver-context.h"
#include "src/lib/uuid/uuid.h"
#include "zircon/errors.h"

namespace abr {

// For testing only.
zx::result<fuchsia_paver::wire::Configuration> PartitionUuidToConfiguration(
    const paver::BlockDevices& devices, uuid::Uuid uuid);
// For testing only.
zx::result<fuchsia_paver::wire::Configuration> CurrentSlotToConfiguration(std::string_view slot);

zx::result<fuchsia_paver::wire::Configuration> QueryBootConfig(const paver::BlockDevices& devices,
                                                               const paver::PaverConfig& config);

/// Ensures the system supports verified boot.
/// This should be called prior to creating a DevicePartitioner and its corresponding abr::Client
/// (via DevicePartitioner::CreateAbrClient), as otherwise unexpected errors may be returned.
zx::result<bool> SupportsVerifiedBoot(const paver::BlockDevices& devices,
                                      const paver::PaverConfig& config);

// Interface for interacting with ABR data.
class Client {
 public:
  virtual ~Client() = default;

  AbrSlotIndex GetBootSlot(bool update_metadata, bool* is_slot_marked_successful) const {
    return AbrGetBootSlot(&abr_ops_, update_metadata, is_slot_marked_successful);
  }

  zx::result<AbrSlotIndex> GetSlotLastMarkedActive() const {
    AbrSlotIndex slot;
    auto status = AbrResultToZxStatus(AbrGetSlotLastMarkedActive(&abr_ops_, &slot));
    if (status.is_error()) {
      return status.take_error();
    }
    return zx::ok(slot);
  }

  zx::result<> MarkSlotActive(AbrSlotIndex index) {
    return AbrResultToZxStatus(AbrMarkSlotActive(&abr_ops_, index));
  }

  zx::result<> MarkSlotUnbootable(AbrSlotIndex index) {
    return AbrResultToZxStatus(
        AbrMarkSlotUnbootable(&abr_ops_, index, kAbrUnbootableReasonOsRequested));
  }

  // Only set |from_unbootable_ok| if this is the final boot into the current slot.
  zx::result<> MarkSlotSuccessful(AbrSlotIndex index, bool from_unbootable_ok = false) {
    return AbrResultToZxStatus(AbrMarkSlotSuccessful(&abr_ops_, index, from_unbootable_ok));
  }

  zx::result<AbrSlotInfo> GetSlotInfo(AbrSlotIndex index) const {
    AbrSlotInfo info;
    auto status = AbrResultToZxStatus(AbrGetSlotInfo(&abr_ops_, index, &info));
    if (status.is_error()) {
      return status.take_error();
    }
    return zx::ok(info);
  }

  zx::result<AbrDataOneShotFlags> GetAndClearOneShotFlags() {
    AbrDataOneShotFlags flags;
    auto status = AbrResultToZxStatus(AbrGetAndClearOneShotFlags(&abr_ops_, &flags));
    if (status.is_error()) {
      return status.take_error();
    }
    return zx::ok(flags);
  }

  zx::result<> SetOneShotRecovery() {
    return AbrResultToZxStatus(AbrSetOneShotRecovery(&abr_ops_, true));
  }

  zx::result<> SetOneShotBootloader() {
    return AbrResultToZxStatus(AbrSetOneShotBootloader(&abr_ops_, true));
  }

  static zx::result<> AbrResultToZxStatus(AbrResult status);

  virtual zx::result<> Flush() = 0;

  void InitializeAbrOps();

  explicit Client(bool custom = false) {
    if (custom) {
      abr_ops_ = {this, nullptr, nullptr, Client::ReadAbrMetadataCustom,
                  Client::WriteAbrMetadataCustom};
    } else {
      abr_ops_ = {this, Client::ReadAbrMetaData, Client::WriteAbrMetaData, nullptr, nullptr};
    }
  }

  // No copy, move, assign.
  // This is to ensure that |abr_ops_| is always valid. |abr_ops_.context| shall always be
  // a |this| pointer to the Client instance that hosts |abr_ops_|. This may be
  // violated if we allow Client to be copied/moved/assign.
  Client(const Client&) = delete;
  Client& operator=(const Client&) = delete;

  Client(Client&&) = delete;
  Client& operator=(Client&&) = delete;

 private:
  friend class MoonflowerAbrClientTest;

  AbrOps abr_ops_;

  // ReadAbrMetaData and WriteAbrMetaData will be assigned to fields in AbrOps
  static bool ReadAbrMetaData(void* context, size_t size, uint8_t* buffer);

  static bool WriteAbrMetaData(void* context, const uint8_t* buffer, size_t size);
  static bool ReadAbrMetadataCustom(void* context, AbrSlotData* a, AbrSlotData* b,
                                    uint8_t* one_shot_recovery);
  static bool WriteAbrMetadataCustom(void* context, const AbrSlotData* a, const AbrSlotData* b,
                                     uint8_t one_shot_recovery);

  virtual zx::result<> Read(uint8_t* buffer, size_t size) = 0;

  virtual zx::result<> Write(const uint8_t* buffer, size_t size) = 0;

  virtual zx::result<> ReadCustom(AbrSlotData* a, AbrSlotData* b, uint8_t* one_shot_recovery) = 0;

  virtual zx::result<> WriteCustom(const AbrSlotData* a, const AbrSlotData* b,
                                   uint8_t one_shot_recovery) = 0;
};

// Implementation of abr::Client which works with a contiguous partition storing AbrData.
class AbrPartitionClient : public Client {
 public:
  // |partition| should contain AbrData with no offset.
  static zx::result<std::unique_ptr<abr::Client>> Create(
      std::unique_ptr<paver::PartitionClient> partition);

 private:
  AbrPartitionClient(std::unique_ptr<paver::PartitionClient> partition, zx::vmo vmo,
                     size_t block_size)
      : partition_(std::move(partition)), vmo_(std::move(vmo)), block_size_(block_size) {}

  zx::result<> Read(uint8_t* buffer, size_t size) override;

  zx::result<> Write(const uint8_t* buffer, size_t size) override;

  zx::result<> ReadCustom(AbrSlotData* a, AbrSlotData* b, uint8_t* one_shot_recovery) override {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  zx::result<> WriteCustom(const AbrSlotData* a, const AbrSlotData* b,
                           uint8_t one_shot_recovery) override {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  zx::result<> Flush() override { return partition_->Flush(); }

  std::unique_ptr<paver::PartitionClient> partition_;
  zx::vmo vmo_;
  size_t block_size_;
};

}  // namespace abr

#endif  // SRC_FIRMWARE_PAVER_ABR_CLIENT_H_
