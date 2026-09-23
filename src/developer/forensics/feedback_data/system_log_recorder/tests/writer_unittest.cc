// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/writer.h"

#include <fuchsia/logger/cpp/fidl.h>
#include <lib/syslog/cpp/log_level.h>

#include <memory>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback_data/system_log_recorder/disk_backed_logs_metadata.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/identity_decoder.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/identity_encoder.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/production_encoding.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/encoding/version.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/log_message_store.h"
#include "src/developer/forensics/feedback_data/system_log_recorder/reader.h"
#include "src/developer/forensics/testing/log_message.h"
#include "src/developer/forensics/testing/scoped_memfs_manager.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/log_format.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/developer/forensics/utils/vmo.h"
#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"

namespace forensics {
namespace feedback_data {
namespace system_log_recorder {
namespace {

class WriterTest : public UnitTestFixture {
 public:
  WriterTest() : redactor_(inspect::BoolProperty{}) {}

  RedactorBase* GetIdentityRedactor() { return &redactor_; }

 private:
  IdentityRedactor redactor_;
};

::fpromise::result<fuchsia::logger::LogMessage, std::string> BuildLogMessage(
    const int32_t severity, const std::string& text,
    const zx::duration timestamp_offset = zx::duration(0),
    const std::vector<std::string>& tags = {}) {
  return ::fpromise::ok(testing::BuildLogMessage(severity, text, timestamp_offset, tags));
}

// Only change "X" for one character. i.e. X -> 12 is not allowed.
const StorageSize kMaxLogLineSize =
    StorageSize::Bytes(Format(BuildLogMessage(FUCHSIA_LOG_INFO, "line X").value()).size());

const StorageSize kMaxDecompressedSize = StorageSize::Kilobytes(256);

constexpr const char* kRootDirectory = "/root";
constexpr const char* kWriteDirectory = "/root/write";
constexpr const char* kReadDirectory = "/read";

class EncoderStub : public Encoder {
 public:
  EncoderStub() {}
  virtual ~EncoderStub() {}
  virtual EncodingVersion GetEncodingVersion() const { return EncodingVersion::kForTesting; }
  virtual std::string Encode(const std::string& msg) {
    input_.back() += msg;
    return msg;
  }
  virtual void Reset() { input_.push_back(""); }
  std::vector<std::string> GetInput() { return input_; }

 private:
  std::vector<std::string> input_ = {""};
};

class Decoder2x : public Decoder {
 public:
  Decoder2x() {}
  virtual ~Decoder2x() {}
  virtual EncodingVersion GetEncodingVersion() const { return EncodingVersion::kForTesting; }
  virtual std::string Decode(const std::string& msg) { return msg + msg; }
  virtual void Reset() {}
};

std::unique_ptr<Encoder> MakeIdentityEncoder() {
  return std::unique_ptr<Encoder>(new IdentityEncoder());
}

std::string MakeLogFilePath(const size_t file_num) {
  return files::JoinPath(kWriteDirectory, std::to_string(file_num));
}

TEST_F(WriterTest, VerifyFileOrdering) {
  // Set up the writer such that each file can fit 1 log message. When consuming a message the
  // end of block signal will be sent and a new empty file will be produced from file rotation.
  // From this behavior although we use 4 files, we only expect to retrieve the last 3 messages.
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const StorageSize kBlockSize = kMaxLogLineSize;
  const StorageSize kBufferSize = kMaxLogLineSize;

  LogMessageStore store(kBlockSize, kBufferSize, GetIdentityRedactor(), MakeIdentityEncoder());
  store.TurnOnRateLimiting();
  SystemLogWriter writer(kWriteDirectory, 4u, std::make_unique<IdentityDecoder>());

  // Written to file 0
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1")));
  writer.Write(store.Consume());

  // Written to file 1
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2")));
  writer.Write(store.Consume());

  // Written to file 2
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 3")));
  writer.Write(store.Consume());

  // Written to file 3
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 4")));
  writer.Write(store.Consume());

  // Written to file 4
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 5")));
  writer.Write(store.Consume());

  memfs_manager.Create(kReadDirectory);
  IdentityDecoder decoder;

  std::string content;
  ASSERT_TRUE(files::ReadFileToString(MakeLogFilePath(2u), &content));
  EXPECT_EQ(content, R"([15604.000][07559][07687][] INFO: line 3
)");

  ASSERT_TRUE(files::ReadFileToString(MakeLogFilePath(3u), &content));
  EXPECT_EQ(content, R"([15604.000][07559][07687][] INFO: line 4
)");

  ASSERT_TRUE(files::ReadFileToString(MakeLogFilePath(4u), &content));
  EXPECT_EQ(content, R"([15604.000][07559][07687][] INFO: line 5
)");

  float compression_ratio;
  const fit::result<ReaderError, std::string> result =
      Concatenate(kWriteDirectory, kMaxDecompressedSize, &decoder, &compression_ratio);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(compression_ratio, 1.0);

  EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 3
[15604.000][07559][07687][] INFO: line 4
[15604.000][07559][07687][] INFO: line 5
)");
}

TEST_F(WriterTest, VerifyEncoderInput) {
  // Set up the writer such that each file can fit 2 log messages. We will then write 4 messages
  // and expect that the encoder receives 2 reset signals and encodes 2 log messages in each block.
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const StorageSize kBlockSize = kMaxLogLineSize * 2;
  const StorageSize kBufferSize = kMaxLogLineSize * 2;

  auto encoder = std::unique_ptr<EncoderStub>(new EncoderStub());
  EncoderStub* encoder_ptr = encoder.get();
  LogMessageStore store(kBlockSize, kBufferSize, GetIdentityRedactor(), std::move(encoder));
  store.TurnOnRateLimiting();
  SystemLogWriter writer(kWriteDirectory, 2u, std::make_unique<IdentityDecoder>());

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1")));
  writer.Write(store.Consume());
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2")));
  writer.Write(store.Consume());
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 3")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 4")));
  writer.Write(store.Consume());

