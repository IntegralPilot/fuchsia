// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/system_log_recorder.h"

#include <fidl/fuchsia.feedback.internal/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>

#include <memory>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/identity_decoder.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/identity_encoder.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/production_encoding.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/reader.h"
#include "src/developer/forensics/testing/stubs/diagnostics_archive.h"
#include "src/developer/forensics/testing/stubs/diagnostics_batch_iterator.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/vmo.h"
#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/files/scoped_temp_dir.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace forensics {
namespace feedback_data {
namespace system_log_recorder {
namespace {

using ::testing::ElementsAre;
using ::testing::HasSubstr;

namespace ffi = fuchsia_feedback_internal;

constexpr zx::duration kTimeWaitForLimitedLogs = zx::sec(60);

// Only change "X" for one character. i.e. X -> 12 is not allowed.
const StorageSize kMaxLogLineSize =
    StorageSize::Bytes(std::string("[15604.000][07559][07687][] INFO: line X\n").size());
const StorageSize kDroppedFormatStrSize =
    StorageSize::Bytes(std::string("!!! DROPPED X MESSAGES !!!\n").size());
const StorageSize kMaxDecompressedSize = StorageSize::Kilobytes(256);

TEST(Encoding, VerifyProductionEncoderDecoderVersion) {
  // Verify that the production decoder and encoder always have the same version.
  ProductionEncoder encoder;
  ProductionDecoder decoder;

  EXPECT_EQ(encoder.GetEncodingVersion(), decoder.GetEncodingVersion());
}

std::string BuildLogMessage(const std::string& message) {
  constexpr char fmt[] = R"JSON(
[
  {
    "metadata": {
      "timestamp": 15604000000000,
      "severity": "INFO",
      "pid": 7559,
      "tid": 7687
    },
    "payload": {
      "root": {
      "message": {
        "value": "%s"
        }
      }
    }
  }
]
)JSON";

  return fxl::StringPrintf(fmt, message.c_str());
}

using SystemLogRecorderSingleDispatcherTest = UnitTestFixture;
using SystemLogRecorderMultiDispatcherTest = UnitTestFixture;

TEST_F(SystemLogRecorderSingleDispatcherTest, SmokeTest) {
  // To simulate a real load, we set up the test with the following conditions:
  //  * The listener will get messages every 750 milliseconds.
  //  * The writer writes messages every 1 second. Each write will contain at most 2 log
  //    lines.
  //  * Each file will contain at most 2 log lines.
  //
  //    Using the above, we'll see log lines arrive with the at the following times:
  //    0.00: line0, line1, line2, line3 -> write 1 -> file 1
  //    0.75: line4, line5, line6, line7 -> write 1 -> file 1
  //    1.50: line8  -> write 2 -> file 2
  //    2.25: line9  -> write 3 -> file 2
  //    3.00: line10 -> write 4 -> file 2
  //    3.75: line11 -> write 4 -> file 2
  //    4.50: line12 -> write 5 -> file 3
  //    5.25: line13 -> write 6 -> file 3
  //
  // Note: we use the IdentityEncoder to easily control which messages are dropped.
  // Note 2: we offset time by kTimeWaitForLimitedLog to wait for the buffer rate limiter.
  const zx::duration kArchivePeriod = zx::msec(750);
  const zx::duration kWriterPeriod = zx::sec(1);

  const std::vector<std::vector<std::string>> json_batches({
      {
          BuildLogMessage("line 0"),
          BuildLogMessage("line 1"),
          BuildLogMessage("line 2"),
          BuildLogMessage("line 3"),

      },
      {
          BuildLogMessage("line 4"),
          BuildLogMessage("line 5"),
          BuildLogMessage("line 6"),
          BuildLogMessage("line 7"),
      },
      {BuildLogMessage("line 8")},
      {BuildLogMessage("line 9")},
      {BuildLogMessage("line A")},
      {BuildLogMessage("line B")},
      {BuildLogMessage("line C")},
      {BuildLogMessage("line D")},
      {},
  });

  stubs::DiagnosticsArchive archive(
      dispatcher(), std::make_unique<stubs::DiagnosticsBatchIteratorDelayedBatches>(
                        dispatcher(), json_batches, kTimeWaitForLimitedLogs, kArchivePeriod));

  InjectServiceProvider(&archive, kArchiveAccessorName);

  files::ScopedTempDir temp_dir;
  files::ScopedTempDir metadata_dir;
  const std::string metadata_path = files::JoinPath(metadata_dir.path(), "metadata.json");

  const StorageSize kWriteSize = kMaxLogLineSize * 2 + kDroppedFormatStrSize;

  SystemLogRecorder recorder(dispatcher(), dispatcher(), services(),
                             SystemLogRecorder::WriteParameters{
                                 .period = kWriterPeriod,
                                 .max_write_size = kWriteSize,
                                 .logs_dir = temp_dir.path(),
                                 .max_num_files = 2u,
                                 .total_log_size = 2u * kWriteSize,
                                 .metadata_path = metadata_path,
                                 .fallback_buffer_size = kFallbackLogBufferSize,
                             },
                             std::make_unique<IdentityRedactor>(inspect::BoolProperty()),
                             std::make_unique<IdentityEncoder>(),
                             std::make_unique<IdentityDecoder>());
  recorder.Start();

  RunLoopFor(kTimeWaitForLimitedLogs);

  IdentityDecoder decoder;

  RunLoopFor(kWriterPeriod);

  {
    float compression_ratio;
    const fit::result<ReaderError, std::string> result =
        Concatenate(temp_dir.path(), kMaxDecompressedSize, &decoder, &compression_ratio);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(compression_ratio, 1.0);
    EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 0
[15604.000][07559][07687][] INFO: line 1
!!! DROPPED 6 MESSAGES !!!
)");
  }

