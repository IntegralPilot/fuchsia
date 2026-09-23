// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/system_time_tracker.h"

#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/lib/files/file.h"
#include "src/lib/files/scoped_temp_dir.h"
#include "src/lib/timekeeper/async_test_clock.h"
#include "third_party/rapidjson/include/rapidjson/document.h"

namespace forensics::feedback {
namespace {

class SystemTimeTrackerTest : public UnitTestFixture {
 public:
  SystemTimeTrackerTest() : clock_(dispatcher()) {}

  void SetUp() override { ASSERT_TRUE(tmp_dir_.NewTempFile(&system_time_path_)); }

  std::optional<int64_t> ReadUptime() const {
    std::string system_time_content;
    if (!files::ReadFileToString(system_time_path_, &system_time_content)) {
      return std::nullopt;
    }

    rapidjson::Document doc;
    doc.Parse(system_time_content.c_str());
    if (doc.HasParseError() || !doc.IsObject() || !doc.HasMember("uptime_ms") ||
        !doc["uptime_ms"].IsInt64()) {
      return std::nullopt;
    }

    return doc["uptime_ms"].GetInt64();
  }

  std::optional<int64_t> ReadRuntime() const {
    std::string system_time_content;
    if (!files::ReadFileToString(system_time_path_, &system_time_content)) {
      return std::nullopt;
    }

    rapidjson::Document doc;
    doc.Parse(system_time_content.c_str());
    if (doc.HasParseError() || !doc.IsObject() || !doc.HasMember("runtime_ms") ||
        !doc["runtime_ms"].IsInt64()) {
      return std::nullopt;
    }

    return doc["runtime_ms"].GetInt64();
  }

  int64_t GetBootTimeMs() const { return zx::duration(clock_.BootNow().to_timespec()).to_msecs(); }
  int64_t GetMonotonicTimeMs() const {
    return zx::duration(clock_.MonotonicNow().to_timespec()).to_msecs();
  }