  std::vector<std::string> input = encoder_ptr->GetInput();
  EXPECT_EQ(input.size(), (size_t)3);

  EXPECT_EQ(input[0], R"([15604.000][07559][07687][] INFO: line 1
[15604.000][07559][07687][] INFO: line 2
)");

  EXPECT_EQ(input[1], R"([15604.000][07559][07687][] INFO: line 3
[15604.000][07559][07687][] INFO: line 4
)");
}

TEST_F(WriterTest, WritesMessages) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  // Set up the writer such that each file can fit 2 log messages and the "!!! DROPPED..."
  // string.
  LogMessageStore store(kMaxLogLineSize * 2, kMaxLogLineSize * 2, GetIdentityRedactor(),
                        MakeIdentityEncoder());
  store.TurnOnRateLimiting();
  SystemLogWriter writer(kWriteDirectory, 2u, std::make_unique<IdentityDecoder>());

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1")));
  EXPECT_FALSE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2")));
  writer.Write(store.Consume());

  memfs_manager.Create(kReadDirectory);
  IdentityDecoder decoder;

  float compression_ratio;
  const fit::result<ReaderError, std::string> result =
      Concatenate(kWriteDirectory, kMaxDecompressedSize, &decoder, &compression_ratio);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(compression_ratio, 1.0);

  EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 0
[15604.000][07559][07687][] INFO: line 1
!!! DROPPED 1 MESSAGES !!!
)");

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 3")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 4")));
  writer.Write(store.Consume());

  const fit::result<ReaderError, std::string> result2 =
      Concatenate(kWriteDirectory, kMaxDecompressedSize, &decoder, &compression_ratio);
  ASSERT_TRUE(result2.is_ok());
  EXPECT_EQ(compression_ratio, 1.0);

  EXPECT_EQ(*result2, R"([15604.000][07559][07687][] INFO: line 3
[15604.000][07559][07687][] INFO: line 4
)");
}

TEST_F(WriterTest, VerifyCompressionRatio) {
  // Generate 2x data when decoding. The decoder data output is not useful, just its size.
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  LogMessageStore store(kMaxLogLineSize * 4, kMaxLogLineSize * 4, GetIdentityRedactor(),
                        MakeIdentityEncoder());
  store.TurnOnRateLimiting();
  SystemLogWriter writer(kWriteDirectory, 2u, std::make_unique<IdentityDecoder>());

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2")));
  writer.Write(store.Consume());

  memfs_manager.Create(kReadDirectory);
  Decoder2x decoder;

  float compression_ratio;
  const fit::result<ReaderError, std::string> result =
      Concatenate(kWriteDirectory, kMaxDecompressedSize, &decoder, &compression_ratio);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(compression_ratio, 2.0);
}

