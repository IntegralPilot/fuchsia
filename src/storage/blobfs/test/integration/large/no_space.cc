// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/zx/result.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/errors.h>

#include <cstddef>
#include <cstdio>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/integration/blobfs_fixtures.h"

namespace blobfs {
namespace {

using NoSpaceTest = ParameterizedBlobfsTest;

TEST_P(NoSpaceTest, NoSpace) {
  Digest last_digest;

  // Keep generating blobs until we run out of space.
  size_t count = 0;
  while (true) {
    auto blob = TestBlobData::CreatePrefixed(1 << 17, count);
    auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);
    zx::result<> result = blob_creator().CreateAndWriteBlob(delivery_blob);
    if (result.is_error()) {
      ASSERT_STATUS(result, ZX_ERR_NO_SPACE) << "Blobfs expected to run out of space";
      // We ran out of space, as expected. Can we allocate if we
      // unlink a previously allocated blob of the desired size?
      ASSERT_OK(Unlink(last_digest)) << "Unlinking old blob";
      ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob)) << "Did not free enough space";
      // Yay! allocated successfully.
      break;
    }
    last_digest = delivery_blob.digest();

    if (++count % 50 == 0) {
      fprintf(stderr, "Allocated %lu blobs\n", count);
    }
  }
}

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, NoSpaceTest,
                         testing::Values(BlobfsDefaultTestParam(), BlobfsWithFvmTestParam(),
                                         BlobfsWithPaddedLayoutTestParam()),
                         testing::PrintToStringParamName());

}  // namespace
}  // namespace blobfs