  RunLoopFor(kWriterPeriod);

  {
    float compression_ratio;
    const fit::result<ReaderError, std::string> result =
        Concatenate(temp_dir.path(), kMaxDecompressedSize, &decoder, &compression_ratio);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(compression_ratio, 1.0);
    EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 0
[15604.000][07559][07687][] INFO: line 1
!!! DROPPED 6 MESSAGES !!!
[15604.000][07559][07687][] INFO: line 8
)");
  }

  RunLoopFor(kWriterPeriod);

  {
    float compression_ratio;
    const fit::result<ReaderError, std::string> result =
        Concatenate(temp_dir.path(), kMaxDecompressedSize, &decoder, &compression_ratio);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(compression_ratio, 1.0);
    EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 0
[15604.000][07559][07687][] INFO: line 1
!!! DROPPED 6 MESSAGES !!!
[15604.000][07559][07687][] INFO: line 8
[15604.000][07559][07687][] INFO: line 9
)");
  }

  RunLoopFor(kWriterPeriod);

  {
    float compression_ratio;
    const fit::result<ReaderError, std::string> result =
        Concatenate(temp_dir.path(), kMaxDecompressedSize, &decoder, &compression_ratio);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(compression_ratio, 1.0);
    EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 8
[15604.000][07559][07687][] INFO: line 9
[15604.000][07559][07687][] INFO: line A
[15604.000][07559][07687][] INFO: line B
)");
  }

  RunLoopFor(kWriterPeriod);

  {
    float compression_ratio;
    const fit::result<ReaderError, std::string> result =
        Concatenate(temp_dir.path(), kMaxDecompressedSize, &decoder, &compression_ratio);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(compression_ratio, 1.0);
    EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 8
[15604.000][07559][07687][] INFO: line 9
[15604.000][07559][07687][] INFO: line A
[15604.000][07559][07687][] INFO: line B
[15604.000][07559][07687][] INFO: line C
)");
  }

  RunLoopFor(kWriterPeriod);

  {
    float compression_ratio;
    const fit::result<ReaderError, std::string> result =
        Concatenate(temp_dir.path(), kMaxDecompressedSize, &decoder, &compression_ratio);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(compression_ratio, 1.0);
    EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 8
[15604.000][07559][07687][] INFO: line 9
[15604.000][07559][07687][] INFO: line A
[15604.000][07559][07687][] INFO: line B
[15604.000][07559][07687][] INFO: line C
[15604.000][07559][07687][] INFO: line D
)");
  }
}

