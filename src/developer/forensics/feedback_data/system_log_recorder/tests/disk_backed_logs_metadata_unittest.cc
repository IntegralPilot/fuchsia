// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/disk_backed_logs_metadata.h"

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback_data/system_log_recorder/writer.h"
#include "src/developer/forensics/testing/scoped_memfs_manager.h"
#include "src/developer/forensics/testing/stubs/cobalt_logger_factory.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/lib/files/file.h"
#include "src/lib/timekeeper/test_clock.h"

namespace forensics::feedback_data::system_log_recorder {
namespace {

using ::testing::UnorderedElementsAreArray;

constexpr const char* kRootDirectory = "/root";
constexpr const char* kMetadataPath = "/root/disk_backed_logs_metadata.json";

class DiskBackedLogsMetadataTest : public UnitTestFixture {
 protected:
  void SetUp() override { memfs_manager_.Create(kRootDirectory); }

 private:
  testing::ScopedMemFsManager memfs_manager_;
};

TEST_F(DiskBackedLogsMetadataTest, IsAtCapacityWhenFirstFileNumIsMissing) {
  const DiskBackedLogsMetadata metadata(
      {
          {
              1,
              LogStats(/*message_count=*/0, /*deduplicated_message_count=*/0,
                       /*first_timestamp=*/std::nullopt,
                       /*last_timestamp=*/std::nullopt),
          },
      },
      SystemLogWriter::kFirstFileNumber);
  EXPECT_TRUE(metadata.IsAtCapacity());
}

TEST_F(DiskBackedLogsMetadataTest, IsNotAtCapacityWhenFirstFileNumIsPresent) {
  const DiskBackedLogsMetadata metadata(
      {
          {
              0,
              LogStats(/*message_count=*/0, /*deduplicated_message_count=*/0,
                       /*first_timestamp=*/std::nullopt,
                       /*last_timestamp=*/std::nullopt),
          },
      },
      SystemLogWriter::kFirstFileNumber);
  EXPECT_FALSE(metadata.IsAtCapacity());
}

TEST_F(DiskBackedLogsMetadataTest, IsNotAtCapacityWhenEmpty) {
  const DiskBackedLogsMetadata empty_metadata({}, SystemLogWriter::kFirstFileNumber);
  EXPECT_FALSE(empty_metadata.IsAtCapacity());
}

TEST_F(DiskBackedLogsMetadataTest, MessageCount) {
  LogStats stat1(/*message_count=*/200, /*deduplicated_message_count=*/0,
                 /*first_timestamp=*/std::nullopt, /*last_timestamp=*/std::nullopt);

  LogStats stat2(/*message_count=*/300, /*deduplicated_message_count=*/0,
                 /*first_timestamp=*/std::nullopt, /*last_timestamp=*/std::nullopt);

  const DiskBackedLogsMetadata metadata({{0, stat1}, {1, stat2}},
                                        SystemLogWriter::kFirstFileNumber);
  EXPECT_EQ(metadata.MessageCount(), 500u);
}

TEST_F(DiskBackedLogsMetadataTest, DeduplicatedMessageCount) {
  LogStats stat1(/*message_count=*/0, /*deduplicated_message_count=*/200,
                 /*first_timestamp=*/std::nullopt, /*last_timestamp=*/std::nullopt);

  LogStats stat2(/*message_count=*/0, /*deduplicated_message_count=*/300,
                 /*first_timestamp=*/std::nullopt, /*last_timestamp=*/std::nullopt);

  const DiskBackedLogsMetadata metadata({{0, stat1}, {1, stat2}},
                                        SystemLogWriter::kFirstFileNumber);
  EXPECT_EQ(metadata.DeduplicatedMessageCount(), 500u);
}

TEST_F(DiskBackedLogsMetadataTest, FirstTimestamp) {
  LogStats stat1(/*message_count=*/0, /*deduplicated_message_count=*/0,
                 /*first_timestamp=*/zx::time_boot(1000000), /*last_timestamp=*/std::nullopt);

  LogStats stat2(/*message_count=*/0, /*deduplicated_message_count=*/0,
                 /*first_timestamp=*/zx::time_boot(3000001), /*last_timestamp=*/std::nullopt);

  const DiskBackedLogsMetadata metadata({{0, stat1}, {1, stat2}},
                                        SystemLogWriter::kFirstFileNumber);
  EXPECT_EQ(metadata.FirstTimestamp(), zx::time_boot(1000000));
}

TEST_F(DiskBackedLogsMetadataTest, LastTimestamp) {
  LogStats stat1(/*message_count=*/0, /*deduplicated_message_count=*/0,
                 /*first_timestamp=*/std::nullopt, /*last_timestamp=*/zx::time_boot(3000000));

  LogStats stat2(/*message_count=*/0, /*deduplicated_message_count=*/0,
                 /*first_timestamp=*/std::nullopt, /*last_timestamp=*/zx::time_boot(5000000));

  const DiskBackedLogsMetadata metadata({{0, stat1}, {1, stat2}},
                                        SystemLogWriter::kFirstFileNumber);
  EXPECT_EQ(metadata.LastTimestamp(), zx::time_boot(5000000));
}

TEST_F(DiskBackedLogsMetadataTest, FromFileMissingFileReturnsNullopt) {
  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile("/root/nonexistent.json", SystemLogWriter::kFirstFileNumber);
  EXPECT_FALSE(read.has_value());
}

TEST_F(DiskBackedLogsMetadataTest, FromFileMalformedJsonReturnsNullopt) {
  ASSERT_TRUE(files::WriteFile(kMetadataPath, "{ malformed json "));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  EXPECT_FALSE(read.has_value());
}

TEST_F(DiskBackedLogsMetadataTest, FromFileInvalidKeysSkipped) {
  const std::string invalid_keys_json = R"({
    "file_stats": {
      "invalid_key": {
        "message_count": 10,
        "deduplicated_message_count": 5
      },
      "1": {
        "message_count": 100,
        "deduplicated_message_count": 70
      }
    }
  })";
  ASSERT_TRUE(files::WriteFile(kMetadataPath, invalid_keys_json));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(read.has_value());
  EXPECT_EQ(read->FileStats().size(), 1u);
  EXPECT_TRUE(read->FileStats().contains(1));
}

