// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/digest.h"

#include <gtest/gtest.h>

#include "gmock/gmock.h"
#include "src/developer/memory/metrics/tests/test_utils.h"

namespace memory::test {
namespace {
using DigestUnitTest = testing::Test;

struct ExpectedBucket {
  std::string name;
  uint64_t size;

  auto operator<=>(const ExpectedBucket&) const = default;

  // Teaches gtest to pretty-print |ExpectedBucket| in assertions.
  friend void PrintTo(const ExpectedBucket& bucket, std::ostream* os) {
    *os << "Bucket{.name=" << bucket.name << ", .size=" << bucket.size << "}";
  }
};

void ConfirmNonEmptyBuckets(const Digest& digest,
                            const std::vector<ExpectedBucket>& expected_buckets) {
  auto non_empty_buckets =
      digest.buckets() | std::views::filter([](auto& b) { return b.size() != 0; });
  std::vector<Bucket> buckets_copy{non_empty_buckets.begin(), non_empty_buckets.end()};
  for (const auto& expected_bucket : expected_buckets) {
    bool found = false;
    for (auto bucket = buckets_copy.begin(); bucket != buckets_copy.end(); bucket++) {
      if (expected_bucket.name == bucket->name()) {
        EXPECT_EQ(expected_bucket.size, bucket->size())
            << "Bucket name='" << expected_bucket.name << "' has an unexpected value";
        buckets_copy.erase(bucket);
        found = true;
        break;
      }
    }
    EXPECT_TRUE(found) << "Bucket name='" << expected_bucket.name << "' is missing";
  }
  for (const auto& unmatched_bucket : buckets_copy) {
    EXPECT_TRUE(false) << "Bucket name='" << unmatched_bucket.name() << "' is unexpected";
  }
}

TEST_F(DigestUnitTest, VMONames) {
  Capture c;
  TestUtils::CreateCapture(
      &c,
      {
          .vmos =
              {
                  {.koid = 1, .name = "a1", .committed_bytes = 100, .committed_scaled_bytes = 100},
                  {.koid = 2, .name = "b1", .committed_bytes = 200, .committed_scaled_bytes = 200},
              },
          .processes =
              {
                  {.koid = 1, .name = "p1", .vmos = {1}},
                  {.koid = 2, .name = "q1", .vmos = {2}},
              },
      });

  Digester digester({{"A", "", "a.*"}, {"B", ".*", "b.*"}});
  Digest d(c, &digester);
  ConfirmNonEmptyBuckets(d, {{.name = "B", .size = 200U}, {.name = "A", .size = 100U}});
  EXPECT_EQ(0U, d.undigested_vmos().size());
}  // namespace test

TEST_F(DigestUnitTest, ProcessNames) {
  Capture c;
  TestUtils::CreateCapture(
      &c,
      {
          .vmos =
              {
                  {.koid = 1, .name = "a1", .committed_bytes = 100, .committed_scaled_bytes = 100},
                  {.koid = 2, .name = "b1", .committed_bytes = 200, .committed_scaled_bytes = 200},
              },
          .processes =
              {
                  {.koid = 1, .name = "p1", .vmos = {1}},
                  {.koid = 2, .name = "q1", .vmos = {2}},
              },
      });

  Digester digester({{"P", "p.*", ""}, {"Q", "q.*", ".*"}});
  Digest d(c, &digester);
  ConfirmNonEmptyBuckets(d, {{.name = "Q", .size = 200U}, {.name = "P", .size = 100U}});
  EXPECT_EQ(0U, d.undigested_vmos().size());
}

TEST_F(DigestUnitTest, Undigested) {
  Capture c;
  TestUtils::CreateCapture(
      &c,
      {
          .vmos =
              {
                  {.koid = 1, .name = "a1", .committed_bytes = 100, .committed_scaled_bytes = 100},
                  {.koid = 2, .name = "b1", .committed_bytes = 200, .committed_scaled_bytes = 200},
              },
          .processes =
              {
                  {.koid = 1, .name = "p1", .vmos = {1}},
                  {.koid = 2, .name = "q1", .vmos = {2}},
              },
      });

  Digester digester({{"A", ".*", "a.*"}});
  Digest d(c, &digester);
  ASSERT_EQ(1U, d.undigested_vmos().size());
  ASSERT_NE(d.undigested_vmos().end(), d.undigested_vmos().find(2U));
  ConfirmNonEmptyBuckets(d, {{.name = "A", .size = 100U}, {.name = "Undigested", .size = 200U}});
}  // namespace test

TEST_F(DigestUnitTest, Kernel) {
  // Test kernel stats.
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .kmem =
                                       {
                                           .total_bytes = 1000,
                                           .free_bytes = 100,
                                           .wired_bytes = 10,
                                           .total_heap_bytes = 20,
                                           .mmu_overhead_bytes = 30,
                                           .ipc_bytes = 40,
                                           .other_bytes = 50,
                                       },
                               });
  Digester digester({});
  Digest d(c, &digester);
  EXPECT_EQ(0U, d.undigested_vmos().size());
  ConfirmNonEmptyBuckets(d, {{.name = "Kernel", .size = 150U}, {.name = "Free", .size = 100U}});
}

