// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_FIRMWARE_PAVER_ASTRO_H_
#define SRC_FIRMWARE_PAVER_ASTRO_H_

#include <lib/sysconfig/sync-client.h>

#include <span>

#include "src/firmware/paver/abr-client.h"
#include "src/firmware/paver/block-devices.h"
#include "src/firmware/paver/partition-client.h"
#include "src/firmware/paver/paver-context.h"
#include "src/firmware/paver/skip-block.h"

namespace paver {

class AstroPartitioner : public DevicePartitioner {
 public:
  enum class AbrWearLevelingOption { ON, OFF };

  static zx::result<std::unique_ptr<DevicePartitioner>> Initialize(
      const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
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

  zx::result<> Flush() const override;

  zx::result<> OnStop() const override { return zx::ok(); }

 private:
  AstroPartitioner(std::unique_ptr<SkipBlockDevicePartitioner> skip_block,
                   fidl::ClientEnd<fuchsia_io::Directory> svc_root,
                   std::shared_ptr<Context> context)
      : skip_block_(std::move(skip_block)),
        svc_root_(std::move(svc_root)),
        context_(std::move(context)) {}

  static zx::result<> InitializeContext(const paver::BlockDevices& skip_block_devices,
                                        AbrWearLevelingOption abr_wear_leveling_opt,
                                        Context* context);

  static bool CanSafelyUpdateLayout(std::shared_ptr<Context> context);

  std::unique_ptr<SkipBlockDevicePartitioner> skip_block_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_root_;

  std::shared_ptr<Context> context_;
};

class AstroPartitionerFactory : public DevicePartitionerFactory {
 public:
  zx::result<std::unique_ptr<DevicePartitioner>> New(
      const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config, std::shared_ptr<Context> context) final;
};

// Specialized astro sysconfig partition client built on SyncClientBuffered.
class AstroSysconfigPartitionClientBuffered : public PartitionClient {
 public:
  AstroSysconfigPartitionClientBuffered(std::shared_ptr<Context> context,
                                        ::sysconfig::SyncClient::PartitionType partition)
      : context_(context), partition_(partition) {}

  zx::result<size_t> GetBlockSize() final;
  zx::result<size_t> GetPartitionSize() final;
  zx::result<> Read(const zx::vmo& vmo, size_t size) final;
  zx::result<> Write(const zx::vmo& vmo, size_t vmo_size) final;
  zx::result<> Trim() final;
  zx::result<> Flush() final;

  // No copy, no move.
  AstroSysconfigPartitionClientBuffered(const AstroSysconfigPartitionClientBuffered&) = delete;
  AstroSysconfigPartitionClientBuffered& operator=(const AstroSysconfigPartitionClientBuffered&) =
      delete;
  AstroSysconfigPartitionClientBuffered(AstroSysconfigPartitionClientBuffered&&) = delete;
  AstroSysconfigPartitionClientBuffered& operator=(AstroSysconfigPartitionClientBuffered&&) =
      delete;

 private:
  std::shared_ptr<Context> context_;
  ::sysconfig::SyncClient::PartitionType partition_;
};

// Specialized layer on top of SkipBlockPartitionClient to deal with page0 quirk and block size
// quirk.
class Bl2PartitionClient final : public SkipBlockPartitionClient {
 public:
  explicit Bl2PartitionClient(fidl::ClientEnd<fuchsia_hardware_skipblock::SkipBlock> partition)
      : SkipBlockPartitionClient(std::move(partition)) {}

  explicit Bl2PartitionClient(SkipBlockPartitionClient&& client)
      : SkipBlockPartitionClient(std::move(client)) {}

  zx::result<size_t> GetBlockSize() final;
  zx::result<size_t> GetPartitionSize() final;
  zx::result<> Read(const zx::vmo& vmo, size_t size) final;
  zx::result<> Write(const zx::vmo& vmo, size_t vmo_size) final;

  // No copy, no move.
  Bl2PartitionClient(const Bl2PartitionClient&) = delete;
  Bl2PartitionClient& operator=(const Bl2PartitionClient&) = delete;
  Bl2PartitionClient(Bl2PartitionClient&&) = delete;
  Bl2PartitionClient& operator=(Bl2PartitionClient&&) = delete;

 private:
  static constexpr size_t kNandPageSize = 4 * 1024;
  static constexpr size_t kBl2Size = 64 * 1024;
};

class AstroPartitionerContext : public ContextBase {
 public:
  std::unique_ptr<::sysconfig::SyncClientBuffered> client_;

  explicit AstroPartitionerContext(std::unique_ptr<::sysconfig::SyncClientBuffered> client)
      : client_{std::move(client)} {}
};

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_ASTRO_H_