TEST_F(WriterTest, VerifyProductionEcoding) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  // Set up the writer such that one file contains 5 log messages.
  auto encoder = std::unique_ptr<Encoder>(new ProductionEncoder());
  LogMessageStore store(kMaxLogLineSize * 5, kMaxLogLineSize * 5, GetIdentityRedactor(),
                        std::move(encoder));
  store.TurnOnRateLimiting();
  SystemLogWriter writer(kWriteDirectory, 2u, std::make_unique<IdentityDecoder>());

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 3")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 4")));
  writer.Write(store.Consume());

  memfs_manager.Create(kReadDirectory);
  ProductionDecoder decoder;

  float compression_ratio;
  const fit::result<ReaderError, std::string> result =
      Concatenate(kWriteDirectory, kMaxDecompressedSize, &decoder, &compression_ratio);
  ASSERT_TRUE(result.is_ok());
  EXPECT_FALSE(std::isnan(compression_ratio));

  EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 0
[15604.000][07559][07687][] INFO: line 1
[15604.000][07559][07687][] INFO: line 2
[15604.000][07559][07687][] INFO: line 3
[15604.000][07559][07687][] INFO: line 4
)");
}

TEST_F(WriterTest, FilesAlreadyPresent) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  {
    // Set up the writer such that one file contains at most 5 log messages.
    auto encoder = std::unique_ptr<Encoder>(new ProductionEncoder());
    LogMessageStore store(kMaxLogLineSize * 5, kMaxLogLineSize * 5, GetIdentityRedactor(),
                          std::move(encoder));
    store.TurnOnRateLimiting();

    SystemLogWriter writer(kWriteDirectory, 2u, std::make_unique<IdentityDecoder>());

    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0")));
    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1")));
    writer.Write(store.Consume());
  }
  {
    // Set up the writer such that one file contains at most 5 log messages.
    auto encoder = std::unique_ptr<Encoder>(new ProductionEncoder());
    LogMessageStore store(kMaxLogLineSize * 5, kMaxLogLineSize * 5, GetIdentityRedactor(),
                          std::move(encoder));
    store.TurnOnRateLimiting();

    SystemLogWriter writer(kWriteDirectory, 2u, std::make_unique<IdentityDecoder>());

    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2")));
    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 3")));
    writer.Write(store.Consume());
  }

  memfs_manager.Create(kReadDirectory);
  ProductionDecoder decoder;

  float compression_ratio;
  const fit::result<ReaderError, std::string> result =
      Concatenate(kWriteDirectory, kMaxDecompressedSize, &decoder, &compression_ratio);
  ASSERT_TRUE(result.is_ok());
  EXPECT_FALSE(std::isnan(compression_ratio));

  EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 0
[15604.000][07559][07687][] INFO: line 1
[15604.000][07559][07687][] INFO: line 2
[15604.000][07559][07687][] INFO: line 3
)");
}

TEST_F(WriterTest, FailCreateDirectory) {
  // Don't set up kRootDirectory
  testing::ScopedMemFsManager memfs_manager;

  // Set up the writer such that each file can fit 2 log messages and the "!!! DROPPED..."
  // string.
  LogMessageStore store(kMaxLogLineSize * 2, kMaxLogLineSize * 2, GetIdentityRedactor(),
                        MakeIdentityEncoder());
  store.TurnOnRateLimiting();
  SystemLogWriter writer(kWriteDirectory, 2u, std::make_unique<IdentityDecoder>());

  // Create the kRootDirectory so kWriteDirectory can be made by |writer| after the next set of
  // writes.
  memfs_manager.Create(kRootDirectory);

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1")));
  EXPECT_FALSE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2")));
  writer.Write(store.Consume());

  memfs_manager.Create(kReadDirectory);
  IdentityDecoder decoder;

  float compression_ratio;
  EXPECT_TRUE(
      Concatenate(kWriteDirectory, kMaxDecompressedSize, &decoder, &compression_ratio).is_error());

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 3")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 4")));
  writer.Write(store.Consume());

  const fit::result<ReaderError, std::string> result =
      Concatenate(kWriteDirectory, kMaxDecompressedSize, &decoder, &compression_ratio);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(compression_ratio, 1.0);

  EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 3