TEST_F(SystemLogRecorderSingleDispatcherTest, SingleThreaded_Flush) {
  // To simulate a real load, we set up the test with the following conditions:
  //  * The listener will get messages every 750 milliseconds.
  //  * The writer writes messages every 1 second. Each write will contain at most 2 log
  //    lines.
  //  * Each file will contain at most 2 log lines.
  //
  //    Using the above, we'll see log lines arrive with the at the following times:
  //    0.00: line0, line1, line2, line3 -> write 1 -> file 1
  //    0.75: line4, line5, line6, line7 -> write 1 -> file 1
  //    0.75: FLUSH
  //    1.50: line8  -> write 2 -> file 2
  //
  // Note: we use the IdentityEncoder to easily control which messages are dropped.
  // Note 2: we offset time by kTimeWaitForLimitedLog to wait for the buffer rate limiter.
  const zx::duration kArchivePeriod = zx::msec(750);
  const zx::duration kWriterPeriod = zx::sec(1);

  const std::vector<std::vector<std::string>> json_batches({
      {
          BuildLogMessage("line 0"),
          BuildLogMessage("line 1"),
          BuildLogMessage("line 2"),
          BuildLogMessage("line 3"),

      },
      {
          BuildLogMessage("line 4"),
          BuildLogMessage("line 5"),
          BuildLogMessage("line 6"),
          BuildLogMessage("line 7"),
      },
      {BuildLogMessage("line 8")},
      {},
  });

  stubs::DiagnosticsArchive archive(
      dispatcher(),
      std::make_unique<stubs::DiagnosticsBatchIteratorDelayedBatches>(
          dispatcher(), json_batches, kTimeWaitForLimitedLogs, kArchivePeriod, /*strict=*/true));

  InjectServiceProvider(&archive, kArchiveAccessorName);

  files::ScopedTempDir temp_dir;
  files::ScopedTempDir metadata_dir;
  const std::string metadata_path = files::JoinPath(metadata_dir.path(), "metadata.json");

  const std::string kFlushStr = "FLUSH\n";

  const StorageSize kWriteSize =
      kMaxLogLineSize * 2 + kDroppedFormatStrSize + StorageSize::Bytes(kFlushStr.size());

  SystemLogRecorder recorder(dispatcher(), dispatcher(), services(),
                             SystemLogRecorder::WriteParameters{
                                 .period = kWriterPeriod,
                                 .max_write_size = kWriteSize,
                                 .logs_dir = temp_dir.path(),
                                 .max_num_files = 2u,
                                 .total_log_size = 2u * kWriteSize,
                                 .metadata_path = metadata_path,
                                 .fallback_buffer_size = kFallbackLogBufferSize,
                             },
                             std::make_unique<IdentityRedactor>(inspect::BoolProperty()),
                             std::make_unique<IdentityEncoder>(),
                             std::make_unique<IdentityDecoder>());
  recorder.Start();

  RunLoopFor(kTimeWaitForLimitedLogs);
  RunLoopFor(kArchivePeriod);

  bool flushed = false;
  recorder.Flush(kFlushStr, [&flushed] { flushed = true; });
  EXPECT_FALSE(flushed);

  RunLoopUntilIdle();
  EXPECT_TRUE(flushed);

  std::string contents;

  files::ScopedTempDir output_dir;
  const std::string output_path = files::JoinPath(output_dir.path(), "output.txt");

  IdentityDecoder decoder;

  {
    float compression_ratio;
    const fit::result<ReaderError, std::string> result =
        Concatenate(temp_dir.path(), kMaxDecompressedSize, &decoder, &compression_ratio);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(compression_ratio, 1.0);
    EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 0
[15604.000][07559][07687][] INFO: line 1
!!! DROPPED 6 MESSAGES !!!
FLUSH
)");
  }

  RunLoopFor(kWriterPeriod);
  RunLoopFor(kWriterPeriod);

  {
    float compression_ratio;
    const fit::result<ReaderError, std::string> result =
        Concatenate(temp_dir.path(), kMaxDecompressedSize, &decoder, &compression_ratio);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(compression_ratio, 1.0);
    EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 0
[15604.000][07559][07687][] INFO: line 1
!!! DROPPED 6 MESSAGES !!!
FLUSH
[15604.000][07559][07687][] INFO: line 8
)");
  }
}

