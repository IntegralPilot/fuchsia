// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/zx/result.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/errors.h>

#include <cstddef>
#include <cstdio>
#include <cstring>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/integration/blobfs_fixtures.h"

namespace blobfs {
namespace {

using FragmentationTest = ParameterizedBlobfsTest;
// The following test attempts to fragment the underlying blobfs partition
// assuming a trivial linear allocator. A more intelligent allocator may require
// modifications to this test.
TEST_P(FragmentationTest, Fragmentation) {
  // Keep generating blobs until we run out of space, in a pattern of large,
  // small, large, small, large.
  //
  // At the end of  the test, we'll free the small blobs, and observe if it is
  // possible to allocate a larger blob. With a simple allocator and no
  // defragmentation, this would result in a NO_SPACE error.
  constexpr size_t kSmallSize = (1 << 16);
  constexpr size_t kLargeSize = (1 << 17);

  std::vector<Digest> small_blobs;

  bool do_small_blob = true;
  bool capture_large_blob_storage_space_usage = true;
  size_t large_blob_storage_space_usage = 0;
  size_t count = 0;
  while (true) {
    auto blob = TestBlobData::CreatePrefixed(do_small_blob ? kSmallSize : kLargeSize, count);
    auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
    if (capture_large_blob_storage_space_usage && !do_small_blob) {
      // Record how much space was used by blobfs before writing a large blob.
      const zx::result fs_info = fs().GetFsInfo();
      ASSERT_OK(fs_info);
      large_blob_storage_space_usage = fs_info->used_bytes;
    }
    auto create_result = blob_creator().CreateAndWriteBlob(delivery_blob);
    if (create_result.is_error()) {
      ASSERT_STATUS(create_result, ZX_ERR_NO_SPACE);
      break;
    }
    if (capture_large_blob_storage_space_usage && !do_small_blob) {
      // Determine how much space was required to store the large by blob by comparing blobfs'
      // space usage before and after writing the blob.
      const zx::result fs_info = fs().GetFsInfo();
      ASSERT_OK(fs_info);
      large_blob_storage_space_usage = fs_info->used_bytes - large_blob_storage_space_usage;
      capture_large_blob_storage_space_usage = false;
    }
    if (do_small_blob) {
      small_blobs.emplace_back(blob.digest());
    }

    do_small_blob = !do_small_blob;

    if (++count % 50 == 0) {
      fprintf(stderr, "Allocated %lu blobs\n", count);
    }
  }

  // We have filled up the disk with both small and large blobs.
  // Observe that we cannot add another large blob.
  auto blob = TestBlobData::CreatePrefixed(kLargeSize, count + 1);
  auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
  ASSERT_STATUS(blob_creator().CreateAndWriteBlob(delivery_blob), ZX_ERR_NO_SPACE);

  // Unlink all small blobs -- except for the last one, since we may have free
  // trailing space at the end.
  for (const auto& digest : small_blobs) {
    ASSERT_OK(Unlink(digest)) << "Unlinking old blob";
  }

  // This asserts an assumption of our test: Freeing these blobs should provide
  // enough space.
  ASSERT_GT(kSmallSize * (small_blobs.size() - 1), kLargeSize);

  // Validate that we have enough space (before we try allocating)...
  const zx::result fs_info = fs().GetFsInfo();
  ASSERT_OK(fs_info);
  ASSERT_GE(fs_info->total_bytes - fs_info->used_bytes, large_blob_storage_space_usage);

  // Now that blobfs supports extents, verify that we can still allocate a large
  // blob, even if it is fragmented.
  ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

  // Sanity check that we can read the fragmented blob.
  ASSERT_OK(blob_reader().VerifyBlob(blob));

  // Sanity check that we can unlink the fragmented blob.
  ASSERT_OK(Unlink(blob.digest()));
}

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, FragmentationTest,
                         testing::Values(BlobfsDefaultTestParam(), BlobfsWithFvmTestParam(),
                                         BlobfsWithPaddedLayoutTestParam()),
                         testing::PrintToStringParamName());

}  // namespace
}  // namespace blobfs
