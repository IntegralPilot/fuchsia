// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <cstddef>
#include <cstdint>
#include <cstdio>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/integration/blobfs_fixtures.h"

namespace blobfs {
namespace {

TEST_F(BlobfsTest, MaxReservation) {
  // Create and destroy kBlobfsDefaultInodeCount number of blobs.
  // This verifies that creating blobs does not lead to stray node reservations.
  // Refer to https://fxbug.dev/42131476 for the bug that lead to this test.
  size_t count = 0;
  for (uint64_t i = 0; i < kBlobfsDefaultInodeCount; i++) {
    auto blob = TestBlobData::CreatePrefixed(64, i);
    auto delivery_blob = TestDeliveryBlob::CreateUncompressed(blob);

    // Write the blob
    ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));

    // Delete the blob
    ASSERT_OK(Unlink(blob.digest())) << "Unlinking blob";

    if (++count % 1000 == 0) {
      fprintf(stderr, "Allocated and deleted %lu blobs\n", count);
    }
  }
}

}  // namespace
}  // namespace blobfs
