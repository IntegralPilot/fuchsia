// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_PAVER_MOONFLOWER_H_
#define SRC_FIRMWARE_PAVER_MOONFLOWER_H_

#include <hwreg/bitfields.h>

#include "src/firmware/paver/abr-client.h"
#include "src/firmware/paver/device-partitioner.h"
#include "src/firmware/paver/gpt.h"
#include "src/firmware/paver/partition-client.h"

namespace paver {

using FilterCallback = GptDevicePartitioner::FilterCallback;

struct MoonflowerGptEntryAttributes {
  static constexpr uint8_t kMoonflowerMaxPriority = 3;

  explicit MoonflowerGptEntryAttributes(uint64_t flags) : flags(flags) {}

  uint64_t flags;
  DEF_SUBFIELD(flags, 49, 48, priority);
  DEF_SUBBIT(flags, 50, active);
  DEF_SUBFIELD(flags, 53, 51, retry_count);
  DEF_SUBBIT(flags, 54, boot_success);
  DEF_SUBBIT(flags, 55, unbootable);
};

class MoonflowerPartitioner : public DevicePartitioner {
 public:
  // Known inactive type GUID - currently we only have one, future Moonflower boards may need to
  // expand this to include more.
  static constexpr uuid::Uuid kInactiveTypeGuid =
      uuid::Uuid(GPT_GUID(0x77036CD4, 0x03D5, 0x42BB, 0x8ED1, 0x37E5A88BAA34));

  static zx::result<std::unique_ptr<DevicePartitioner>> Initialize(
      const PaverConfig& config, const BlockDevices& devices,
      fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root);

  zx::result<std::unique_ptr<abr::Client>> CreateAbrClient() const override;

  const paver::BlockDevices& Devices() const override;

  fidl::UnownedClientEnd<fuchsia_io::Directory> SvcRoot() const override;

  bool SupportsPartition(const PartitionSpec& spec) const override;

  zx::result<std::unique_ptr<PartitionClient>> FindPartition(
      const PartitionSpec& spec) const override;

  zx::result<> ResetPartitionTables() const override;

  zx::result<> ValidatePayload(const PartitionSpec& spec,
                               std::span<const uint8_t> data) const override;

  zx::result<> Flush() const override { return zx::ok(); }

  zx::result<> OnStop() const override { return zx::ok(); }

  // Like FindPartition() above, but returns all matching entries.
  zx::result<std::vector<std::unique_ptr<BlockPartitionClient>>> FindAllPartitions(
      FilterCallback filter) const;

  // Like FindPartition() above, but returns a BlockPartitionClient instead, which has additional
  // GPT-related methods.
  zx::result<std::unique_ptr<BlockPartitionClient>> FindGptPartition(
      const PartitionSpec& spec) const;

 private:
  MoonflowerPartitioner(const PaverConfig& config, std::unique_ptr<GptDevicePartitioner> gpt)
      : config_(config), gpt_(std::move(gpt)) {}

  zx::result<std::vector<std::string>> PartitionNamesForSpec(const PartitionSpec& spec) const;

  const PaverConfig config_;
  std::unique_ptr<GptDevicePartitioner> gpt_;
};

class MoonflowerPartitionerFactory : public DevicePartitionerFactory {
 public:
  zx::result<std::unique_ptr<DevicePartitioner>> New(
      const BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config, std::shared_ptr<Context> context) final;
};

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_MOONFLOWER_H_