[15604.000][07559][07687][] INFO: line 4
)");
}

TEST_F(WriterTest, DirectoryDisappears) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  // Set up the writer such that each file can fit 2 log messages and the "!!! DROPPED..."
  // string.
  LogMessageStore store(kMaxLogLineSize * 2, kMaxLogLineSize * 2, GetIdentityRedactor(),
                        MakeIdentityEncoder());
  store.TurnOnRateLimiting();
  SystemLogWriter writer(kWriteDirectory, 2u, std::make_unique<IdentityDecoder>());

  // Destroy kWriteDirectory so the next set of writes fail and the directory is recreated.
  ASSERT_TRUE(files::DeletePath(kWriteDirectory, /*recursive=*/true));

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1")));
  EXPECT_FALSE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2")));
  writer.Write(store.Consume());

  memfs_manager.Create(kReadDirectory);
  IdentityDecoder decoder;

  float compression_ratio;
  EXPECT_TRUE(
      Concatenate(kWriteDirectory, kMaxDecompressedSize, &decoder, &compression_ratio).is_error());

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 3")));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 4")));
  writer.Write(store.Consume());

  const fit::result<ReaderError, std::string> result =
      Concatenate(kWriteDirectory, kMaxDecompressedSize, &decoder, &compression_ratio);
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(compression_ratio, 1.0);

  EXPECT_EQ(*result, R"([15604.000][07559][07687][] INFO: line 3
[15604.000][07559][07687][] INFO: line 4
)");
}

TEST_F(WriterTest, IgnoreNonNumericFiles) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);
  ASSERT_TRUE(files::CreateDirectory(kWriteDirectory));

  ASSERT_TRUE(files::WriteFile(files::JoinPath(kWriteDirectory, "0"), "data"));
  ASSERT_TRUE(files::WriteFile(files::JoinPath(kWriteDirectory, "invalid_file.log"), "data"));
  ASSERT_TRUE(files::WriteFile(files::JoinPath(kWriteDirectory, "1"), "data"));

  LogMessageStore store(kMaxLogLineSize * 5, kMaxLogLineSize * 5, GetIdentityRedactor(),
                        MakeIdentityEncoder());
  SystemLogWriter writer(kWriteDirectory, 5u, std::make_unique<IdentityDecoder>());

  // Additional writes should continue from file 2, ignoring invalid files.
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0")));
  writer.Write(store.Consume());

  EXPECT_TRUE(files::IsFile(files::JoinPath(kWriteDirectory, "2")));
}

TEST_F(WriterTest, SavesMetadata) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const std::string metadata_path = files::JoinPath(kRootDirectory, "metadata.json");

  LogMessageStore store(kMaxLogLineSize, kMaxLogLineSize, GetIdentityRedactor(),
                        MakeIdentityEncoder());
  SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/2u, std::make_unique<IdentityDecoder>(),
                         metadata_path);

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0", zx::msec(100))));
  writer.Write(store.Consume());

  std::optional<DiskBackedLogsMetadata> metadata =
      DiskBackedLogsMetadata::FromFile(metadata_path, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(metadata.has_value());
  EXPECT_FALSE(metadata->IsAtCapacity());
  EXPECT_EQ(metadata->MessageCount(), 1u);
  EXPECT_EQ(metadata->DeduplicatedMessageCount(), 1u);
  ASSERT_TRUE(metadata->FirstTimestamp().has_value());
  EXPECT_EQ(metadata->FirstTimestamp()->get(), (zx::sec(15604) + zx::msec(100)).get());
  ASSERT_TRUE(metadata->LastTimestamp().has_value());
  EXPECT_EQ(metadata->LastTimestamp()->get(), (zx::sec(15604) + zx::msec(100)).get());
}

