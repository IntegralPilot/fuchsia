// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_FIRMWARE_PAVER_VIM3_H_
#define SRC_FIRMWARE_PAVER_VIM3_H_

#include "src/firmware/paver/abr-client.h"
#include "src/firmware/paver/device-partitioner.h"
#include "src/firmware/paver/gpt.h"
#include "src/firmware/paver/partition-client.h"

namespace paver {

class Vim3Partitioner : public DevicePartitioner {
 public:
  static zx::result<std::unique_ptr<DevicePartitioner>> Initialize(
      const BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
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
  Vim3Partitioner(std::unique_ptr<GptDevicePartitioner> gpt, BlockDevices non_gpt_devices)
      : gpt_(std::move(gpt)), non_gpt_devices_(std::move(non_gpt_devices)) {}

  zx::result<std::unique_ptr<PartitionClient>> GetEmmcBootPartitionClient() const;

  std::unique_ptr<GptDevicePartitioner> gpt_;
  // Some partitions aren't stored in the GPT, so we retain a connector for them.
  BlockDevices non_gpt_devices_;
};

class Vim3PartitionerFactory : public DevicePartitionerFactory {
 public:
  zx::result<std::unique_ptr<DevicePartitioner>> New(
      const BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config, std::shared_ptr<Context> context) final;
};

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_VIM3_H_