TEST_F(DiskBackedLogsMetadataTest, FromFileNotAnObjectReturnsNullopt) {
  ASSERT_TRUE(files::WriteFile(kMetadataPath, "[1, 2, 3]"));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  EXPECT_FALSE(read.has_value());
}

TEST_F(DiskBackedLogsMetadataTest, FromFileFileStatsMissing) {
  ASSERT_TRUE(files::WriteFile(kMetadataPath, "{}"));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(read.has_value());
  EXPECT_TRUE(read->Empty());
}

TEST_F(DiskBackedLogsMetadataTest, FromFileFileStatsWrongType) {
  ASSERT_TRUE(files::WriteFile(kMetadataPath, R"({"file_stats": "not_an_object"})"));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(read.has_value());
  EXPECT_TRUE(read->Empty());
}

TEST_F(DiskBackedLogsMetadataTest, FromFileFileStatValueWrongType) {
  const std::string json = R"({
    "file_stats": {
      "1": "not_an_object",
      "2": {
        "message_count": 100,
        "deduplicated_message_count": 70
      }
    }
  })";
  ASSERT_TRUE(files::WriteFile(kMetadataPath, json));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(read.has_value());
  EXPECT_EQ(read->FileStats().size(), 1u);
  EXPECT_TRUE(read->FileStats().contains(2));
}