 protected:
  timekeeper::AsyncTestClock clock_;
  files::ScopedTempDir tmp_dir_;
  std::string system_time_path_;
};

TEST_F(SystemTimeTrackerTest, WritesOnStart) {
  SystemTimeTracker tracker(dispatcher(), &clock_, zx::sec(1), system_time_path_);
  EXPECT_FALSE(ReadRuntime().has_value());
  EXPECT_FALSE(ReadUptime().has_value());

  tracker.Start();
  RunLoopUntilIdle();
  EXPECT_EQ(ReadRuntime(), GetMonotonicTimeMs());
  EXPECT_EQ(ReadUptime(), GetBootTimeMs());
}

TEST_F(SystemTimeTrackerTest, UpdatesFileEveryPeriod) {
  SystemTimeTracker tracker(dispatcher(), &clock_, zx::sec(1), system_time_path_);

  tracker.Start();

  RunLoopFor(zx::sec(1));
  EXPECT_EQ(ReadRuntime(), GetMonotonicTimeMs());
  EXPECT_EQ(ReadUptime(), GetBootTimeMs());

  RunLoopFor(zx::sec(2));
  EXPECT_EQ(ReadRuntime(), GetMonotonicTimeMs());
  EXPECT_EQ(ReadUptime(), GetBootTimeMs());
}

TEST_F(SystemTimeTrackerTest, FailsGracefullyOnBadPath) {
  SystemTimeTracker tracker(dispatcher(), &clock_, zx::sec(1), "/bad/path");
  tracker.Start();

  std::string system_time_content;
  EXPECT_FALSE(files::ReadFileToString("/bad/path", &system_time_content));

  RunLoopFor(zx::sec(1));
  EXPECT_FALSE(files::ReadFileToString("/bad/path", &system_time_content));

  const std::optional<SystemTime> previous_system_time = GetPreviousSystemTime(system_time_path_);
  EXPECT_FALSE(previous_system_time.has_value());
}

TEST_F(SystemTimeTrackerTest, RecordSystemShutdownSignal) {
  SystemTimeTracker tracker(dispatcher(), &clock_, zx::sec(1), system_time_path_);

  tracker.Start();

  RunLoopFor(zx::sec(1));
  EXPECT_EQ(ReadRuntime(), GetMonotonicTimeMs());
  EXPECT_EQ(ReadUptime(), GetBootTimeMs());

  RunLoopFor(zx::msec(500));
  tracker.RecordSystemShutdownSignal();
  RunLoopUntilIdle();
  EXPECT_EQ(ReadRuntime(), GetMonotonicTimeMs());
  EXPECT_EQ(ReadUptime(), GetBootTimeMs());
}

TEST_F(SystemTimeTrackerTest, GetPreviousSystemTimeSucceed) {
  SystemTimeTracker tracker(dispatcher(), &clock_, zx::sec(1), system_time_path_);

  tracker.Start();

  RunLoopFor(zx::sec(1));
  EXPECT_EQ(ReadRuntime(), GetMonotonicTimeMs());
  EXPECT_EQ(ReadUptime(), GetBootTimeMs());

  RunLoopFor(zx::sec(2));
  const std::optional<SystemTime> previous_system_time = GetPreviousSystemTime(system_time_path_);
  ASSERT_TRUE(previous_system_time.has_value());
  ASSERT_TRUE(previous_system_time->runtime.has_value());
  EXPECT_EQ(ReadRuntime(), previous_system_time->runtime->to_msecs());
  ASSERT_TRUE(previous_system_time->uptime.has_value());
  EXPECT_EQ(ReadUptime(), previous_system_time->uptime->to_msecs());
}

TEST_F(SystemTimeTrackerTest, GetPreviousSystemTimeEmptyFile) {
  ASSERT_TRUE(files::WriteFile(system_time_path_, ""));

  const std::optional<SystemTime> previous_system_time = GetPreviousSystemTime(system_time_path_);
  EXPECT_FALSE(previous_system_time.has_value());
}

TEST_F(SystemTimeTrackerTest, GetPreviousSystemTimeMissingRuntime) {
  ASSERT_TRUE(files::WriteFile(system_time_path_, R"({"uptime_ms":9876})"));

  const std::optional<SystemTime> previous_system_time = GetPreviousSystemTime(system_time_path_);
  ASSERT_TRUE(previous_system_time.has_value());
  ASSERT_TRUE(previous_system_time->uptime.has_value());
  EXPECT_EQ(*previous_system_time->uptime, zx::msec(9876));
  EXPECT_FALSE(previous_system_time->runtime.has_value());
}

TEST_F(SystemTimeTrackerTest, GetPreviousSystemTimeMissingUptime) {
  ASSERT_TRUE(files::WriteFile(system_time_path_, R"({"runtime_ms":8765})"));

  const std::optional<SystemTime> previous_system_time = GetPreviousSystemTime(system_time_path_);
  ASSERT_TRUE(previous_system_time.has_value());
  EXPECT_FALSE(previous_system_time->uptime.has_value());
  ASSERT_TRUE(previous_system_time->runtime.has_value());
  EXPECT_EQ(*previous_system_time->runtime, zx::msec(8765));
}

TEST_F(SystemTimeTrackerTest, GetPreviousSystemTimeInvalidUptime) {
  ASSERT_TRUE(
      files::WriteFile(system_time_path_, R"({"uptime_ms":"not a number","runtime_ms":8765})"));

  const std::optional<SystemTime> previous_system_time = GetPreviousSystemTime(system_time_path_);
  ASSERT_TRUE(previous_system_time.has_value());
  EXPECT_FALSE(previous_system_time->uptime.has_value());
  ASSERT_TRUE(previous_system_time->runtime.has_value());
  EXPECT_EQ(*previous_system_time->runtime, zx::msec(8765));
}

TEST_F(SystemTimeTrackerTest, GetPreviousSystemTimeInvalidRuntime) {
  ASSERT_TRUE(
      files::WriteFile(system_time_path_, R"({"uptime_ms":9876,"runtime_ms":"not a number"})"));

  const std::optional<SystemTime> previous_system_time = GetPreviousSystemTime(system_time_path_);
  ASSERT_TRUE(previous_system_time.has_value());
  ASSERT_TRUE(previous_system_time->uptime.has_value());
  EXPECT_EQ(*previous_system_time->uptime, zx::msec(9876));
  EXPECT_FALSE(previous_system_time->runtime.has_value());
}

}  // namespace
}  // namespace forensics::feedback
