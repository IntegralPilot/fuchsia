// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/test/integration/blobfs_fixtures.h"

#include <dirent.h>
#include <fcntl.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/traits.h>
#include <lib/zx/result.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <cerrno>
#include <cstdint>
#include <vector>

#include <fbl/string.h>

#include "src/storage/blobfs/blob_layout.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/fs_test/fs_test.h"
#include "src/storage/fs_test/fs_test_fixture.h"
#include "src/storage/lib/fs_management/cpp/options.h"

namespace blobfs {

BaseBlobfsTest::BaseBlobfsTest(const fs_test::TestFilesystemOptions& options)
    : fs_test::BaseFilesystemTest(options),
      root_fd_(fs().GetRootFd()),
      blob_creator_(BlobCreatorWrapper::Connect(fs().ServiceDirectory())),
      blob_reader_(BlobReaderWrapper::Connect(fs().ServiceDirectory())) {
  ZX_ASSERT(root_fd_.is_valid());
}

zx::result<> BaseBlobfsTest::Unlink(const Digest& digest) {
  // This uses fuchsia.io instead of POSIX to avoid converting the error status to errno and back.
  fdio_cpp::UnownedFdioCaller caller(root_fd_.get());
  fbl::String blob_name = digest.ToString();
  auto result =
      fidl::WireCall(caller.directory())->Unlink(fidl::StringView::FromExternal(blob_name), {});
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result->is_error()) {
    return zx::error(result->error_value());
  }
  return zx::ok();
}

std::vector<Digest> BaseBlobfsTest::ListBlobs() const {
  auto dup = root_fd_.duplicate();
  ZX_ASSERT(dup.is_valid());
  DIR* dir = fdopendir(dup.get());
  ZX_ASSERT(dir != nullptr);
  struct dirent* de;
  errno = 0;
  std::vector<Digest> blobs;
  while ((de = readdir(dir)) != nullptr) {
    Digest digest;
    ZX_ASSERT_MSG(digest.Parse(de->d_name) == ZX_OK, "Failed to parse %s", de->d_name);
    blobs.push_back(digest);
  }
  ZX_ASSERT(errno == 0);
  closedir(dir);
  return blobs;
}

zx::result<> BaseBlobfsTest::Remount() { return Remount(fs().DefaultMountOptions()); }

zx::result<> BaseBlobfsTest::Remount(const fs_management::MountOptions& mount_options) {
  if (zx::result<> result = fs().Unmount(); result.is_error()) {
    return result;
  }
  if (zx::result<> result = fs().Mount(mount_options); result.is_error()) {
    return result;
  }
  return Reconnect();
}

zx::result<> BaseBlobfsTest::Reconnect() {
  root_fd_.reset(open(fs().mount_path().c_str(), O_DIRECTORY));
  ZX_ASSERT(root_fd_.is_valid());
  blob_creator_ = BlobCreatorWrapper::Connect(fs().ServiceDirectory());
  blob_reader_ = BlobReaderWrapper::Connect(fs().ServiceDirectory());
  return zx::ok();
}

fs_test::TestFilesystemOptions BlobfsDefaultTestParam() {
  auto options = fs_test::TestFilesystemOptions::BlobfsWithoutFvm();
  options.description = "Blobfs";
  return options;
}

fs_test::TestFilesystemOptions BlobfsWithFvmTestParam() {
  auto options = fs_test::TestFilesystemOptions::DefaultBlobfs();
  options.description = "BlobfsWithFvm";
  return options;
}

fs_test::TestFilesystemOptions BlobfsWithPaddedLayoutTestParam() {
  auto options = BlobfsDefaultTestParam();
  options.description = "BlobfsWithPaddedLayout";
  options.blob_layout_format = BlobLayoutFormat::kDeprecatedPaddedMerkleTreeAtStart;
  return options;
}

fs_test::TestFilesystemOptions BlobfsWithFixedDiskSizeTestParam(uint64_t disk_size) {
  auto options = BlobfsDefaultTestParam();
  options.description = "BlobfsWithFixedDiskSize";
  options.device_block_count = disk_size / options.device_block_size;
  return options;
}

}  // namespace blobfs