TEST_F(WriterTest, SavesMetadataMultipleFiles) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const std::string metadata_path = files::JoinPath(kRootDirectory, "metadata.json");

  LogMessageStore store(kMaxLogLineSize, kMaxLogLineSize, GetIdentityRedactor(),
                        MakeIdentityEncoder());
  SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/2u, std::make_unique<IdentityDecoder>(),
                         metadata_path);

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0", zx::msec(100))));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1", zx::msec(200))));
  writer.Write(store.Consume());

  std::optional<DiskBackedLogsMetadata> metadata =
      DiskBackedLogsMetadata::FromFile(metadata_path, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(metadata.has_value());
  EXPECT_FALSE(metadata->IsAtCapacity());
  EXPECT_EQ(metadata->MessageCount(), 2u);
  EXPECT_EQ(metadata->DeduplicatedMessageCount(), 2u);
  ASSERT_TRUE(metadata->FirstTimestamp().has_value());
  EXPECT_EQ(metadata->FirstTimestamp()->get(), (zx::sec(15604) + zx::msec(100)).get());
  ASSERT_TRUE(metadata->LastTimestamp().has_value());
  EXPECT_EQ(metadata->LastTimestamp()->get(), (zx::sec(15604) + zx::msec(200)).get());
}

TEST_F(WriterTest, SavesMetadataMultipleWritesInSingleBlock) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const std::string metadata_path = files::JoinPath(kRootDirectory, "metadata.json");

  // Block size can hold 10 log messages; buffer size holds 1 log message.
  LogMessageStore store(kMaxLogLineSize * 10, kMaxLogLineSize, GetIdentityRedactor(),
                        MakeIdentityEncoder());
  SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/2u, std::make_unique<IdentityDecoder>(),
                         metadata_path);

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0", zx::msec(100))));
  writer.Write(store.Consume());

  std::optional<DiskBackedLogsMetadata> metadata =
      DiskBackedLogsMetadata::FromFile(metadata_path, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(metadata.has_value());
  EXPECT_EQ(metadata->MessageCount(), 1u);
  EXPECT_EQ(metadata->DeduplicatedMessageCount(), 1u);
  ASSERT_TRUE(metadata->FirstTimestamp().has_value());
  EXPECT_EQ(metadata->FirstTimestamp()->get(), (zx::sec(15604) + zx::msec(100)).get());
  ASSERT_TRUE(metadata->LastTimestamp().has_value());
  EXPECT_EQ(metadata->LastTimestamp()->get(), (zx::sec(15604) + zx::msec(100)).get());

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1", zx::msec(200))));
  writer.Write(store.Consume());

  metadata = DiskBackedLogsMetadata::FromFile(metadata_path, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(metadata.has_value());
  EXPECT_EQ(metadata->MessageCount(), 2u);
  EXPECT_EQ(metadata->DeduplicatedMessageCount(), 2u);
  ASSERT_TRUE(metadata->FirstTimestamp().has_value());
  EXPECT_EQ(metadata->FirstTimestamp()->get(), (zx::sec(15604) + zx::msec(100)).get());
  ASSERT_TRUE(metadata->LastTimestamp().has_value());
  EXPECT_EQ(metadata->LastTimestamp()->get(), (zx::sec(15604) + zx::msec(200)).get());

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2", zx::msec(300))));
  writer.Write(store.Consume());

  metadata = DiskBackedLogsMetadata::FromFile(metadata_path, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(metadata.has_value());
  EXPECT_EQ(metadata->MessageCount(), 3u);
  EXPECT_EQ(metadata->DeduplicatedMessageCount(), 3u);
  ASSERT_TRUE(metadata->FirstTimestamp().has_value());
  EXPECT_EQ(metadata->FirstTimestamp()->get(), (zx::sec(15604) + zx::msec(100)).get());
  ASSERT_TRUE(metadata->LastTimestamp().has_value());
  EXPECT_EQ(metadata->LastTimestamp()->get(), (zx::sec(15604) + zx::msec(300)).get());
}

TEST_F(WriterTest, RestoresMetadataOnRestart) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const std::string metadata_path = files::JoinPath(kRootDirectory, "metadata.json");

  {
    LogMessageStore store(kMaxLogLineSize, kMaxLogLineSize, GetIdentityRedactor(),
                          MakeIdentityEncoder());
    SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/4u,
                           std::make_unique<IdentityDecoder>(), metadata_path);

    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0", zx::msec(100))));
    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1", zx::msec(200))));
    writer.Write(store.Consume());
  }

  std::optional<DiskBackedLogsMetadata> metadata =
      DiskBackedLogsMetadata::FromFile(metadata_path, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(metadata.has_value());
  EXPECT_EQ(metadata->MessageCount(), 2u);
  EXPECT_EQ(metadata->DeduplicatedMessageCount(), 2u);
  ASSERT_TRUE(metadata->FirstTimestamp().has_value());
  EXPECT_EQ(metadata->FirstTimestamp()->get(), (zx::sec(15604) + zx::msec(100)).get());
  ASSERT_TRUE(metadata->LastTimestamp().has_value());
  EXPECT_EQ(metadata->LastTimestamp()->get(), (zx::sec(15604) + zx::msec(200)).get());

  {
    LogMessageStore store(kMaxLogLineSize, kMaxLogLineSize, GetIdentityRedactor(),
                          MakeIdentityEncoder());
    SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/4u,
                           std::make_unique<IdentityDecoder>(), metadata_path);

    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2", zx::msec(300))));
    writer.Write(store.Consume());
  }

  metadata = DiskBackedLogsMetadata::FromFile(metadata_path, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(metadata.has_value());
  EXPECT_EQ(metadata->MessageCount(), 3u);
  EXPECT_EQ(metadata->DeduplicatedMessageCount(), 3u);
  ASSERT_TRUE(metadata->FirstTimestamp().has_value());
  EXPECT_EQ(metadata->FirstTimestamp()->get(), (zx::sec(15604) + zx::msec(100)).get());
  ASSERT_TRUE(metadata->LastTimestamp().has_value());
  EXPECT_EQ(metadata->LastTimestamp()->get(), (zx::sec(15604) + zx::msec(300)).get());
}

TEST_F(WriterTest, RollsOutRestoredMetadataOnRotation) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const std::string metadata_path = files::JoinPath(kRootDirectory, "metadata.json");

  {
    LogMessageStore store(kMaxLogLineSize, kMaxLogLineSize, GetIdentityRedactor(),
                          MakeIdentityEncoder());
    SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/4u,
                           std::make_unique<IdentityDecoder>(), metadata_path);

    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0", zx::msec(100))));
    writer.Write(store.Consume());
    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1", zx::msec(200))));
    writer.Write(store.Consume());
    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2", zx::msec(300))));
    writer.Write(store.Consume());
  }

  std::optional<DiskBackedLogsMetadata> metadata =
      DiskBackedLogsMetadata::FromFile(metadata_path, SystemLogWriter::kFirstFileNumber);
  ASSERT_TRUE(metadata.has_value());
  EXPECT_EQ(metadata->MessageCount(), 3u);
  EXPECT_EQ(metadata->DeduplicatedMessageCount(), 3u);
  ASSERT_TRUE(metadata->FirstTimestamp().has_value());
  EXPECT_EQ(metadata->FirstTimestamp()->get(), (zx::sec(15604) + zx::msec(100)).get());
  ASSERT_TRUE(metadata->LastTimestamp().has_value());
  EXPECT_EQ(metadata->LastTimestamp()->get(), (zx::sec(15604) + zx::msec(300)).get());

  {
    // 4 files (0, 1, 2, 3) are present on disk, with file 3 being empty. The constructor calls
    // StartNewFile, causing file 0 to be deleted. Writing a new message (line 3) finishes block 4
    // and triggers StartNewFile, causing file 1 to be deleted. Metadata saved after StartNewFile
    // reflects remaining files on disk (files 2, 3, 4) = only 2 msgs because file 3 is empty.
    LogMessageStore store(kMaxLogLineSize, kMaxLogLineSize, GetIdentityRedactor(),
                          MakeIdentityEncoder());
    SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/4u,
                           std::make_unique<IdentityDecoder>(), metadata_path);

    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 3", zx::msec(400))));
    writer.Write(store.Consume());

    metadata = DiskBackedLogsMetadata::FromFile(metadata_path, SystemLogWriter::kFirstFileNumber);
    ASSERT_TRUE(metadata.has_value());
    EXPECT_EQ(metadata->MessageCount(), 2u);
    EXPECT_EQ(metadata->DeduplicatedMessageCount(), 2u);
    ASSERT_TRUE(metadata->FirstTimestamp().has_value());
    EXPECT_EQ(metadata->FirstTimestamp()->get(), (zx::sec(15604) + zx::msec(300)).get());
    ASSERT_TRUE(metadata->LastTimestamp().has_value());
    EXPECT_EQ(metadata->LastTimestamp()->get(), (zx::sec(15604) + zx::msec(400)).get());
  }
}