TEST_F(DigestUnitTest, Orphaned) {
  // Test kernel stats.
  Capture c;
  TestUtils::CreateCapture(
      &c,
      {
          .kmem =
              {
                  .total_bytes = 1000,
                  .vmo_bytes = 300,
              },
          .vmos =
              {
                  {.koid = 1, .name = "a1", .committed_bytes = 100, .committed_scaled_bytes = 100},
              },
          .processes =
              {
                  {.koid = 1, .name = "p1", .vmos = {1}},
              },
      });
  Digester digester({{"A", ".*", "a.*"}});
  Digest d(c, &digester);
  EXPECT_EQ(0U, d.undigested_vmos().size());
  ConfirmNonEmptyBuckets(d, {{.name = "A", .size = 100U}, {.name = "Orphaned", .size = 200U}});
}

TEST_F(DigestUnitTest, DefaultBuckets) {
  // Test kernel stats.
  Capture c;
  TestUtils::CreateCapture(
      &c,
      {.vmos =
           {
               {.koid = 1,
                .name = "uncompressed-bootfs",
                .committed_bytes = 1,
                .committed_scaled_bytes = 1},
               {.koid = 2,
                .name = "magma_create_buffer",
                .committed_bytes = 2,
                .committed_scaled_bytes = 2},
               {.koid = 3,
                .name = "SysmemAmlogicProtectedPool",
                .committed_bytes = 3,
                .committed_scaled_bytes = 3},
               {.koid = 4,
                .name = "SysmemContiguousPool",
                .committed_bytes = 4,
                .committed_scaled_bytes = 4},
               {.koid = 5, .name = "test", .committed_bytes = 5, .committed_scaled_bytes = 5},
               {.koid = 6, .name = "test", .committed_bytes = 6, .committed_scaled_bytes = 6},
               {.koid = 7, .name = "test", .committed_bytes = 7, .committed_scaled_bytes = 7},
               {.koid = 8, .name = "dart", .committed_bytes = 8, .committed_scaled_bytes = 8},
               {.koid = 9, .name = "test", .committed_bytes = 9, .committed_scaled_bytes = 9},
               {.koid = 10, .name = "test", .committed_bytes = 10, .committed_scaled_bytes = 10},
               {.koid = 11, .name = "test", .committed_bytes = 11, .committed_scaled_bytes = 11},
               {.koid = 12, .name = "test", .committed_bytes = 12, .committed_scaled_bytes = 12},
               {.koid = 13, .name = "test", .committed_bytes = 13, .committed_scaled_bytes = 13},
               {.koid = 14, .name = "test", .committed_bytes = 14, .committed_scaled_bytes = 14},
               {.koid = 15, .name = "test", .committed_bytes = 15, .committed_scaled_bytes = 15},
               {.koid = 16, .name = "test", .committed_bytes = 16, .committed_scaled_bytes = 16},
               {.koid = 17, .name = "test", .committed_bytes = 17, .committed_scaled_bytes = 17},
               {.koid = 18, .name = "test", .committed_bytes = 18, .committed_scaled_bytes = 18},
               {.koid = 19, .name = "test", .committed_bytes = 19, .committed_scaled_bytes = 19},
               {.koid = 20, .name = "test", .committed_bytes = 20, .committed_scaled_bytes = 20},
               {.koid = 21, .name = "test", .committed_bytes = 21, .committed_scaled_bytes = 21},
               {.koid = 22, .name = "test", .committed_bytes = 22, .committed_scaled_bytes = 22},
               {.koid = 23,
                .name = "inactive-blob-123",
                .committed_bytes = 23,
                .committed_scaled_bytes = 23},
               {.koid = 24,
                .name = "blob-abc",
                .committed_bytes = 24,
                .committed_scaled_bytes = 24},
               {.koid = 25,
                .name = "Mali JIT memory",
                .committed_bytes = 25,
                .committed_scaled_bytes = 25},
               {.koid = 26,
                .name = "MagmaProtectedSysmem",
                .committed_bytes = 26,
                .committed_scaled_bytes = 26},
               {.koid = 27,
                .name = "ImagePipe2Surface:0",
                .committed_bytes = 27,
                .committed_scaled_bytes = 27},
               {.koid = 28,
                .name = "GFXBufferCollection:1",
                .committed_bytes = 28,
                .committed_scaled_bytes = 28},
               {.koid = 29,
                .name = "ScenicImageMemory",
                .committed_bytes = 29,
                .committed_scaled_bytes = 29},
               {.koid = 30,
                .name = "Display:0",
                .committed_bytes = 30,
                .committed_scaled_bytes = 30},
               {.koid = 31,
                .name = "Display-Protected:0",
                .committed_bytes = 31,
                .committed_scaled_bytes = 31},
               {.koid = 32,
                .name = "CompactImage:0",
                .committed_bytes = 32,
                .committed_scaled_bytes = 32},
               {.koid = 33,
                .name = "GFX Device Memory CPU Uncached",
                .committed_bytes = 33,
                .committed_scaled_bytes = 33},
           },
       .processes = {
           {.koid = 1, .name = "bin/bootsvc", .vmos = {1}},
           {.koid = 2, .name = "test", .vmos = {2, 25, 26}},
           {.koid = 3, .name = "driver_host", .vmos = {3, 4}},
           {.koid = 4, .name = "fshost.cm", .vmos = {5}},
           {.koid = 5, .name = "/boot/bin/minfs", .vmos = {6}},
           {.koid = 6, .name = "/boot/bin/blobfs", .vmos = {7, 23, 24}},
           {.koid = 7, .name = "io.flutter.product_runner.aot", .vmos = {8, 9, 28, 29}},
           {.koid = 10, .name = "kronk.cm", .vmos = {10}},
           {.koid = 8, .name = "web_engine_exe:renderer", .vmos = {11}},
           {.koid = 9, .name = "web_engine_exe:gpu", .vmos = {12, 27, 32, 33}},
           {.koid = 11, .name = "scenic.cm", .vmos = {13, 27, 28, 29, 30, 31}},
           {.koid = 12, .name = "driver_host", .vmos = {14}},
           {.koid = 13, .name = "netstack.cm", .vmos = {15}},
           {.koid = 14, .name = "pkgfs", .vmos = {16}},
           {.koid = 15, .name = "cast_agent.cm", .vmos = {17}},
           {.koid = 16, .name = "archivist.cm", .vmos = {18}},
           {.koid = 17, .name = "cobalt.cm", .vmos = {19}},
           {.koid = 18, .name = "audio_core.cm", .vmos = {20}},
           {.koid = 19, .name = "context_provider.cm", .vmos = {21}},
           {.koid = 20, .name = "new", .vmos = {22}},
       }});

  const std::vector<BucketMatch> bucket_matches = {
      {"ZBI Buffer", ".*", "uncompressed-bootfs"},
      // Memory used with the GPU or display hardware.
      {"Graphics", ".*",
       "magma_create_buffer|Mali "
       ".*|Magma.*|ImagePipe2Surface.*|GFXBufferCollection.*|ScenicImageMemory|Display.*|"
       "CompactImage.*|GFX Device Memory.*"},
      // Unused protected pool memory.
      {"ProtectedPool", "driver_host", "SysmemAmlogicProtectedPool"},
      // Unused contiguous pool memory.
      {"ContiguousPool", "driver_host", "SysmemContiguousPool"},
      {"Fshost", "fshost.cm", ".*"},
      {"Minfs", ".*minfs", ".*"},
      {"BlobfsInactive", ".*blobfs", "inactive-blob-.*"},
      {"Blobfs", ".*blobfs", ".*"},
      {"FlutterApps", "io\\.flutter\\..*", "dart.*"},
      {"Flutter", "io\\.flutter\\..*", ".*"},
      {"Web", "web_engine_exe:.*", ".*"},
      {"Kronk", "kronk.cm", ".*"},
      {"Scenic", "scenic.cm", ".*"},
      {"Amlogic", "driver_host", ".*"},
      {"Netstack", "netstack.cm", ".*"},
      {"Pkgfs", "pkgfs", ".*"},
      {"Cast", "cast_agent.cm", ".*"},
      {"Archivist", "archivist.cm", ".*"},
      {"Cobalt", "cobalt.cm", ".*"},
      {"Audio", "audio_core.cm", ".*"},
      {"Context", "context_provider.cm", ".*"},
  };

  Digester digester(bucket_matches);
  Digest d(c, &digester);
  EXPECT_EQ(1U, d.undigested_vmos().size());

  ConfirmNonEmptyBuckets(
      d, {
             {.name = "Web", .size = 23U},
             {.name = "Context", .size = 21U},
             {.name = "Audio", .size = 20U},
             {.name = "Cobalt", .size = 19U},
             {.name = "Archivist", .size = 18U},
             {.name = "Cast", .size = 17U},
             {.name = "Pkgfs", .size = 16U},
             {.name = "Netstack", .size = 15U},
             {.name = "Amlogic", .size = 14U},
             {.name = "Scenic", .size = 13U},
             {.name = "Kronk", .size = 10U},
             {.name = "Flutter", .size = 9U},
             {.name = "FlutterApps", .size = 8U},
             {.name = "Blobfs", .size = 31U},
             {.name = "Minfs", .size = 6U},
             {.name = "Fshost", .size = 5U},
             {.name = "ContiguousPool", .size = 4U},
             {.name = "ProtectedPool", .size = 3U},
             {.name = "Graphics", .size = 2U + 25U + 26U + 27U + 28U + 29U + 30U + 31U + 32U + 33U},
             {.name = "ZBI Buffer", .size = 1U},
             {.name = "BlobfsInactive", .size = 23U},
             {.name = "Undigested", .size = 22U},
         });
}

