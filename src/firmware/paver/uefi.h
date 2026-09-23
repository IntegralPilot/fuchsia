// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_FIRMWARE_PAVER_UEFI_H_
#define SRC_FIRMWARE_PAVER_UEFI_H_

#include <utility>

#include "src/firmware/paver/device-partitioner.h"
#include "src/firmware/paver/gpt.h"

namespace paver {

// DevicePartitioner implementation for EFI based devices.
class EfiDevicePartitioner : public DevicePartitioner {
 public:
  static zx::result<std::unique_ptr<DevicePartitioner>> Initialize(
      const BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config, std::shared_ptr<Context> context);

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

  zx::result<> OnStop() const override;

 private:
  EfiDevicePartitioner(Arch arch, std::unique_ptr<GptDevicePartitioner> gpt,
                       std::shared_ptr<Context> context)
      : gpt_(std::move(gpt)), arch_(arch), context_(std::move(context)) {}

  zx::result<> CallAbr(std::function<zx::result<>(abr::Client&)> call_abr) const;

  std::unique_ptr<GptDevicePartitioner> gpt_;
  Arch arch_;
  std::shared_ptr<Context> context_;
};

class UefiPartitionerFactory : public DevicePartitionerFactory {
 public:
  zx::result<std::unique_ptr<DevicePartitioner>> New(
      const BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config, std::shared_ptr<Context> context) final;
};

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_UEFI_H_