TEST_F(WriterTest, FlushAndReadLogs) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const std::string metadata_path = files::JoinPath(kRootDirectory, "metadata.json");

  const StorageSize kBlockSize = kMaxLogLineSize * 5;
  const StorageSize kBufferSize = kMaxLogLineSize * 5;

  LogMessageStore store(kBlockSize, kBufferSize, GetIdentityRedactor(),
                        std::make_unique<IdentityEncoder>());
  store.TurnOnRateLimiting();
  SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/2u, std::make_unique<IdentityDecoder>(),
                         metadata_path);

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0", zx::msec(100))));
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1", zx::msec(200))));

  SystemLogWriter::FlushAndReadLogsResult result = writer.FlushAndReadLogs(store.Consume());

  ASSERT_TRUE(result.logs.is_ok());
  EXPECT_FALSE(result.cache_purged);
  SystemLogWriter::Logs logs = std::move(result.logs.value());
  ASSERT_TRUE(logs.vmo.is_valid());

  const zx::result<std::string> contents = StringFromVmo(logs.vmo);
  ASSERT_TRUE(contents.is_ok());

  EXPECT_EQ(*contents, R"([15604.100][07559][07687][] INFO: line 0
[15604.200][07559][07687][] INFO: line 1
)");
  ASSERT_TRUE(logs.first_timestamp.has_value());
  ASSERT_TRUE(logs.last_timestamp.has_value());
  EXPECT_EQ(*logs.first_timestamp, zx::time_boot((zx::sec(15604) + zx::msec(100)).get()));
  EXPECT_EQ(*logs.last_timestamp, zx::time_boot((zx::sec(15604) + zx::msec(200)).get()));
}