TEST_F(DiskBackedLogsMetadataTest, FromFileMissingFieldsUseDefaultValue) {
  const std::string json = R"({
    "file_stats": {
      "1": {}
    }
  })";
  ASSERT_TRUE(files::WriteFile(kMetadataPath, json));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(read.has_value());
  ASSERT_EQ(read->FileStats().size(), 1u);
  ASSERT_TRUE(read->FileStats().contains(1));

  const LogStats& stat = read->FileStats().at(1);
  EXPECT_EQ(stat.message_count, 0u);
  EXPECT_EQ(stat.deduplicated_message_count, 0u);
  EXPECT_FALSE(stat.first_timestamp.has_value());
  EXPECT_FALSE(stat.last_timestamp.has_value());
}

TEST_F(DiskBackedLogsMetadataTest, FromFileFieldsPresent) {
  const std::string json = R"({
    "file_stats": {
      "1": {
        "message_count": 100,
        "deduplicated_message_count": 70,
        "first_timestamp_nanos": 1000000,
        "last_timestamp_nanos": 3000000
      }
    }
  })";
  ASSERT_TRUE(files::WriteFile(kMetadataPath, json));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(read.has_value());
  ASSERT_EQ(read->FileStats().size(), 1u);
  ASSERT_TRUE(read->FileStats().contains(1));

  const LogStats& stat = read->FileStats().at(1);
  EXPECT_EQ(stat.message_count, 100u);
  EXPECT_EQ(stat.deduplicated_message_count, 70u);
  ASSERT_TRUE(stat.first_timestamp.has_value());
  EXPECT_EQ(stat.first_timestamp, zx::time_boot(1000000));
  ASSERT_TRUE(stat.last_timestamp.has_value());
  EXPECT_EQ(stat.last_timestamp, zx::time_boot(3000000));
}

TEST_F(DiskBackedLogsMetadataTest, FromFileMessageCountWrongType) {
  const std::string json = R"({
    "file_stats": {
      "1": {
        "message_count": "invalid",
        "deduplicated_message_count": 70,
        "first_timestamp_nanos": 1000000,
        "last_timestamp_nanos": 3000000
      }
    }
  })";
  ASSERT_TRUE(files::WriteFile(kMetadataPath, json));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(read.has_value());
  ASSERT_EQ(read->FileStats().size(), 1u);
  ASSERT_TRUE(read->FileStats().contains(1));

  const LogStats& stat = read->FileStats().at(1);
  EXPECT_EQ(stat.message_count, 0u);
  EXPECT_EQ(stat.deduplicated_message_count, 70u);
  ASSERT_TRUE(stat.first_timestamp.has_value());
  EXPECT_EQ(stat.first_timestamp, zx::time_boot(1000000));
  ASSERT_TRUE(stat.last_timestamp.has_value());
  EXPECT_EQ(stat.last_timestamp, zx::time_boot(3000000));
}

TEST_F(DiskBackedLogsMetadataTest, FromFileDeduplicatedMessageCountWrongType) {
  const std::string json = R"({
    "file_stats": {
      "1": {
        "message_count": 100,
        "deduplicated_message_count": "invalid",
        "first_timestamp_nanos": 1000000,
        "last_timestamp_nanos": 3000000
      }
    }
  })";
  ASSERT_TRUE(files::WriteFile(kMetadataPath, json));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(read.has_value());
  ASSERT_EQ(read->FileStats().size(), 1u);
  ASSERT_TRUE(read->FileStats().contains(1));

  const LogStats& stat = read->FileStats().at(1);
  EXPECT_EQ(stat.message_count, 100u);
  EXPECT_EQ(stat.deduplicated_message_count, 0u);
  ASSERT_TRUE(stat.first_timestamp.has_value());
  EXPECT_EQ(stat.first_timestamp, zx::time_boot(1000000));
  ASSERT_TRUE(stat.last_timestamp.has_value());
  EXPECT_EQ(stat.last_timestamp, zx::time_boot(3000000));
}

