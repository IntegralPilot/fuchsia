// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_PAVER_IRIS_H_
#define SRC_FIRMWARE_PAVER_IRIS_H_

#include <lib/zx/result.h>

#include <memory>
#include <span>

#include <hwreg/bitfields.h>

#include "src/firmware/paver/device-partitioner.h"

namespace paver {

class IrisPartitioner : public DevicePartitioner {
 public:
  static zx::result<std::unique_ptr<DevicePartitioner>> Initialize(
      const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config);

  bool SupportsPartition(const PartitionSpec& spec) const override;

  zx::result<std::unique_ptr<PartitionClient>> FindPartition(
      const PartitionSpec& spec) const override;

  zx::result<> ResetPartitionTables() const override;

  zx::result<> ValidatePayload(const PartitionSpec& spec,
                               std::span<const uint8_t> data) const override;

  const paver::BlockDevices& Devices() const override;

  fidl::UnownedClientEnd<fuchsia_io::Directory> SvcRoot() const override;

  zx::result<> Flush() const override { return zx::ok(); }

  zx::result<> OnStop() const override { return zx::ok(); }

  zx::result<std::unique_ptr<abr::Client>> CreateAbrClient() const override;

 private:
  zx::result<std::unique_ptr<PartitionClient>> OpenPartitionFromBlockDevices(
      std::string_view name) const;

  IrisPartitioner(BlockDevices devices, fidl::ClientEnd<fuchsia_io::Directory> svc_root)
      : devices_(std::move(devices)), svc_root_(std::move(svc_root)) {}

  BlockDevices devices_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_root_;
};

class IrisPartitionerFactory : public DevicePartitionerFactory {
 public:
  zx::result<std::unique_ptr<DevicePartitioner>> New(
      const paver::BlockDevices& devices, fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root,
      const PaverConfig& config, std::shared_ptr<Context> context) override;
};

// Iris stores the following format of slot metadata on devinfo partition.
#define IRIS_DEVINFO_MAGIC 0x49564544  // DEVI

struct iris_devinfo_ab_slot_data_t {
  uint8_t retry_count;
  uint8_t flags;
  uint8_t unused[2];

  DEF_SUBBIT(flags, 0, unbootable);
  DEF_SUBBIT(flags, 1, successful);
  DEF_SUBBIT(flags, 2, active);
  DEF_SUBBIT(flags, 3, fastboot_ok);
  DEF_SUBBIT(flags, 4, bl1_bootable);
} __attribute__((packed));

struct iris_devinfo_ab_data_t {
  iris_devinfo_ab_slot_data_t slots[2];
} __attribute__((packed));

// Hardcode offset so that we don't expose the entire devinfo structure.
constexpr size_t kIrisDevinfoSize = 8192;
constexpr size_t kIrisAbrMetadataOffset = 48;

static_assert(sizeof(iris_devinfo_ab_data_t) == 8, "devinfo_ab_data_t size must be 8");

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_IRIS_H_
