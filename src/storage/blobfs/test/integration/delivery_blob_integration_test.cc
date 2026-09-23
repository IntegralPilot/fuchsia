// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/result.h>

#include <cerrno>
#include <cstring>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/integration/blobfs_fixtures.h"
#include "src/storage/fs_test/fs_test.h"

namespace blobfs {
namespace {

using fs_test::TestFilesystemOptions;

std::vector<TestBlobData> TestBlobs() {
  std::vector<TestBlobData> blobs;
  blobs.push_back(TestBlobData::Create(0));             // Null Blob
  blobs.push_back(TestBlobData::CreateRandom(1024ul));  // Random smaller than 1 block
  blobs.push_back(
      TestBlobData::CreateRandom(kBlobfsBlockSize * 20ul));   // Random larger than 1 block
  blobs.push_back(TestBlobData::CreateRealistic(1ul << 16));  // Realistic 64k blob
  return blobs;
}

class DeliveryBlobIntegrationTest : public BaseBlobfsTest {
 protected:
  DeliveryBlobIntegrationTest() : BaseBlobfsTest(TestFilesystemOptions::DefaultBlobfs()) {}
};

// Verify we can write uncompressed delivery blobs.
TEST_F(DeliveryBlobIntegrationTest, WriteUncompressed) {
  for (const TestBlobData& blob : TestBlobs()) {
    const auto delivery_data = TestDeliveryBlob::CreateUncompressed(blob);

    // Write delivery blob and verify the blob.
    ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_data).status_value());
    ASSERT_OK(blob_reader().VerifyBlob(blob));
  }
}

// Verify we can write compressed delivery blobs.
TEST_F(DeliveryBlobIntegrationTest, WriteCompressed) {
  for (const TestBlobData& blob : TestBlobs()) {
    const auto delivery_data = TestDeliveryBlob::CreateCompressed(blob);

    // Write delivery blob and verify the blob.
    ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_data).status_value());
    ASSERT_OK(blob_reader().VerifyBlob(blob));
  }
}

}  // namespace
}  // namespace blobfs