TEST_F(SystemLogRecorderSingleDispatcherTest, MultipleFlushes) {
  const zx::duration kArchivePeriod = zx::msec(750);
  const zx::duration kWriterPeriod = zx::sec(1);

  const std::vector<std::vector<std::string>> json_batches({
      {
          BuildLogMessage("line 0"),
          BuildLogMessage("line 1"),
      },
      {},
  });

  stubs::DiagnosticsArchive archive(
      dispatcher(),
      std::make_unique<stubs::DiagnosticsBatchIteratorDelayedBatches>(
          dispatcher(), json_batches, kTimeWaitForLimitedLogs, kArchivePeriod, /*strict=*/true));

  InjectServiceProvider(&archive, kArchiveAccessorName);

  files::ScopedTempDir temp_dir;
  files::ScopedTempDir metadata_dir;
  const std::string metadata_path = files::JoinPath(metadata_dir.path(), "metadata.json");

  const std::string kFlushStr1 = "FLUSH 1\n";
  const std::string kFlushStr2 = "FLUSH 2\n";

  const StorageSize kWriteSize = kMaxLogLineSize * 2 + kDroppedFormatStrSize +
                                 StorageSize::Bytes(kFlushStr1.size() + kFlushStr2.size());

  SystemLogRecorder recorder(dispatcher(), dispatcher(), services(),
                             SystemLogRecorder::WriteParameters{
                                 .period = kWriterPeriod,
                                 .max_write_size = kWriteSize,
                                 .logs_dir = temp_dir.path(),
                                 .max_num_files = 2u,
                                 .total_log_size = 2u * kWriteSize,
                                 .metadata_path = metadata_path,
                                 .fallback_buffer_size = kFallbackLogBufferSize,
                             },
                             std::make_unique<IdentityRedactor>(inspect::BoolProperty()),
                             std::make_unique<IdentityEncoder>(),
                             std::make_unique<IdentityDecoder>());
  recorder.Start();

  RunLoopFor(kTimeWaitForLimitedLogs);
  RunLoopFor(kArchivePeriod);

  std::vector<int> flush_order;
  recorder.Flush(kFlushStr1, [&flush_order] { flush_order.push_back(1); });
  recorder.Flush(kFlushStr2, [&flush_order] { flush_order.push_back(2); });
  EXPECT_TRUE(flush_order.empty());

  RunLoopUntilIdle();
  EXPECT_THAT(flush_order, ElementsAre(1, 2));

  IdentityDecoder decoder;

  float compression_ratio;
  const fit::result<ReaderError, std::string> result =
      Concatenate(temp_dir.path(), kMaxDecompressedSize, &decoder, &compression_ratio);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(compression_ratio, 1.0);
  EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 0
[15604.000][07559][07687][] INFO: line 1
FLUSH 1
FLUSH 2
)");
}

TEST_F(SystemLogRecorderMultiDispatcherTest, RecordsLogs) {
  std::unique_ptr<async::LoopInterface> write_loop = test_loop().StartNewLoop();

  const zx::duration kArchivePeriod = zx::msec(750);
  const zx::duration kWriterPeriod = zx::sec(1);

  const std::vector<std::vector<std::string>> json_batches({
      {
          BuildLogMessage("line 0"),
          BuildLogMessage("line 1"),
      },
      {
          BuildLogMessage("line 2"),
          BuildLogMessage("line 3"),
      },
      {},
  });

  stubs::DiagnosticsArchive archive(
      dispatcher(), std::make_unique<stubs::DiagnosticsBatchIteratorDelayedBatches>(
                        dispatcher(), json_batches, /*initial_delay=*/zx::nsec(0), kArchivePeriod,
                        /*strict=*/false));

  InjectServiceProvider(&archive, kArchiveAccessorName);

  files::ScopedTempDir temp_dir;
  files::ScopedTempDir metadata_dir;
  const std::string metadata_path = files::JoinPath(metadata_dir.path(), "metadata.json");

  const StorageSize kWriteSize = kMaxLogLineSize * 2;

  SystemLogRecorder recorder(dispatcher(), write_loop->dispatcher(), services(),
                             SystemLogRecorder::WriteParameters{
                                 .period = kWriterPeriod,
                                 .max_write_size = kWriteSize,
                                 .logs_dir = temp_dir.path(),
                                 .max_num_files = 2u,
                                 .total_log_size = 2u * kWriteSize,
                                 .metadata_path = metadata_path,
                                 .fallback_buffer_size = kFallbackLogBufferSize,
                             },
                             std::make_unique<IdentityRedactor>(inspect::BoolProperty()),
                             std::make_unique<IdentityEncoder>(),
                             std::make_unique<IdentityDecoder>());
  recorder.Start();

  RunLoopFor(kWriterPeriod * 2);

  IdentityDecoder decoder;

  float compression_ratio;
  const fit::result<ReaderError, std::string> result =
      Concatenate(temp_dir.path(), kMaxDecompressedSize, &decoder, &compression_ratio);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 0
[15604.000][07559][07687][] INFO: line 1
[15604.000][07559][07687][] INFO: line 2
[15604.000][07559][07687][] INFO: line 3
)");
}

