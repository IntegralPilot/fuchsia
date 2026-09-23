// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_BLOBFS_TEST_INTEGRATION_BLOBFS_FIXTURES_H_
#define SRC_STORAGE_BLOBFS_TEST_INTEGRATION_BLOBFS_FIXTURES_H_

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <lib/fdio/fd.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstdint>
#include <vector>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/fs_test/fs_test.h"
#include "src/storage/fs_test/fs_test_fixture.h"
#include "src/storage/lib/fs_management/cpp/options.h"

namespace blobfs {

class BaseBlobfsTest : public fs_test::BaseFilesystemTest {
 public:
  explicit BaseBlobfsTest(const fs_test::TestFilesystemOptions& options);

  int root_fd() { return root_fd_.get(); }
  const BlobCreatorWrapper& blob_creator() const { return blob_creator_; }
  const BlobReaderWrapper& blob_reader() const { return blob_reader_; }

  // Unlinks a blob.
  zx::result<> Unlink(const Digest& digest);

  // Lists all blobs.
  std::vector<Digest> ListBlobs() const;

  // Unmount, mount, and reconnect to blobfs.
  zx::result<> Remount();
  // Unmount, mount, and reconnect to blobfs.
  zx::result<> Remount(const fs_management::MountOptions& mount_options);

  // The root_fd, blob_creator, and blob_reader connects will be disconnected whe unmounting blobfs.
  // This method reconnects those protocols.
  zx::result<> Reconnect();

  zx::result<int> exec_root_fd() {
    if (!exec_root_fd_) {
      auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
      fuchsia_io::wire::Flags flags = fuchsia_io::wire::kPermReadable |
                                      fuchsia_io::wire::Flags::kPermInheritWrite |
                                      fuchsia_io::wire::Flags::kPermInheritExecute;
      const fidl::Status result = fidl::WireCall(fs().ServiceDirectory())
                                      ->Open("blob-exec", flags, {}, server.TakeChannel());
      if (!result.ok()) {
        return zx::error(result.status());
      }
      if (zx_status_t status =
              fdio_fd_create(client.TakeChannel().release(), exec_root_fd_.reset_and_get_address());
          status != ZX_OK) {
        return zx::error(status);
      }
    }
    return zx::ok(exec_root_fd_.get());
  }

 private:
  fbl::unique_fd root_fd_;
  fbl::unique_fd exec_root_fd_;
  BlobCreatorWrapper blob_creator_;
  BlobReaderWrapper blob_reader_;
};

// A test fixture for running tests with different blobfs settings.
class ParameterizedBlobfsTest : public BaseBlobfsTest,
                                public testing::WithParamInterface<fs_test::TestFilesystemOptions> {
 protected:
  ParameterizedBlobfsTest() : BaseBlobfsTest(GetParam()) {}
};

// Different blobfs settings to use with |ParameterizedBlobfsTest|.
fs_test::TestFilesystemOptions BlobfsDefaultTestParam();
fs_test::TestFilesystemOptions BlobfsWithFvmTestParam();
fs_test::TestFilesystemOptions BlobfsWithPaddedLayoutTestParam();
fs_test::TestFilesystemOptions BlobfsWithFixedDiskSizeTestParam(uint64_t disk_size);

// A test fixture for tests that only run against blobfs with the default settings.
class BlobfsTest : public BaseBlobfsTest {
 protected:
  explicit BlobfsTest() : BaseBlobfsTest(BlobfsDefaultTestParam()) {}
};

// A test fixture for tests that only run against blobfs with FVM.
class BlobfsWithFvmTest : public BaseBlobfsTest {
 protected:
  explicit BlobfsWithFvmTest() : BaseBlobfsTest(BlobfsWithFvmTestParam()) {}
};

}  // namespace blobfs

#endif  // SRC_STORAGE_BLOBFS_TEST_INTEGRATION_BLOBFS_FIXTURES_H_
