// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/bucket_match.h"

#include <zircon/syscalls/object.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/memory/metrics/tests/test_utils.h"
#include "zircon/system/public/zircon/types.h"

using testing::SizeIs;

namespace memory {
namespace test {

using BucketMatchUnitTest = testing::Test;

const std::string kValidConfiguration = R"([
    {
        "name": "ContiguousPool",
        "process": "driver_host",
        "vmo": "SysmemContiguousPool",
        "event_code": 1
    },
    {
        "name": "Blobfs",
        "process": ".*blobfs",
        "vmo": ".*",
        "event_code": 2
    }
])";

TEST_F(BucketMatchUnitTest, ValidConfiguration) {
  auto result = BucketMatch::ReadBucketMatchesFromConfig(kValidConfiguration);
  ASSERT_TRUE(result);

  auto bucket_matches = *result;
  EXPECT_THAT(bucket_matches, SizeIs(2));

  BucketMatch& match_0 = bucket_matches[0];
  EXPECT_EQ(match_0.name(), "ContiguousPool");
  EXPECT_EQ(match_0.event_code(), 1);
  EXPECT_TRUE(match_0.ProcessMatch(Process{1, 0, "driver_host", {}}));
  EXPECT_TRUE(match_0.VmoMatch("SysmemContiguousPool"));

  BucketMatch& match_1 = bucket_matches[1];
  EXPECT_EQ(match_1.name(), "Blobfs");
  EXPECT_EQ(match_1.event_code(), 2);
  EXPECT_TRUE(match_1.ProcessMatch(Process{1, 0, "active_blobfs", {}}));
  EXPECT_TRUE(match_1.VmoMatch("blob-01234"));
}

TEST_F(BucketMatchUnitTest, InvalidConfiguration) {
  // Missing "name"
  EXPECT_FALSE(BucketMatch::ReadBucketMatchesFromConfig(R"([{"process": "a", "vmo": ".*"}])"));
  // Missing "process"
  EXPECT_FALSE(BucketMatch::ReadBucketMatchesFromConfig(R"([{"name": "a", "vmo": ".*"}])"));
  // Missing "vmo"
  EXPECT_FALSE(BucketMatch::ReadBucketMatchesFromConfig(R"([{"name": "a", "process": ".*"}])"));

  // Badly formatted JSON
  EXPECT_FALSE(
      BucketMatch::ReadBucketMatchesFromConfig(R"([{"name": "a", "process": ".*", "vmo": ".*"]})"));
}

TEST_F(BucketMatchUnitTest, Sweep) {
  auto result = BucketMatch::ReadBucketMatchesFromConfig(kValidConfiguration);
  ASSERT_TRUE(result);

  auto bucket_matches = *result;
  BucketMatch& match_0 = bucket_matches[0];

  // Perform some matches to populate the cache
  EXPECT_TRUE(match_0.ProcessMatch(Process{1, 0, "driver_host", {}}));
  EXPECT_TRUE(match_0.VmoMatch("SysmemContiguousPool"));

  // Sweep. This will unmark all matches, but not delete any entry.
  match_0.Sweep();

  // Match one of the same process again, but not the VMO, and call Sweep. The Sweep will delete the
  // VMO cache entry.
  EXPECT_TRUE(match_0.ProcessMatch(Process{1, 0, "driver_host", {}}));
  match_0.Sweep();
}

}  // namespace test
}  // namespace memory
