// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/monitor/high_water.h"

#include <fcntl.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/developer/memory/metrics/capture.h"
#include "src/developer/memory/metrics/digest.h"
#include "src/developer/memory/metrics/tests/test_utils.h"
#include "src/lib/files/file.h"
#include "src/lib/files/scoped_temp_dir.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace monitor {
namespace {

using memory::Capture;
using memory::CaptureLevel;
using memory::CaptureSupplier;
using memory::Digest;
using memory::Digester;

class HighWaterUnitTest : public gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    temp_dir_fd_.reset(open(temp_dir_.path().c_str(), O_RDONLY | O_DIRECTORY));
  }

  int dir_fd() { return temp_dir_fd_.get(); }

  const std::string& dir_path() { return temp_dir_.path(); }

 private:
  files::ScopedTempDir temp_dir_;
  fbl::unique_fd temp_dir_fd_;
};

TEST_F(HighWaterUnitTest, Basic) {
  CaptureSupplier cs(
      {{
           .kmem = {.free_bytes = 100},
       },
       {.kmem = {.free_bytes = 100},
        .vmos =
            {
                {.koid = 1, .name = "v1", .committed_bytes = 101, .committed_scaled_bytes = 101},
            },
        .processes = {
            {.koid = 2, .name = "p1", .vmos = {1}},
        }}});
  ASSERT_FALSE(files::IsFileAt(dir_fd(), "latest.txt"));
  HighWater hw(
      dir_path(), zx::msec(10), 100, dispatcher(),
      [&cs](Capture* c, CaptureLevel l) { return cs.GetCapture(c, l); },
      [](const Capture& c, Digest* d) { Digester({}).Digest(c, d); });
  ASSERT_FALSE(files::IsFileAt(dir_fd(), "latest.txt"));
  ASSERT_FALSE(files::IsFileAt(dir_fd(), "previous.txt"));
  ASSERT_FALSE(files::IsFileAt(dir_fd(), "latest_digest.txt"));
  ASSERT_FALSE(files::IsFileAt(dir_fd(), "previous_digest.txt"));
  RunLoopUntil([&cs] { return cs.empty(); });
  EXPECT_TRUE(files::IsFileAt(dir_fd(), "latest.txt"));
  EXPECT_TRUE(files::IsFileAt(dir_fd(), "latest_digest.txt"));
  EXPECT_FALSE(hw.GetHighWater().empty());
  EXPECT_FALSE(hw.GetHighWaterDigest().empty());
}

TEST_F(HighWaterUnitTest, RunTwice) {
  ASSERT_FALSE(files::IsFileAt(dir_fd(), "previous.txt"));
  ASSERT_FALSE(files::IsFileAt(dir_fd(), "latest.txt"));
  ASSERT_FALSE(files::IsFileAt(dir_fd(), "previous_digest.txt"));
  ASSERT_FALSE(files::IsFileAt(dir_fd(), "latest_digest.txt"));
  {
    CaptureSupplier cs(
        {{
             .kmem = {.free_bytes = 100},
         },
         {.kmem = {.free_bytes = 100},
          .vmos =
              {
                  {.koid = 1, .name = "v1", .committed_bytes = 101, .committed_scaled_bytes = 101},
              },
          .processes = {
              {.koid = 2, .name = "p1", .vmos = {1}},
          }}});
    HighWater hw(
        dir_path(), zx::msec(10), 100, dispatcher(),
        [&cs](Capture* c, CaptureLevel l) { return cs.GetCapture(c, l); },
        [](const Capture& c, Digest* d) { Digester({}).Digest(c, d); });
    RunLoopUntil([&cs] { return cs.empty(); });
    EXPECT_FALSE(hw.GetHighWater().empty());
  }
  EXPECT_TRUE(files::IsFileAt(dir_fd(), "latest.txt"));
  EXPECT_TRUE(files::IsFileAt(dir_fd(), "latest_digest.txt"));
  EXPECT_FALSE(files::IsFileAt(dir_fd(), "previous.txt"));
  EXPECT_FALSE(files::IsFileAt(dir_fd(), "previous_digest.txt"));
  {
    CaptureSupplier cs(
        {{
             .kmem = {.free_bytes = 100},
         },
         {.kmem = {.free_bytes = 100},
          .vmos =
              {
                  {.koid = 1, .name = "v1", .committed_bytes = 101, .committed_scaled_bytes = 101},
              },
          .processes = {
              {.koid = 2, .name = "p1", .vmos = {1}},
          }}});
    HighWater hw(
        dir_path(), zx::msec(10), 100, dispatcher(),
        [&cs](Capture* c, CaptureLevel l) { return cs.GetCapture(c, l); },
        [](const Capture& c, Digest* d) { Digester({}).Digest(c, d); });
    RunLoopUntil([&cs] { return cs.empty(); });
    EXPECT_FALSE(hw.GetHighWater().empty());
    EXPECT_FALSE(hw.GetPreviousHighWater().empty());
    EXPECT_FALSE(hw.GetHighWaterDigest().empty());
    EXPECT_FALSE(hw.GetPreviousHighWaterDigest().empty());
  }
  EXPECT_TRUE(files::IsFileAt(dir_fd(), "latest.txt"));
  EXPECT_TRUE(files::IsFileAt(dir_fd(), "latest_digest.txt"));
  EXPECT_TRUE(files::IsFileAt(dir_fd(), "previous.txt"));
  EXPECT_TRUE(files::IsFileAt(dir_fd(), "previous_digest.txt"));
}

}  // namespace
}  // namespace monitor