TEST_F(SystemLogRecorderMultiDispatcherTest, GetCurrentBootLogs) {
  std::unique_ptr<async::LoopInterface> write_loop = test_loop().StartNewLoop();

  const zx::duration kArchivePeriod = zx::msec(750);
  const zx::duration kWriterPeriod = zx::sec(1);

  const std::vector<std::vector<std::string>> json_batches({
      {
          BuildLogMessage("line 0"),
          BuildLogMessage("line 1"),
      },
      {},
  });

  stubs::DiagnosticsArchive archive(
      dispatcher(),
      std::make_unique<stubs::DiagnosticsBatchIteratorNeverRespondsAfterOneBatch>(json_batches[0]));

  InjectServiceProvider(&archive, kArchiveAccessorName);

  files::ScopedTempDir temp_dir;
  const std::string logs_dir = files::JoinPath(temp_dir.path(), "logs");
  const std::string metadata_path = files::JoinPath(temp_dir.path(), "metadata.json");

  const StorageSize kWriteSize = kMaxLogLineSize * 2 + kDroppedFormatStrSize;

  SystemLogRecorder recorder(dispatcher(), write_loop->dispatcher(), services(),
                             SystemLogRecorder::WriteParameters{
                                 .period = kWriterPeriod,
                                 .max_write_size = kWriteSize,
                                 .logs_dir = logs_dir,
                                 .max_num_files = 2u,
                                 .total_log_size = 2u * kWriteSize,
                                 .metadata_path = metadata_path,
                                 .fallback_buffer_size = kFallbackLogBufferSize,
                             },
                             std::make_unique<IdentityRedactor>(inspect::BoolProperty()),
                             std::make_unique<ProductionEncoder>(),
                             std::make_unique<ProductionDecoder>());
  recorder.Start();

  RunLoopFor(kTimeWaitForLimitedLogs);
  RunLoopFor(kArchivePeriod);
  RunLoopFor(kWriterPeriod);

  auto endpoints = fidl::CreateEndpoints<ffi::SystemLogRecorder>();
  ASSERT_TRUE(endpoints.is_ok());

  fidl::BindServer(dispatcher(), std::move(endpoints->server), &recorder);
  fidl::Client client(std::move(endpoints->client), dispatcher());

  std::optional<fidl::Result<ffi::SystemLogRecorder::GetCurrentBootLogs>> result;
  client->GetCurrentBootLogs().Then([&result](auto& res) { result = std::move(res); });

  RunLoopUntilIdle();

  ASSERT_TRUE(result.has_value());
  ASSERT_TRUE(result->is_ok());
  ffi::SystemLogRecorderGetCurrentBootLogsResponse response = std::move(result->value());
  ASSERT_TRUE(response.logs().has_value());
  zx::vmo vmo = std::move(*response.logs());

  const zx::result<std::string> contents = StringFromVmo(vmo);
  ASSERT_TRUE(contents.is_ok());

  EXPECT_THAT(*contents, HasSubstr("line 0"));
  EXPECT_THAT(*contents, HasSubstr("line 1"));
  ASSERT_TRUE(response.metadata().has_value());
  ASSERT_TRUE(response.metadata()->first_timestamp().has_value());
  ASSERT_TRUE(response.metadata()->last_timestamp().has_value());
  EXPECT_EQ(*response.metadata()->first_timestamp(), zx::time_boot(zx::sec(15604).get()));
  EXPECT_EQ(*response.metadata()->last_timestamp(), zx::time_boot(zx::sec(15604).get()));
  ASSERT_TRUE(response.metadata()->source().has_value());
  EXPECT_EQ(*response.metadata()->source(), ffi::SystemLogSource::kDisk);
}