TEST_F(WriterTest, FlushAndReadLogsEmptyLogs) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const std::string metadata_path = files::JoinPath(kRootDirectory, "metadata.json");

  const StorageSize kBlockSize = kMaxLogLineSize * 5;
  const StorageSize kBufferSize = kMaxLogLineSize * 5;

  LogMessageStore store(kBlockSize, kBufferSize, GetIdentityRedactor(),
                        std::make_unique<IdentityEncoder>());
  SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/2u, std::make_unique<IdentityDecoder>(),
                         metadata_path);

  const SystemLogWriter::FlushAndReadLogsResult result = writer.FlushAndReadLogs(store.Consume());

  ASSERT_TRUE(result.logs.is_error());
  EXPECT_FALSE(result.cache_purged);
  EXPECT_EQ(result.logs.error_value(), SystemLogWriter::WriterError::kIoError);
}

TEST_F(WriterTest, FlushAndReadLogsDetectsCachePurgeOnLogsDirectoryDeletion) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const std::string metadata_path = files::JoinPath(kRootDirectory, "metadata.json");

  const StorageSize kBlockSize = kMaxLogLineSize * 5;
  const StorageSize kBufferSize = kMaxLogLineSize * 5;

  LogMessageStore store(kBlockSize, kBufferSize, GetIdentityRedactor(),
                        std::make_unique<IdentityEncoder>());
  SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/2u, std::make_unique<IdentityDecoder>(),
                         metadata_path);

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0", zx::msec(100))));
  writer.Write(store.Consume());

  ASSERT_TRUE(files::DeletePath(kWriteDirectory, /*recursive=*/true));

  SystemLogWriter::FlushAndReadLogsResult result = writer.FlushAndReadLogs(store.Consume());

  ASSERT_TRUE(result.logs.is_error());
  EXPECT_TRUE(result.cache_purged);
  EXPECT_EQ(result.logs.error_value(), SystemLogWriter::WriterError::kInsufficientCoverage);
}