TEST_F(DiskBackedLogsMetadataTest, FromFileFirstTimestampWrongType) {
  const std::string json = R"({
    "file_stats": {
      "1": {
        "message_count": 100,
        "deduplicated_message_count": 70,
        "first_timestamp_nanos": "invalid",
        "last_timestamp_nanos": 3000000
      }
    }
  })";
  ASSERT_TRUE(files::WriteFile(kMetadataPath, json));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(read.has_value());
  ASSERT_EQ(read->FileStats().size(), 1u);
  ASSERT_TRUE(read->FileStats().contains(1));

  const LogStats& stat = read->FileStats().at(1);
  EXPECT_EQ(stat.message_count, 100u);
  EXPECT_EQ(stat.deduplicated_message_count, 70u);
  EXPECT_FALSE(stat.first_timestamp.has_value());
  ASSERT_TRUE(stat.last_timestamp.has_value());
  EXPECT_EQ(stat.last_timestamp, zx::time_boot(3000000));
}

TEST_F(DiskBackedLogsMetadataTest, FromFileLastTimestampWrongType) {
  const std::string json = R"({
    "file_stats": {
      "1": {
        "message_count": 100,
        "deduplicated_message_count": 70,
        "first_timestamp_nanos": 1000000,
        "last_timestamp_nanos": "invalid"
      }
    }
  })";
  ASSERT_TRUE(files::WriteFile(kMetadataPath, json));

  const std::optional<DiskBackedLogsMetadata> read =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(read.has_value());
  ASSERT_EQ(read->FileStats().size(), 1u);
  ASSERT_TRUE(read->FileStats().contains(1));

  const LogStats& stat = read->FileStats().at(1);
  EXPECT_EQ(stat.message_count, 100u);
  EXPECT_EQ(stat.deduplicated_message_count, 70u);
  ASSERT_TRUE(stat.first_timestamp.has_value());
  EXPECT_EQ(stat.first_timestamp, zx::time_boot(1000000));
  EXPECT_FALSE(stat.last_timestamp.has_value());
}

TEST_F(DiskBackedLogsMetadataTest, ToFileEmptyReturnsFalse) {
  const DiskBackedLogsMetadata empty_metadata({}, SystemLogWriter::kFirstFileNumber);
  EXPECT_FALSE(empty_metadata.ToFile(kMetadataPath));
}