TEST_F(SystemLogRecorderMultiDispatcherTest, GetCurrentBootLogsEmptyLogs) {
  std::unique_ptr<async::LoopInterface> write_loop = test_loop().StartNewLoop();

  const zx::duration kWriterPeriod = zx::sec(1);

  files::ScopedTempDir temp_dir;
  const std::string logs_dir = files::JoinPath(temp_dir.path(), "logs");
  const std::string metadata_path = files::JoinPath(temp_dir.path(), "metadata.json");

  const StorageSize kWriteSize = kMaxLogLineSize * 2 + kDroppedFormatStrSize;

  SystemLogRecorder recorder(dispatcher(), write_loop->dispatcher(), services(),
                             SystemLogRecorder::WriteParameters{
                                 .period = kWriterPeriod,
                                 .max_write_size = kWriteSize,
                                 .logs_dir = logs_dir,
                                 .max_num_files = 2u,
                                 .total_log_size = 2u * kWriteSize,
                                 .metadata_path = metadata_path,
                                 .fallback_buffer_size = kFallbackLogBufferSize,
                             },
                             std::make_unique<IdentityRedactor>(inspect::BoolProperty()),
                             std::make_unique<ProductionEncoder>(),
                             std::make_unique<ProductionDecoder>());

  auto endpoints = fidl::CreateEndpoints<ffi::SystemLogRecorder>();
  ASSERT_TRUE(endpoints.is_ok());

  fidl::BindServer(dispatcher(), std::move(endpoints->server), &recorder);
  fidl::Client client(std::move(endpoints->client), dispatcher());

  std::optional<fidl::Result<ffi::SystemLogRecorder::GetCurrentBootLogs>> result;
  client->GetCurrentBootLogs().Then([&result](auto& res) { result = std::move(res); });

  RunLoopUntilIdle();

  ASSERT_TRUE(result.has_value());
  ASSERT_TRUE(result->is_error());
  ASSERT_TRUE(result->error_value().is_domain_error());
  EXPECT_EQ(result->error_value().domain_error(), ffi::RecorderError::kIoError);
}

TEST_F(SystemLogRecorderMultiDispatcherTest, GetCurrentBootLogsFallbackToArchivistOnPurgeSuccess) {
  std::unique_ptr<async::LoopInterface> write_loop = test_loop().StartNewLoop();

  const zx::duration kArchivePeriod = zx::msec(750);
  const zx::duration kWriterPeriod = zx::sec(1);

  const std::vector<std::vector<std::string>> disk_json_batches({
      {
          BuildLogMessage("disk line 0"),
          BuildLogMessage("disk line 1"),
      },
      {},
  });

  const std::vector<std::vector<std::string>> fallback_stream_json_batches({
      {
          BuildLogMessage("fallback stream line 0"),
          BuildLogMessage("fallback stream line 1"),
      },
      {},
  });

  std::vector<std::unique_ptr<stubs::DiagnosticsBatchIteratorBase>> iterators;
  iterators.push_back(std::make_unique<stubs::DiagnosticsBatchIteratorNeverRespondsAfterOneBatch>(
      disk_json_batches[0]));
  iterators.push_back(
      std::make_unique<stubs::DiagnosticsBatchIterator>(fallback_stream_json_batches));

  stubs::DiagnosticsArchiveSequentialIterators archive(dispatcher(), std::move(iterators));
  InjectServiceProvider(&archive, kArchiveAccessorName);

  files::ScopedTempDir temp_dir;
  const std::string logs_dir = files::JoinPath(temp_dir.path(), "logs");
  const std::string metadata_path = files::JoinPath(temp_dir.path(), "metadata.json");
  const StorageSize kWriteSize = kMaxLogLineSize * 2 + kDroppedFormatStrSize;
  SystemLogRecorder recorder(dispatcher(), write_loop->dispatcher(), services(),
                             SystemLogRecorder::WriteParameters{
                                 .period = kWriterPeriod,
                                 .max_write_size = kWriteSize,
                                 .logs_dir = logs_dir,
                                 .max_num_files = 2u,
                                 .total_log_size = 2u * kWriteSize,
                                 .metadata_path = metadata_path,
                                 .fallback_buffer_size = kFallbackLogBufferSize,
                             },
                             std::make_unique<IdentityRedactor>(inspect::BoolProperty()),
                             std::make_unique<ProductionEncoder>(),
                             std::make_unique<ProductionDecoder>());
  recorder.Start();

  RunLoopFor(kTimeWaitForLimitedLogs);
  RunLoopFor(kArchivePeriod);
  RunLoopFor(kWriterPeriod);

  // Simulate a cache purge.
  ASSERT_TRUE(files::DeletePath(temp_dir.path(), /*recursive=*/true));

  auto endpoints = fidl::CreateEndpoints<ffi::SystemLogRecorder>();
  ASSERT_TRUE(endpoints.is_ok());

  fidl::BindServer(dispatcher(), std::move(endpoints->server), &recorder);
  fidl::Client client(std::move(endpoints->client), dispatcher());

  std::optional<fidl::Result<ffi::SystemLogRecorder::GetCurrentBootLogs>> result;
  client->GetCurrentBootLogs().Then([&result](auto& res) { result = std::move(res); });

  RunLoopUntilIdle();
  ASSERT_TRUE(result.has_value());
  ASSERT_TRUE(result->is_ok());
  ffi::SystemLogRecorderGetCurrentBootLogsResponse response = std::move(result->value());

  ASSERT_TRUE(response.logs().has_value());
  zx::vmo vmo = std::move(*response.logs());

  const zx::result<std::string> contents = StringFromVmo(vmo);
  ASSERT_TRUE(contents.is_ok());

  EXPECT_THAT(*contents, HasSubstr("fallback stream line 0"));
  EXPECT_THAT(*contents, HasSubstr("fallback stream line 1"));
  ASSERT_TRUE(response.metadata().has_value());
  ASSERT_TRUE(response.metadata()->first_timestamp().has_value());
  ASSERT_TRUE(response.metadata()->last_timestamp().has_value());
  EXPECT_EQ(*response.metadata()->first_timestamp(), zx::time_boot(zx::sec(15604).get()));
  EXPECT_EQ(*response.metadata()->last_timestamp(), zx::time_boot(zx::sec(15604).get()));
  ASSERT_TRUE(response.metadata()->source().has_value());
  EXPECT_EQ(*response.metadata()->source(), ffi::SystemLogSource::kStream);
}