TEST_F(WriterTest, FlushAndReadLogsSufficientCoverageAfterMinFilesWritten) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const std::string metadata_path = files::JoinPath(kRootDirectory, "metadata.json");

  const StorageSize kBlockSize = kMaxLogLineSize;
  const StorageSize kBufferSize = kMaxLogLineSize;

  LogMessageStore store(kBlockSize, kBufferSize, GetIdentityRedactor(),
                        std::make_unique<IdentityEncoder>());
  SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/8u, std::make_unique<IdentityDecoder>(),
                         metadata_path);

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0", zx::msec(100))));
  writer.Write(store.Consume());

  EXPECT_TRUE(files::DeletePath(kWriteDirectory, /*recursive=*/true));
  EXPECT_TRUE(files::DeletePath(metadata_path, /*recursive=*/false));

  // Write blocks 1 to 4 post-purge (end_of_block = true).
  for (size_t i = 1; i <= 4; ++i) {
    EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line " + std::to_string(i))));
    LogMessageStore::ConsumeResult consume_result = store.Consume();
    EXPECT_TRUE(consume_result.end_of_block);
    writer.Write(consume_result);

    SystemLogWriter::FlushAndReadLogsResult result = writer.FlushAndReadLogs(store.Consume());
    ASSERT_TRUE(result.logs.is_error());
    EXPECT_FALSE(result.cache_purged);
    EXPECT_EQ(result.logs.error_value(), SystemLogWriter::WriterError::kInsufficientCoverage);
  }

  // Write block 5 post-purge (end_of_block = true) -> reaches kMinFilesForSufficientCoverage (5).
  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 5")));
  LogMessageStore::ConsumeResult consume_result = store.Consume();
  EXPECT_TRUE(consume_result.end_of_block);
  writer.Write(consume_result);

  SystemLogWriter::FlushAndReadLogsResult result = writer.FlushAndReadLogs(store.Consume());
  ASSERT_TRUE(result.logs.is_ok());
  EXPECT_FALSE(result.cache_purged);
}

TEST_F(WriterTest, WriteReturnsCachePurgedWhenLogsDirectoryDeleted) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const std::string metadata_path = files::JoinPath(kRootDirectory, "metadata.json");

  LogMessageStore store(kMaxLogLineSize * 10, kMaxLogLineSize * 10, GetIdentityRedactor(),
                        std::make_unique<IdentityEncoder>());
  SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/2u, std::make_unique<IdentityDecoder>(),
                         metadata_path);

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0")));
  EXPECT_EQ(writer.Write(store.Consume()), SystemLogWriter::WriteResult::kOk);

  ASSERT_TRUE(files::DeletePath(kWriteDirectory, /*recursive=*/true));

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1")));
  EXPECT_EQ(writer.Write(store.Consume()), SystemLogWriter::WriteResult::kCachePurge);
}

TEST_F(WriterTest, WriteReopensFileAfterPurge) {
  testing::ScopedMemFsManager memfs_manager;
  memfs_manager.Create(kRootDirectory);

  const std::string metadata_path = files::JoinPath(kRootDirectory, "metadata.json");

  LogMessageStore store(kMaxLogLineSize * 10, kMaxLogLineSize * 10, GetIdentityRedactor(),
                        std::make_unique<IdentityEncoder>());
  SystemLogWriter writer(kWriteDirectory, /*max_num_files=*/2u, std::make_unique<IdentityDecoder>(),
                         metadata_path);

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 0")));
  EXPECT_EQ(writer.Write(store.Consume()), SystemLogWriter::WriteResult::kOk);

  ASSERT_TRUE(files::DeletePath(kWriteDirectory, /*recursive=*/true));

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 1")));
  EXPECT_EQ(writer.Write(store.Consume()), SystemLogWriter::WriteResult::kCachePurge);

  EXPECT_TRUE(files::IsDirectory(kWriteDirectory));
  EXPECT_TRUE(files::IsFile(MakeLogFilePath(0)));

  EXPECT_TRUE(store.Add(BuildLogMessage(FUCHSIA_LOG_INFO, "line 2")));
  EXPECT_EQ(writer.Write(store.Consume()), SystemLogWriter::WriteResult::kOk);
}

}  // namespace
}  // namespace system_log_recorder
}  // namespace feedback_data
}  // namespace forensics
