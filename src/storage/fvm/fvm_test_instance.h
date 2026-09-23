// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_FVM_FVM_TEST_INSTANCE_H_
#define SRC_STORAGE_FVM_FVM_TEST_INSTANCE_H_

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/zx/result.h>

#include <memory>

#include <fbl/unique_fd.h>
#include <ramdevice-client/ramdisk.h>

#include "src/storage/lib/block_server/fake_server.h"
#include "src/storage/lib/fs_management/cpp/fvm.h"
#include "src/storage/lib/fs_management/cpp/mount.h"

namespace fvm {

enum class FvmImplementation : uint8_t { kDriver, kComponent };

constexpr char kFvmDriverLib[] = "fvm.cm";

constexpr uuid::Uuid kTestUniqueGuid1 = {0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
                                         0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f};
constexpr uuid::Uuid kTestUniqueGuid2 = {0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
                                         0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f};

constexpr std::string_view kTestPartDataName = "data";
constexpr uuid::Uuid kTestPartDataGuid = {
    0xAA, 0xFF, 0xBB, 0x00, 0x33, 0x44, 0x88, 0x99, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
};

constexpr std::string_view kTestPartBlobName = "blob";
constexpr uuid::Uuid kTestPartBlobGuid = {
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0xAA, 0xFF, 0xBB, 0x00, 0x33, 0x44, 0x88, 0x99,
};

constexpr std::string_view kTestPartSystemName = "system";
constexpr uuid::Uuid kTestPartSystemGuid = {
    0xEE, 0xFF, 0xBB, 0x00, 0x33, 0x44, 0x88, 0x99, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
};

class BlockConnector {
 public:
  virtual ~BlockConnector() = default;

  virtual fidl::ClientEnd<fuchsia_storage_block::Block> connect_block() const = 0;

  virtual fidl::UnownedClientEnd<fuchsia_storage_block::Block> as_block() const = 0;
};

struct AllocatePartitionRequest {
  size_t slice_count = 1;
  const uuid::Uuid& type;
  const uuid::Uuid& guid;
  const std::string_view& name;
  uint32_t flags = 0;
};

class FvmInstance {
 public:
  // Creates a ramdisk, destroying and recreating it if it already exists.
  void CreateRamdisk(uint64_t block_size, uint64_t block_count);

  // Creates a ramdisk and formats it with fvm.
  void CreateFvm(uint64_t block_size, uint64_t block_count, uint64_t slice_size);

  void StartFvm();

  // Rebinds or restarts the FVM instance.
  void RestartFvm();

  // Create a new ramdisk with a new total size. The block size must be the same as the existing
  // block size. This will start the disk and the fvm after recreating the disk.
  void RestartFvmWithNewDiskSize(uint64_t block_size, uint64_t block_count);

  // Get general info about fvm.
  fuchsia_storage_block::wire::VolumeManagerInfo GetFvmInfo() const;

  // Allocates a new partition.
  zx::result<std::unique_ptr<BlockConnector>> AllocatePartition(
      const AllocatePartitionRequest& request) const;

  // Opens an existing partition. This will wait for it to appear if it doesn't already exist.
  zx::result<std::unique_ptr<BlockConnector>> OpenPartition(std::string_view label) const;

  // Destroys the named partition, removing it from this fvm instance.
  void DestroyPartition(std::string_view label) const;

  // Returns the block interface of the underlying ramdisk.
  fidl::ClientEnd<fuchsia_storage_block::Block> GetRamdiskPartition() const;

 private:
  zx::vmo vmo_;
  std::unique_ptr<block_server::FakeServer> device_;
  fidl::ClientEnd<fuchsia_storage_block::Block> block_;
  fs_management::FsComponent component_ =
      fs_management::FsComponent::FromDiskFormat(fs_management::kDiskFormatFvm);
  std::unique_ptr<fs_management::StartedMultiVolumeFilesystem> fvm_;
  uint64_t slice_size_;
};

}  // namespace fvm

#endif  // SRC_STORAGE_FVM_FVM_TEST_INSTANCE_H_
