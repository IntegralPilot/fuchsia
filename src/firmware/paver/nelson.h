// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_FIRMWARE_PAVER_NELSON_H_
#define SRC_FIRMWARE_PAVER_NELSON_H_

#include "src/firmware/paver/abr-client.h"
#include "src/firmware/paver/block-devices.h"
#include "src/firmware/paver/device-partitioner.h"
#include "src/firmware/paver/gpt.h"
#include "src/firmware/paver/partition-client.h"

namespace paver {

constexpr size_t kNelsonBL2Size = 64 * 1024;

class NelsonPartitioner : public DevicePartitioner {
 public:
  static zx::result<std::unique_ptr<DevicePartitioner>> Initialize(
      const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config);

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

 private:
  NelsonPartitioner(std::unique_ptr<GptDevicePartitioner> gpt, BlockDevices non_gpt_devices)
      : gpt_(std::move(gpt)), non_gpt_devices_(std::move(non_gpt_devices)) {}

  zx::result<std::unique_ptr<PartitionClient>> GetEmmcBootPartitionClient() const;

  zx::result<std::unique_ptr<PartitionClient>> GetBootloaderPartitionClient(
      const PartitionSpec& spec) const;

  std::unique_ptr<GptDevicePartitioner> gpt_;
  BlockDevices non_gpt_devices_;
};

class NelsonPartitionerFactory : public DevicePartitionerFactory {
 public:
  zx::result<std::unique_ptr<DevicePartitioner>> New(
      const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config, std::shared_ptr<Context> context) final;
};

class NelsonBootloaderPartitionClient final : public PartitionClient {
 public:
  explicit NelsonBootloaderPartitionClient(
      std::unique_ptr<PartitionClient> emmc_boot_client,
      std::unique_ptr<FixedOffsetBlockPartitionClient> tpl_client)
      : emmc_boot_client_(std::move(emmc_boot_client)), tpl_client_(std::move(tpl_client)) {}

  zx::result<size_t> GetBlockSize() final;
  zx::result<size_t> GetPartitionSize() final;
  zx::result<> Read(const zx::vmo& vmo, size_t size) final;
  zx::result<> Write(const zx::vmo& vmo, size_t vmo_size) final;
  zx::result<> Trim() final;
  zx::result<> Flush() final;

  // No copy, no move.
  NelsonBootloaderPartitionClient(const NelsonBootloaderPartitionClient&) = delete;
  NelsonBootloaderPartitionClient& operator=(const NelsonBootloaderPartitionClient&) = delete;
  NelsonBootloaderPartitionClient(NelsonBootloaderPartitionClient&&) = delete;
  NelsonBootloaderPartitionClient& operator=(NelsonBootloaderPartitionClient&&) = delete;

 private:
  bool CheckIfTplSame(const zx::vmo& vmo, size_t size);

  std::unique_ptr<PartitionClient> emmc_boot_client_;
  std::unique_ptr<FixedOffsetBlockPartitionClient> tpl_client_;
};
}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_NELSON_H_