TEST_F(DigestUnitTest, AllDefaultBuckets) {
  // Test kernel stats.
  Capture c;
  TestUtils::CreateCapture(
      &c,
      {.vmos =
           {
               {.koid = 1,
                .name = "uncompressed-bootfs",
                .committed_bytes = 1,
                .committed_scaled_bytes = 1},
               {.koid = 2,
                .name = "magma_create_buffer",
                .committed_bytes = 2,
                .committed_scaled_bytes = 2},
               {.koid = 3,
                .name = "SysmemAmlogicProtectedPool",
                .committed_bytes = 3,
                .committed_scaled_bytes = 3},
               {.koid = 4,
                .name = "SysmemContiguousPool",
                .committed_bytes = 4,
                .committed_scaled_bytes = 4},
               {.koid = 5, .name = "test", .committed_bytes = 5, .committed_scaled_bytes = 5},
               {.koid = 6, .name = "test", .committed_bytes = 6, .committed_scaled_bytes = 6},
               {.koid = 7, .name = "test", .committed_bytes = 7, .committed_scaled_bytes = 7},
               {.koid = 8, .name = "dart", .committed_bytes = 8, .committed_scaled_bytes = 8},
               {.koid = 9, .name = "test", .committed_bytes = 9, .committed_scaled_bytes = 9},
               {.koid = 10, .name = "test", .committed_bytes = 10, .committed_scaled_bytes = 10},
               {.koid = 11, .name = "test", .committed_bytes = 11, .committed_scaled_bytes = 11},
               {.koid = 12, .name = "test", .committed_bytes = 12, .committed_scaled_bytes = 12},
               {.koid = 13, .name = "test", .committed_bytes = 13, .committed_scaled_bytes = 13},
               {.koid = 14, .name = "test", .committed_bytes = 14, .committed_scaled_bytes = 14},
               {.koid = 15, .name = "test", .committed_bytes = 15, .committed_scaled_bytes = 15},
               {.koid = 16, .name = "test", .committed_bytes = 16, .committed_scaled_bytes = 16},
               {.koid = 17, .name = "test", .committed_bytes = 17, .committed_scaled_bytes = 17},
               {.koid = 18, .name = "test", .committed_bytes = 18, .committed_scaled_bytes = 18},
               {.koid = 19, .name = "test", .committed_bytes = 19, .committed_scaled_bytes = 19},
               {.koid = 20, .name = "test", .committed_bytes = 20, .committed_scaled_bytes = 20},
               {.koid = 21, .name = "test", .committed_bytes = 21, .committed_scaled_bytes = 21},
               {.koid = 22, .name = "test", .committed_bytes = 22, .committed_scaled_bytes = 22},
               {.koid = 23,
                .name = "inactive-blob-123",
                .committed_bytes = 23,
                .committed_scaled_bytes = 23},
               {.koid = 24,
                .name = "blob-abc",
                .committed_bytes = 24,
                .committed_scaled_bytes = 24},
               {.koid = 25,
                .name = "Mali JIT memory",
                .committed_bytes = 25,
                .committed_scaled_bytes = 25},
               {.koid = 26,
                .name = "MagmaProtectedSysmem",
                .committed_bytes = 26,
                .committed_scaled_bytes = 26},
               {.koid = 27,
                .name = "ImagePipe2Surface:0",
                .committed_bytes = 27,
                .committed_scaled_bytes = 27},
               {.koid = 28,
                .name = "GFXBufferCollection:1",
                .committed_bytes = 28,
                .committed_scaled_bytes = 28},
               {.koid = 29,
                .name = "ScenicImageMemory",
                .committed_bytes = 29,
                .committed_scaled_bytes = 29},
               {.koid = 30,
                .name = "Display:0",
                .committed_bytes = 30,
                .committed_scaled_bytes = 30},
               {.koid = 31,
                .name = "Display-Protected:0",
                .committed_bytes = 31,
                .committed_scaled_bytes = 31},
               {.koid = 32,
                .name = "CompactImage:0",
                .committed_bytes = 32,
                .committed_scaled_bytes = 32},
               {.koid = 33,
                .name = "GFX Device Memory CPU Uncached",
                .committed_bytes = 33,
                .committed_scaled_bytes = 33},
           },
       .processes = {
           {.koid = 1, .name = "bin/bootsvc", .vmos = {1}},
           {.koid = 2, .name = "test", .vmos = {2, 25, 26}},
           {.koid = 3, .name = "driver_host", .vmos = {3, 4}},
           {.koid = 4, .name = "fshost.cm", .vmos = {5}},
           {.koid = 5, .name = "/boot/bin/minfs", .vmos = {6}},
           {.koid = 6, .name = "/boot/bin/blobfs", .vmos = {7, 23, 24}},
           {.koid = 7, .name = "io.flutter.product_runner.aot", .vmos = {8, 9, 28, 29}},
           {.koid = 10, .name = "kronk.cm", .vmos = {10}},
           {.koid = 8, .name = "web_engine_exe:renderer", .vmos = {11}},
           {.koid = 9, .name = "web_engine_exe:gpu", .vmos = {12, 27, 32, 33}},
           {.koid = 11, .name = "scenic.cm", .vmos = {13, 27, 28, 29, 30, 31}},
           {.koid = 12, .name = "driver_host", .vmos = {14}},
           {.koid = 13, .name = "netstack.cm", .vmos = {15}},
           {.koid = 14, .name = "pkgfs", .vmos = {16}},
           {.koid = 15, .name = "cast_agent.cm", .vmos = {17}},
           {.koid = 16, .name = "archivist.cm", .vmos = {18}},
           {.koid = 17, .name = "cobalt.cm", .vmos = {19}},
           {.koid = 18, .name = "audio_core.cm", .vmos = {20}},
           {.koid = 19, .name = "context_provider.cm", .vmos = {21}},
           {.koid = 20, .name = "new", .vmos = {22}},
       }});

  const std::vector<BucketMatch> bucket_matches = {
      {"ZBI Buffer", ".*", "uncompressed-bootfs"},
      // Memory used with the GPU or display hardware.
      {"Graphics", ".*",
       "magma_create_buffer|Mali "
       ".*|Magma.*|ImagePipe2Surface.*|GFXBufferCollection.*|ScenicImageMemory|Display.*|"
       "CompactImage.*|GFX Device Memory.*"},
      // Unused protected pool memory.
      {"ProtectedPool", "driver_host", "SysmemAmlogicProtectedPool"},
      // Unused contiguous pool memory.
      {"ContiguousPool", "driver_host", "SysmemContiguousPool"},
      {"Fshost", "fshost.cm", ".*"},
      {"Minfs", ".*minfs", ".*"},
      {"BlobfsInactive", ".*blobfs", "inactive-blob-.*"},
      {"Blobfs", ".*blobfs", ".*"},
      {"FlutterApps", "io\\.flutter\\..*", "dart.*"},
      {"Flutter", "io\\.flutter\\..*", ".*"},
      {"Web", "web_engine_exe:.*", ".*"},
      {"Kronk", "kronk.cm", ".*"},
      {"Scenic", "scenic.cm", ".*"},
      {"Amlogic", "driver_host", ".*"},
      {"Netstack", "netstack.cm", ".*"},
      {"Pkgfs", "pkgfs", ".*"},
      {"Cast", "cast_agent.cm", ".*"},
      {"Archivist", "archivist.cm", ".*"},
      {"Cobalt", "cobalt.cm", ".*"},
      {"Audio", "audio_core.cm", ".*"},
      {"Context", "context_provider.cm", ".*"},
  };

  Digester digester(bucket_matches);
  Digest d(c, &digester);
  EXPECT_EQ(1U, d.undigested_vmos().size());
  std::vector<ExpectedBucket> expected_buckets{
      {.name = "Web", .size = 23U},
      {.name = "Context", .size = 21U},
      {.name = "Audio", .size = 20U},
      {.name = "Cobalt", .size = 19U},
      {.name = "Archivist", .size = 18U},
      {.name = "Cast", .size = 17U},
      {.name = "Pkgfs", .size = 16U},
      {.name = "Netstack", .size = 15U},
      {.name = "Amlogic", .size = 14U},
      {.name = "Scenic", .size = 13U},
      {.name = "Kronk", .size = 10U},
      {.name = "Flutter", .size = 9U},
      {.name = "FlutterApps", .size = 8U},
      {.name = "Blobfs", .size = 31U},
      {.name = "Minfs", .size = 6U},
      {.name = "Fshost", .size = 5U},
      {.name = "ContiguousPool", .size = 4U},
      {.name = "ProtectedPool", .size = 3U},
      {.name = "Graphics", .size = 2U + 25U + 26U + 27U + 28U + 29U + 30U + 31U + 32U + 33U},
      {.name = "ZBI Buffer", .size = 1U},
      {.name = "BlobfsInactive", .size = 23U},
      {.name = "Undigested", .size = 22U},
      {.name = "Orphaned", .size = 0},
      {.name = "Kernel", .size = 0},
      {.name = "Free", .size = 0},
      {.name = "[Addl]PagerTotal", .size = 0},
      {.name = "[Addl]PagerNewest", .size = 0},
      {.name = "[Addl]PagerOldest", .size = 0},
      {.name = "[Addl]DiscardableLocked", .size = 0},
      {.name = "[Addl]DiscardableUnlocked", .size = 0},
      {.name = "[Addl]ZramCompressedBytes", .size = 0}};

  auto actual_buckets_range = d.buckets() | std::views::transform([](auto& b) {
                                return ExpectedBucket{.name = b.name(), .size = b.size()};
                              });
  std::vector<ExpectedBucket> actual_buckets{actual_buckets_range.begin(),
                                             actual_buckets_range.end()};
  EXPECT_THAT(actual_buckets, testing::UnorderedElementsAreArray(expected_buckets));
}

TEST_F(DigestUnitTest, ProcessRename) {
  Digester digester({{"Flutter", "FlutterApp", ".*"}});

  Capture c1;
  TestUtils::CreateCapture(&c1, {
                                    .vmos = {{.koid = 10,
                                              .name = "vmo1",
                                              .committed_bytes = 100,
                                              .committed_scaled_bytes = 100}},
                                    .processes = {{.koid = 1, .name = "bin/app", .vmos = {10}}},
                                });

  {
    Digest d1(c1, &digester);
    ConfirmNonEmptyBuckets(d1, {{.name = "Undigested", .size = 100U}});
  }

  Capture c2;
  TestUtils::CreateCapture(&c2, {
                                    .vmos = {{.koid = 10,
                                              .name = "vmo1",
                                              .committed_bytes = 100,
                                              .committed_scaled_bytes = 100}},
                                    .processes = {{.koid = 1, .name = "FlutterApp", .vmos = {10}}},
                                });

  {
    Digest d2(c2, &digester);
    ConfirmNonEmptyBuckets(d2, {{.name = "Flutter", .size = 100U}});
  }
}

}  // namespace
}  // namespace memory::test
