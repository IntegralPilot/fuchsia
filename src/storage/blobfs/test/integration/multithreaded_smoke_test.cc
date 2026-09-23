// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/zx/vmo.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <random>
#include <thread>
#include <utility>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/compression_settings.h"
#include "src/storage/blobfs/format.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/integration/fdio_test.h"

namespace blobfs {
namespace {

using ::testing::UnitTest;

// With 32KB chunks coming from blobfs, we only have 160 page faults available for a 5MB file.
constexpr int kFileSize = 5 << 20;
constexpr int kChunkSize = 32 << 10;
constexpr int kReadsPerFile = kFileSize / kChunkSize;

class BlobfsMultithreadedSmokeTest : public FdioTest, public testing::WithParamInterface<int> {
 public:
  BlobfsMultithreadedSmokeTest() {
    MountOptions options;
    options.paging_threads = NumThreads();
    set_mount_options(options);
  }

  static int NumThreads() { return GetParam(); }
};

struct ReadLocation {
  size_t file;
  size_t offset;
};

void PerformReads(const ReadLocation* locations, size_t num_reads, const zx::vmo* vmos) {
  uint8_t ch;
  for (size_t i = 0; i < num_reads; ++i) {
    vmos[locations[i].file].read(&ch, locations[i].offset, 1);
  }
}

TEST_P(BlobfsMultithreadedSmokeTest, MultithreadedReads) {
  std::vector<Digest> blob_digests;
  // Add more files for more threads. We'll need it for scaling up the number of available pages to
  // fault in.
  for (int i = 0; i < NumThreads(); ++i) {
    auto blob = TestBlobData::CreateRealistic(kFileSize, i);
    auto delivery_blob = TestDeliveryBlob::CreateCompressed(blob);
    ASSERT_OK(blob_creator().CreateAndWriteBlob(delivery_blob));
    blob_digests.push_back(blob.digest());
  }

  std::vector<zx::vmo> vmos;
  for (const auto& digest : blob_digests) {
    auto vmo = blob_reader().GetVmo(digest);
    ASSERT_OK(vmo);
    vmos.emplace_back(std::move(*vmo));
  }

  // Generate every page fault possible, then scramble them up.
  std::vector<ReadLocation> reads(static_cast<size_t>(NumThreads() * kReadsPerFile));
  for (size_t file_id = 0; file_id < vmos.size(); ++file_id) {
    for (size_t offset = 0; offset < kReadsPerFile; ++offset) {
      reads[(file_id * kReadsPerFile) + offset] = {.file = file_id, .offset = offset * kChunkSize};
    }
  }
  std::shuffle(reads.begin(), reads.end(),
               std::mt19937(testing::UnitTest::GetInstance()->random_seed()));

  std::vector<std::thread> threads;
  for (size_t i = 0; static_cast<int>(i) < NumThreads(); i++) {
    threads.emplace_back(PerformReads, &reads[i * kReadsPerFile], kReadsPerFile, vmos.data());
  }

  for (auto& t : threads) {
    t.join();
  }
}

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, BlobfsMultithreadedSmokeTest, testing::Values(1, 2, 4),
                         testing::PrintToStringParamName());

}  // namespace
}  // namespace blobfs