TEST_F(SystemLogRecorderMultiDispatcherTest, GetCurrentBootLogsFallbackToArchivistOnPurgeError) {
  std::unique_ptr<async::LoopInterface> write_loop = test_loop().StartNewLoop();

  const zx::duration kArchivePeriod = zx::msec(750);
  const zx::duration kWriterPeriod = zx::sec(1);

  const std::vector<std::vector<std::string>> disk_json_batches({
      {
          BuildLogMessage("disk line 0"),
      },
      {},
  });

  // |archive| provides an iterator for the initial stream to disk and an error-returning iterator
  // for the subsequent fallback request.
  std::vector<std::unique_ptr<stubs::DiagnosticsBatchIteratorBase>> iterators;
  iterators.push_back(std::make_unique<stubs::DiagnosticsBatchIteratorNeverRespondsAfterOneBatch>(
      disk_json_batches[0]));
  iterators.push_back(std::make_unique<stubs::DiagnosticsBatchIteratorReturnsError>());
  stubs::DiagnosticsArchiveSequentialIterators archive(dispatcher(), std::move(iterators));
  InjectServiceProvider(&archive, kArchiveAccessorName);

  files::ScopedTempDir temp_dir;
  const std::string logs_dir = files::JoinPath(temp_dir.path(), "logs");
  const std::string metadata_path = files::JoinPath(temp_dir.path(), "metadata.json");
  const StorageSize kWriteSize = kMaxLogLineSize * 2 + kDroppedFormatStrSize;

  SystemLogRecorder recorder(dispatcher(), write_loop->dispatcher(), services(),
                             SystemLogRecorder::WriteParameters{
                                 .period = kWriterPeriod,
                                 .max_write_size = kWriteSize,
                                 .logs_dir = logs_dir,
                                 .max_num_files = 2u,
                                 .total_log_size = 2u * kWriteSize,
                                 .metadata_path = metadata_path,
                                 .fallback_buffer_size = kFallbackLogBufferSize,
                             },
                             std::make_unique<IdentityRedactor>(inspect::BoolProperty()),
                             std::make_unique<ProductionEncoder>(),
                             std::make_unique<ProductionDecoder>());
  recorder.Start();

  RunLoopFor(kTimeWaitForLimitedLogs);
  RunLoopFor(kArchivePeriod);
  RunLoopFor(kWriterPeriod);

  // Simulate cache purge.
  ASSERT_TRUE(files::DeletePath(temp_dir.path(), /*recursive=*/true));

  auto endpoints = fidl::CreateEndpoints<ffi::SystemLogRecorder>();
  ASSERT_TRUE(endpoints.is_ok());

  fidl::BindServer(dispatcher(), std::move(endpoints->server), &recorder);
  fidl::Client client(std::move(endpoints->client), dispatcher());

  std::optional<fidl::Result<ffi::SystemLogRecorder::GetCurrentBootLogs>> result;
  client->GetCurrentBootLogs().Then([&result](auto& res) { result = std::move(res); });

  RunLoopUntilIdle();

  ASSERT_TRUE(result.has_value());
  ASSERT_TRUE(result->is_error());
  ASSERT_TRUE(result->error_value().is_domain_error());
  EXPECT_EQ(result->error_value().domain_error(), ffi::RecorderError::kStreamError);
}