TEST_F(DiskBackedLogsMetadataTest, ToFileAndFromFile) {
  LogStats stat1(/*message_count=*/100, /*deduplicated_message_count=*/70,
                 /*first_timestamp=*/zx::time_boot(1000000),
                 /*last_timestamp=*/zx::time_boot(3000000));

  LogStats stat2(/*message_count=*/50, /*deduplicated_message_count=*/30,
                 /*first_timestamp=*/zx::time_boot(3000001),
                 /*last_timestamp=*/zx::time_boot(5000000));

  DiskBackedLogsMetadata metadata({{1, stat1}, {2, stat2}}, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(metadata.ToFile(kMetadataPath));
  EXPECT_TRUE(files::IsFile(kMetadataPath));

  const std::optional<DiskBackedLogsMetadata> result =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(result.has_value());
  EXPECT_TRUE(result->IsAtCapacity());
  EXPECT_EQ(result->MessageCount(), 150u);
  EXPECT_EQ(result->DeduplicatedMessageCount(), 100u);

  ASSERT_TRUE(result->FirstTimestamp().has_value());
  EXPECT_EQ(result->FirstTimestamp(), zx::time_boot(1000000));
  ASSERT_TRUE(result->LastTimestamp().has_value());
  EXPECT_EQ(result->LastTimestamp(), zx::time_boot(5000000));

  ASSERT_EQ(result->FileStats().size(), 2u);
  ASSERT_TRUE(result->FileStats().contains(1));
  EXPECT_EQ(result->FileStats().at(1).message_count, 100u);
  EXPECT_EQ(result->FileStats().at(1).deduplicated_message_count, 70u);
  ASSERT_TRUE(result->FileStats().at(1).first_timestamp.has_value());
  EXPECT_EQ(result->FileStats().at(1).first_timestamp, zx::time_boot(1000000));
  ASSERT_TRUE(result->FileStats().at(1).last_timestamp.has_value());
  EXPECT_EQ(result->FileStats().at(1).last_timestamp, zx::time_boot(3000000));

  ASSERT_TRUE(result->FileStats().contains(2));
  EXPECT_EQ(result->FileStats().at(2).message_count, 50u);
  EXPECT_EQ(result->FileStats().at(2).deduplicated_message_count, 30u);
  ASSERT_TRUE(result->FileStats().at(2).first_timestamp.has_value());
  EXPECT_EQ(result->FileStats().at(2).first_timestamp, zx::time_boot(3000001));
  ASSERT_TRUE(result->FileStats().at(2).last_timestamp.has_value());
  EXPECT_EQ(result->FileStats().at(2).last_timestamp, zx::time_boot(5000000));
}

TEST_F(DiskBackedLogsMetadataTest, ToFileAndFromFileWithMissingTimestamps) {
  LogStats stat(/*message_count=*/10, /*deduplicated_message_count=*/5,
                /*first_timestamp=*/std::nullopt, /*last_timestamp=*/std::nullopt);

  DiskBackedLogsMetadata metadata({{1, stat}}, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(metadata.ToFile(kMetadataPath));

  const std::optional<DiskBackedLogsMetadata> result =
      DiskBackedLogsMetadata::FromFile(kMetadataPath, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(result.has_value());
  ASSERT_EQ(result->FileStats().size(), 1u);
  ASSERT_TRUE(result->FileStats().contains(1));

  const LogStats& read_stat = result->FileStats().at(1);
  EXPECT_EQ(read_stat.message_count, 10u);
  EXPECT_EQ(read_stat.deduplicated_message_count, 5u);
  EXPECT_FALSE(read_stat.first_timestamp.has_value());
  EXPECT_FALSE(read_stat.last_timestamp.has_value());
}

TEST_F(DiskBackedLogsMetadataTest, Empty) {
  DiskBackedLogsMetadata metadata({}, SystemLogWriter::kFirstFileNumber);
  EXPECT_TRUE(metadata.Empty());
  EXPECT_EQ(metadata.NumFiles(), 0u);
}

TEST_F(DiskBackedLogsMetadataTest, NextFileNumber) {
  DiskBackedLogsMetadata empty_metadata({}, SystemLogWriter::kFirstFileNumber);
  EXPECT_EQ(empty_metadata.NextFileNumber(), SystemLogWriter::kFirstFileNumber);

  DiskBackedLogsMetadata metadata({}, SystemLogWriter::kFirstFileNumber);
  metadata.NewStats(0);
  EXPECT_EQ(metadata.NextFileNumber(), 1u);
  metadata.NewStats(5);
  EXPECT_EQ(metadata.NextFileNumber(), 6u);
}

TEST_F(DiskBackedLogsMetadataTest, NewStats) {
  DiskBackedLogsMetadata metadata({}, SystemLogWriter::kFirstFileNumber);
  metadata.NewStats(0);
  EXPECT_FALSE(metadata.Empty());
  EXPECT_EQ(metadata.NumFiles(), 1u);
  EXPECT_EQ(metadata.OldestFileNumber(), 0u);
  EXPECT_EQ(metadata.LatestFileNumber(), 0u);

  metadata.NewStats(1);
  EXPECT_EQ(metadata.NumFiles(), 2u);
  EXPECT_EQ(metadata.OldestFileNumber(), 0u);
  EXPECT_EQ(metadata.LatestFileNumber(), 1u);
}

TEST_F(DiskBackedLogsMetadataTest, MergeInto) {
  DiskBackedLogsMetadata metadata({}, SystemLogWriter::kFirstFileNumber);
  metadata.NewStats(0);

  LogStats stat1(/*message_count=*/10, /*deduplicated_message_count=*/8,
                 /*first_timestamp=*/zx::time_boot(100), /*last_timestamp=*/zx::time_boot(200));
  metadata.MergeInto(0, stat1);
  EXPECT_EQ(metadata.MessageCount(), 10u);
  EXPECT_EQ(metadata.DeduplicatedMessageCount(), 8u);
  ASSERT_TRUE(metadata.FirstTimestamp().has_value());
  EXPECT_EQ(metadata.FirstTimestamp(), zx::time_boot(100));
  ASSERT_TRUE(metadata.LastTimestamp().has_value());
  EXPECT_EQ(metadata.LastTimestamp(), zx::time_boot(200));

  LogStats stat2(/*message_count=*/5, /*deduplicated_message_count=*/3,
                 /*first_timestamp=*/zx::time_boot(150), /*last_timestamp=*/zx::time_boot(300));
  metadata.MergeInto(0, stat2);
  EXPECT_EQ(metadata.MessageCount(), 15u);
  EXPECT_EQ(metadata.DeduplicatedMessageCount(), 11u);
  ASSERT_TRUE(metadata.FirstTimestamp().has_value());
  EXPECT_EQ(metadata.FirstTimestamp(), zx::time_boot(100));
  ASSERT_TRUE(metadata.LastTimestamp().has_value());
  EXPECT_EQ(metadata.LastTimestamp(), zx::time_boot(300));
}

TEST_F(DiskBackedLogsMetadataTest, RemoveStats) {
  DiskBackedLogsMetadata metadata({}, SystemLogWriter::kFirstFileNumber);
  metadata.NewStats(0);
  metadata.NewStats(1);
  EXPECT_EQ(metadata.NumFiles(), 2u);

  metadata.RemoveStats(0);
  EXPECT_EQ(metadata.NumFiles(), 1u);
  EXPECT_EQ(metadata.OldestFileNumber(), 1u);
  EXPECT_EQ(metadata.LatestFileNumber(), 1u);
}

TEST_F(DiskBackedLogsMetadataTest, Clear) {
  DiskBackedLogsMetadata metadata({}, SystemLogWriter::kFirstFileNumber);
  metadata.NewStats(0);
  metadata.NewStats(1);
  EXPECT_FALSE(metadata.Empty());
  EXPECT_EQ(metadata.NumFiles(), 2u);

  metadata.Clear();
  EXPECT_TRUE(metadata.Empty());
  EXPECT_EQ(metadata.NumFiles(), 0u);
}

TEST_F(DiskBackedLogsMetadataTest, ReconcileWithExistingFiles) {
  LogStats stat1(/*message_count=*/100, /*deduplicated_message_count=*/70,
                 /*first_timestamp=*/zx::time_boot(1000000),
                 /*last_timestamp=*/zx::time_boot(3000000));
  LogStats stat2(/*message_count=*/50, /*deduplicated_message_count=*/30,
                 /*first_timestamp=*/zx::time_boot(3000001),
                 /*last_timestamp=*/zx::time_boot(5000000));

  DiskBackedLogsMetadata metadata({{1, stat1}, {2, stat2}}, SystemLogWriter::kFirstFileNumber);
  // Reconcile keeping file 2 and adding file 3. File 1 is dropped.
  metadata.ReconcileWithExistingFiles({2, 3});

  EXPECT_EQ(metadata.NumFiles(), 2u);
  EXPECT_FALSE(metadata.FileStats().contains(1));
  ASSERT_TRUE(metadata.FileStats().contains(2));
  ASSERT_TRUE(metadata.FileStats().contains(3));

  EXPECT_EQ(metadata.FileStats().at(2).message_count, 50u);
  EXPECT_EQ(metadata.FileStats().at(3).message_count, 0u);
}

class LogPreviousBootMetricsTest : public UnitTestFixture {};

TEST_F(LogPreviousBootMetricsTest, AtCapacity) {
  timekeeper::TestClock clock;
  cobalt::Logger cobalt(dispatcher(), services(), &clock);
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  LogStats stat(/*message_count=*/200, /*deduplicated_message_count=*/150,
                /*first_timestamp=*/zx::time_boot(zx::sec(2).get()),
                /*last_timestamp=*/zx::time_boot(zx::sec(124).get()));
  DiskBackedLogsMetadata metadata({{1, stat}}, SystemLogWriter::kFirstFileNumber);

  metadata.LogToCobalt(cobalt, zx::sec(124) + zx::msec(55));
  RunLoopUntilIdle();

  EXPECT_THAT(
      ReceivedCobaltEvents(),
      UnorderedElementsAreArray({
          cobalt::Event(cobalt::EventType::kInteger,
                        cobalt_registry::kPreviousBootLogMessageCountAtCapacityMetricId,
                        {cobalt_registry::FeedbackMetricDimensionProvider::Feedback}, 200),
          cobalt::Event(cobalt::EventType::kInteger,
                        cobalt_registry::kPreviousBootLogDeduplicatedMessageCountAtCapacityMetricId,
                        {cobalt_registry::FeedbackMetricDimensionProvider::Feedback}, 150),
          cobalt::Event(cobalt::EventType::kInteger,
                        cobalt_registry::kPreviousBootLogDurationAtCapacityMetricId,
                        {cobalt_registry::FeedbackMetricDimensionProvider::Feedback}, 2),
          cobalt::Event(cobalt::EventType::kInteger,
                        cobalt_registry::kPreviousBootLogLastTimestampDeltaMetricId,
                        {cobalt_registry::FeedbackMetricDimensionProvider::Feedback}, 55),
      }));
}

TEST_F(LogPreviousBootMetricsTest, NotAtCapacity) {
  timekeeper::TestClock clock;
  cobalt::Logger cobalt(dispatcher(), services(), &clock);
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  LogStats stat(/*message_count=*/200, /*deduplicated_message_count=*/150,
                /*first_timestamp=*/zx::time_boot(zx::sec(2).get()),
                /*last_timestamp=*/zx::time_boot(zx::sec(124).get()));
  DiskBackedLogsMetadata metadata({{0, stat}}, SystemLogWriter::kFirstFileNumber);

  metadata.LogToCobalt(cobalt, zx::sec(124) + zx::msec(55));
  RunLoopUntilIdle();

  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::EventType::kInteger,
                                cobalt_registry::kPreviousBootLogLastTimestampDeltaMetricId,
                                {cobalt_registry::FeedbackMetricDimensionProvider::Feedback}, 55),
              }));
}

TEST_F(LogPreviousBootMetricsTest, NegativeDelta) {
  timekeeper::TestClock clock;
  cobalt::Logger cobalt(dispatcher(), services(), &clock);
  SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));

  LogStats stat(/*message_count=*/100, /*deduplicated_message_count=*/80,
                /*first_timestamp=*/zx::time_boot(zx::sec(2).get()),
                /*last_timestamp=*/zx::time_boot(zx::sec(120).get()));
  DiskBackedLogsMetadata metadata({{0, stat}}, SystemLogWriter::kFirstFileNumber);

  metadata.LogToCobalt(cobalt, /*last_boot_uptime=*/zx::sec(100));
  RunLoopUntilIdle();
  EXPECT_THAT(
      ReceivedCobaltEvents(),
      UnorderedElementsAreArray({
          cobalt::Event(cobalt::EventType::kInteger,
                        cobalt_registry::kPreviousBootLogLastTimestampDeltaMetricId,
                        {cobalt_registry::FeedbackMetricDimensionProvider::Feedback}, -20000),
      }));
}

}  // namespace
}  // namespace forensics::feedback_data::system_log_recorder
