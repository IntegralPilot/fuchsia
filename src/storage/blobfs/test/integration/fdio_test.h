// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_TEST_INTEGRATION_FDIO_TEST_H_
#define SRC_STORAGE_BLOBFS_TEST_INTEGRATION_FDIO_TEST_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/zx/resource.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/storage/blobfs/component_runner.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/unit/local_decompressor_creator.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"

namespace blobfs {

// Test harness that sets up a blobfs and fdio backed by a FakeBlockDevice.
class FdioTest : public testing::Test {
 public:
  FdioTest() = default;

  void SetUp() override;
  void TearDown() override;

 protected:
  async::Loop* loop() { return loop_.get(); }

  int outgoing_dir_fd() const { return outgoing_dir_fd_.get(); }
  // get a clone of the outgoing dir in handle form
  zx_handle_t outgoing_dir();
  int root_fd() const { return root_fd_.get(); }
  block_client::FakeBlockDevice* block_device() { return block_device_; }
  const BlobCreatorWrapper& blob_creator() const { return *blob_creator_; }
  const BlobReaderWrapper& blob_reader() const { return *blob_reader_; }

  void set_mount_options(MountOptions options) { mount_options_ = options; }

  // Fetches a fresh Inspect snapshot from the running blobfs instance.
  void TakeSnapshot(inspect::Hierarchy* output);

  // Takes an inspect snapshot `hierarchy` and navigates through the nodes using
  // the `path` given and fetches the `property` there to be stored in `value`.
  static void GetUintMetricFromHierarchy(const inspect::Hierarchy& hierarchy,
                                         const std::vector<std::string>& path,
                                         const std::string& property, uint64_t* value);

  // Strings together `TakeSnapshot` and `GetUintMetricFromHierarchy` to fetch a
  // single value from a fresh snapshot.
  void GetUintMetric(const std::vector<std::string>& path, const std::string& property,
                     uint64_t* value);

  virtual uint64_t GetOldestMinorVersion() const { return kBlobfsCurrentMinorVersion; }

 private:
  block_client::FakeBlockDevice* block_device_ = nullptr;  // Owned by the runner_.

  MountOptions mount_options_;
  fbl::unique_fd outgoing_dir_fd_;
  fbl::unique_fd root_fd_;
  std::unique_ptr<ComponentRunner> runner_;
  std::unique_ptr<LocalDecompressorCreator> decompressor_creator_;

  std::unique_ptr<async::Loop> loop_;  // Must be destroyed after the runner.
  std::unique_ptr<BlobCreatorWrapper> blob_creator_;
  std::unique_ptr<BlobReaderWrapper> blob_reader_;
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_TEST_INTEGRATION_FDIO_TEST_H_