TEST_F(SystemLogRecorderMultiDispatcherTest, ResumesPersistingAfterCachePurge) {
  std::unique_ptr<async::LoopInterface> write_loop = test_loop().StartNewLoop();

  const zx::duration kArchivePeriod = zx::msec(750);
  const zx::duration kWriterPeriod = zx::sec(1);

  const std::vector<std::vector<std::string>> json_batches({
      {BuildLogMessage("line 0")},
      {BuildLogMessage("line 1")},
      {BuildLogMessage("line 2")},
      {BuildLogMessage("line 3")},
      {BuildLogMessage("line 4")},
      {BuildLogMessage("line 5")},
      {},
  });

  stubs::DiagnosticsArchive archive(
      dispatcher(), std::make_unique<stubs::DiagnosticsBatchIteratorDelayedBatches>(
                        dispatcher(), json_batches, zx::sec(0), kArchivePeriod, /*strict=*/false));
  InjectServiceProvider(&archive, kArchiveAccessorName);

  files::ScopedTempDir temp_dir;
  const std::string logs_dir = files::JoinPath(temp_dir.path(), "logs");
  const std::string metadata_path = files::JoinPath(temp_dir.path(), "metadata.json");
  const StorageSize kWriteSize = kMaxLogLineSize * 2 + kDroppedFormatStrSize;
  SystemLogRecorder recorder(dispatcher(), write_loop->dispatcher(), services(),
                             SystemLogRecorder::WriteParameters{
                                 .period = kWriterPeriod,
                                 .max_write_size = kWriteSize,
                                 .logs_dir = logs_dir,
                                 .max_num_files = 2u,
                                 .total_log_size = 2u * kWriteSize,
                                 .metadata_path = metadata_path,
                                 .fallback_buffer_size = kFallbackLogBufferSize,
                             },
                             std::make_unique<IdentityRedactor>(inspect::BoolProperty()),
                             std::make_unique<ProductionEncoder>(),
                             std::make_unique<ProductionDecoder>());
  recorder.Start();

  RunLoopFor(kArchivePeriod);
  RunLoopFor(kWriterPeriod);

  // File 0 should exist with initial logs.
  ASSERT_TRUE(files::IsFile(files::JoinPath(logs_dir, "0")));

  // Simulate a cache purge.
  ASSERT_TRUE(files::DeletePath(temp_dir.path(), /*recursive=*/true));

  // Allow periodic write to detect purge, reset store, and open file 0.
  RunLoopFor(kWriterPeriod);

  // Allow new logs to arrive and be written to the newly opened file 0.
  RunLoopFor(kArchivePeriod);
  RunLoopFor(kWriterPeriod);

  // Persistence should have resumed into file 0.
  EXPECT_TRUE(files::IsDirectory(logs_dir));
  EXPECT_TRUE(files::IsFile(files::JoinPath(logs_dir, "0")));
  EXPECT_TRUE(files::IsFile(metadata_path));

  ProductionDecoder decoder;
  float compression_ratio;
  const fit::result<ReaderError, std::string> result =
      Concatenate(logs_dir, kMaxDecompressedSize, &decoder, &compression_ratio);
  ASSERT_TRUE(result.is_ok());
  EXPECT_THAT(*result, ::testing::Not(HasSubstr("line 0")));
  EXPECT_THAT(*result, ::testing::Not(HasSubstr("line 1")));
  EXPECT_THAT(*result, ::testing::Not(HasSubstr("line 2")));
  EXPECT_THAT(*result, HasSubstr("line 3"));
  EXPECT_THAT(*result, HasSubstr("line 4"));
  EXPECT_THAT(*result, HasSubstr("line 5"));
}

}  // namespace
}  // namespace system_log_recorder
}  // namespace feedback_data
}  // namespace forensics
