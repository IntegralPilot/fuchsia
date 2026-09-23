// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_TESTING_FVM_H_
#define SRC_STORAGE_TESTING_FVM_H_

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/zx/result.h>

#include <array>
#include <memory>
#include <optional>
#include <string>

#include "src/storage/lib/fs_management/cpp/mount.h"

namespace storage {

constexpr const char* kTestPartitionName = "fs-test-partition";

struct FvmOptions {
  std::string_view name = kTestPartitionName;

  // If not set, a test GUID type is used.
  std::optional<std::array<uint8_t, 16>> type;

  uint64_t initial_fvm_slice_count = 1;
};

// Manages an FVM component instance.
class FvmInstance {
 public:
  FvmInstance(fs_management::FsComponent component, fs_management::StartedMultiVolumeFilesystem fs)
      : component_(std::move(component)), fs_(std::move(fs)) {}

  fs_management::StartedMultiVolumeFilesystem& fs() { return fs_; }

 protected:
  fs_management::FsComponent component_;
  fs_management::StartedMultiVolumeFilesystem fs_;
};

// Manages a specific partition and a launched FVM component.
class FvmPartition {
 public:
  FvmPartition(FvmInstance fvm, fs_management::NamespaceBinding binding,
               std::string_view partition_name, std::string_view path)
      : fvm_(std::move(fvm)),
        binding_(std::move(binding)),
        partition_name_(partition_name),
        path_(path) {}

  zx::result<fidl::ClientEnd<fuchsia_storage_block::Block>> Connect() const;

  FvmInstance& fvm() { return fvm_; }
  const std::string& partition_name() const { return partition_name_; }
  const std::string& path() const { return path_; }

  // Sets the limit for the partition (in bytes).  See fuchsia.fs.startup.Volume::SetLimit.
  zx::result<> SetLimit(uint64_t limit);

 private:
  FvmInstance fvm_;
  fs_management::NamespaceBinding binding_;
  std::string partition_name_;
  std::string path_;
};

// Formats the given block device to be FVM managed, launches an FVM component, and creates a new
// partition on the device.
//
// Returns the path to the newly created block device.
zx::result<FvmPartition> CreateFvmPartition(const std::string& device_path, size_t slice_size,
                                            const FvmOptions& options = {});

// Launches an FVM component for the given device, and opens the partition which was previously
// created by `CreateFvmPartition`.
zx::result<FvmPartition> OpenFvmPartition(const std::string& device_path,
                                          std::string_view partition_name = kTestPartitionName);

}  // namespace storage

#endif  // SRC_STORAGE_TESTING_FVM_H_
