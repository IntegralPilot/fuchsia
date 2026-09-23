// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tracing.h"

#include <lib/fxt/fields.h>
#include <lib/fxt/serializer.h>
#include <lib/zircon-internal/ktrace.h>
#include <lib/zx/result.h>

#include <fstream>
#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <re2/re2.h>

using ::testing::Invoke;
using ::testing::Return;

namespace {
class MockTracing : public Tracing {
 public:
  MOCK_METHOD(void, ReadKernelBuffer,
              (zx_handle_t handle, void* data_buf, uint32_t offset, size_t len, size_t* bytes_read),
              (override));
};

struct FxtReservation {
 public:
  FxtReservation(uint64_t* bytes, size_t buffer_size, uint64_t header)
      : bytes_(bytes), buffer_size_(buffer_size), header_(header) {
    ZX_ASSERT(fxt::RecordFields::RecordSize::Get<uint64_t>(header) * 8 <= buffer_size);
  }

  void WriteWord(uint64_t word) {
    ASSERT_LE((words_written_ + 1) * 8, buffer_size_);
    bytes_[words_written_++] = word;
  }

  void WriteBytes(const void* buffer, size_t num_bytes) {
    if (num_bytes == 0) {
      return;
    }
    size_t num_words = (num_bytes + 7) / 8;
    ASSERT_LE((words_written_ + num_words) * 8, buffer_size_);
    // Zero out any padding bytes
    bytes_[words_written_ + num_words - 1] = 0;
    memcpy(bytes_ + words_written_, buffer, num_bytes);
    words_written_ += num_words;
  }

  void Commit() { bytes_[0] = header_; }

 private:
  uint64_t* const bytes_;
  const size_t buffer_size_;
  const uint64_t header_;
  size_t words_written_ = 1;
};

struct FxtWriter {
  using Reservation = FxtReservation;
  explicit FxtWriter(uint64_t* buffer, size_t buffer_size)
      : buffer(buffer), buffer_size_(buffer_size) {}

  zx::result<FxtReservation> Reserve(uint64_t header) {
    FxtReservation rec{buffer + offset, buffer_size_, header};
    offset += fxt::RecordFields::RecordSize::Get<size_t>(header);
    return zx::ok(rec);
  }

  size_t offset = 0;
  uint64_t* buffer;
  size_t buffer_size_ = 0;
};

class BytesOfData {
 public:
  explicit BytesOfData(size_t num_bytes) : num_bytes_(num_bytes) {}

  void operator()(zx_handle_t, void*, uint32_t, size_t, size_t* bytes_read) {
    *bytes_read = num_bytes_;
  }

 private:
  size_t num_bytes_;
};

class CreateMockInstant {
 public:
  CreateMockInstant(std::string name, std::string category, zx_koid_t pid, zx_koid_t tid,
                    uint64_t ts)
      : name_(name), category_(category), pid_(pid), tid_(tid), ts_(ts) {}

  void operator()(zx_handle_t, void* data_buf, uint32_t offset, size_t len, size_t* bytes_read) {
    FxtWriter writer(reinterpret_cast<uint64_t*>(data_buf), len);
    fxt::WriteInstantEventRecord(&writer, ts_, fxt::ThreadRef(pid_, tid_),
                                 fxt::StringRef(category_.c_str()), fxt::StringRef(name_.c_str()));
    uint64_t header = reinterpret_cast<uint64_t*>(data_buf)[0];
    *bytes_read = 8 * fxt::RecordFields::RecordSize::Get<size_t>(header);
  }

 private:
  std::string name_;
  std::string category_;
  zx_koid_t pid_;
  zx_koid_t tid_;
  uint64_t ts_;
};

class CreateMockDuration {
 public:
  CreateMockDuration(std::string name, std::string category, zx_koid_t pid, zx_koid_t tid,
                     uint64_t start, uint64_t end)
      : name_(name), category_(category), pid_(pid), tid_(tid), start_(start), end_(end) {}

  CreateMockDuration(std::string name, std::string category, zx_koid_t pid, zx_koid_t tid,
                     uint64_t start, uint64_t end, uint32_t arg1, uint32_t arg2)
      : name_(name),
        category_(category),
        pid_(pid),
        tid_(tid),
        start_(start),
        end_(end),
        has_args(true),
        arg1_(arg1),
        arg2_(arg2) {}

  void operator()(zx_handle_t, void* data_buf, uint32_t offset, size_t len, size_t* bytes_read) {
    FxtWriter writer(reinterpret_cast<uint64_t*>(data_buf), len);
    if (!has_args) {
      fxt::WriteDurationCompleteEventRecord(&writer, start_, fxt::ThreadRef(pid_, tid_),
                                            fxt::StringRef(category_.c_str()),
                                            fxt::StringRef(name_.c_str()), end_);
    } else {
      fxt::WriteDurationCompleteEventRecord(
          &writer, start_, fxt::ThreadRef(pid_, tid_), fxt::StringRef(category_.c_str()),
          fxt::StringRef(name_.c_str()), end_, fxt::Argument{fxt::StringRef("arg1"), arg1_},
          fxt::Argument{fxt::StringRef("arg2"), arg2_});
    }
    uint64_t header = reinterpret_cast<uint64_t*>(data_buf)[0];
    *bytes_read = 8 * fxt::RecordFields::RecordSize::Get<size_t>(header);
  }

 private:
  std::string name_;
  std::string category_;
  zx_koid_t pid_;
  zx_koid_t tid_;
  uint64_t start_;
  uint64_t end_;

  bool has_args = false;
  uint32_t arg1_;
  uint32_t arg2_;
};

class CreateMockFlowBegin {
 public:
  CreateMockFlowBegin(std::string name, std::string category, uint64_t ts, zx_koid_t pid,
                      zx_koid_t tid, uint64_t flowid)
      : name_(name), category_(category), ts_(ts), pid_(pid), tid_(tid), flowid_(flowid) {}

  void operator()(zx_handle_t, void* data_buf, uint32_t offset, size_t len, size_t* bytes_read) {
    FxtWriter writer(reinterpret_cast<uint64_t*>(data_buf), len);
    fxt::WriteFlowBeginEventRecord(&writer, ts_, fxt::ThreadRef(pid_, tid_),
                                   fxt::StringRef(category_.c_str()), fxt::StringRef(name_.c_str()),
                                   flowid_);
    uint64_t header = reinterpret_cast<uint64_t*>(data_buf)[0];
    *bytes_read = 8 * fxt::RecordFields::RecordSize::Get<size_t>(header);
  }

 private:
  std::string name_;
  std::string category_;
  uint64_t ts_;
  zx_koid_t pid_;
  zx_koid_t tid_;
  uint64_t flowid_;
};

class CreateMockFlowEnd {
 public:
  CreateMockFlowEnd(std::string name, std::string category, uint64_t ts, zx_koid_t pid,
                    zx_koid_t tid, uint64_t flowid)
      : name_(name), category_(category), ts_(ts), pid_(pid), tid_(tid), flowid_(flowid) {}

  void operator()(zx_handle_t, void* data_buf, uint32_t offset, size_t len, size_t* bytes_read) {
    FxtWriter writer(reinterpret_cast<uint64_t*>(data_buf), len);
    fxt::WriteFlowEndEventRecord(&writer, ts_, fxt::ThreadRef(pid_, tid_),
                                 fxt::StringRef(category_.c_str()), fxt::StringRef(name_.c_str()),
                                 flowid_);
    uint64_t header = reinterpret_cast<uint64_t*>(data_buf)[0];
    *bytes_read = 8 * fxt::RecordFields::RecordSize::Get<size_t>(header);
  }

 private:
  std::string name_;
  std::string category_;
  uint64_t ts_;
  zx_koid_t pid_;
  zx_koid_t tid_;
  uint64_t flowid_;
};

class NoMoreData {
 public:
  NoMoreData() = default;

  void operator()(zx_handle_t, void*, uint32_t, size_t, size_t* bytes_read) { *bytes_read = 0; }
};

std::ofstream OpenFile(std::string tracing_filepath) {
  std::ofstream file;
  file.open(tracing_filepath);
  return file;
}

TEST(TracingTest, StartSetsRunningToTrue) {
  Tracing tracing;

  tracing.Start(KTRACE_GRP_ALL);
  ASSERT_TRUE(tracing.running());
}

TEST(TracingTest, StopSetsRunningToFalse) {
  Tracing tracing;

  tracing.Stop();
  ASSERT_FALSE(tracing.running());
}

TEST(TracingTest, DestructorStopsTracing) {
  Tracing tracing;

  tracing.Start(KTRACE_GRP_ALL);
  EXPECT_TRUE(tracing.running());

  tracing.~Tracing();

  ASSERT_FALSE(tracing.running());
}

TEST(TracingTest, BasicWriteSucceeds) {
  Tracing tracing;

  // Reset the contents of the trace buffer. This will ensure that the:
  // 1. Metadata is present at the beginning of the trace.
  // 2. Arbitrary reads/writes from other tests that ran prior to this one do not change the
  //    behavior of this test.
  tracing.Rewind();
  tracing.Start(KTRACE_GRP_ALL);
  tracing.Stop();

  std::ofstream human_readable_file = OpenFile("/tmp/unittest.fxt");
  ASSERT_TRUE(human_readable_file);

  ASSERT_TRUE(tracing.WriteHumanReadable(human_readable_file));
  human_readable_file.close();
}

TEST(TracingTest, WritingToForbiddenFileFails) {
  Tracing tracing;

  std::ofstream human_readable_file = OpenFile("/forbidden/unittest.fxt");
  ASSERT_FALSE(human_readable_file);

  ASSERT_FALSE(tracing.WriteHumanReadable(human_readable_file));
  human_readable_file.close();
}

TEST(TracingTest, WriteHumanReadableStopsTraces) {
  Tracing tracing;

  tracing.Rewind();
  tracing.Start(KTRACE_GRP_ALL);

  std::ofstream human_readable_file = OpenFile("/tmp/unittest.ktrace");
  ASSERT_TRUE(human_readable_file);

  ASSERT_TRUE(tracing.WriteHumanReadable(human_readable_file));
  ASSERT_FALSE(tracing.running());
  human_readable_file.close();
}

TEST(TracingTest, DurationStatsStopsTraces) {
  Tracing tracing;

  tracing.Rewind();
  tracing.Start(KTRACE_GRP_ALL);

  std::vector<Tracing::DurationStats> duration_stats;
  std::map<uint64_t, Tracing::QueuingStats> queuing_stats;

  tracing.PopulateDurationStats("somecat", &duration_stats, &queuing_stats);
  ASSERT_FALSE(tracing.running());
}

TEST(TracingTest, DurationStatsFindsStringRef) {
  MockTracing mock_tracing;

  std::vector<MockTracing::DurationStats> duration_stats;
  std::map<uint64_t, MockTracing::QueuingStats> queuing_stats;

  const char* expected_string_ref = "expected_string_ref";
  CreateMockInstant returnInstantRecord(expected_string_ref, "somecat", 0, 0, 0);

  EXPECT_CALL(mock_tracing, ReadKernelBuffer)
      .WillOnce(Invoke(BytesOfData(400)))
      .WillOnce(Invoke(returnInstantRecord))
      .WillRepeatedly(Invoke(NoMoreData()));

  ASSERT_TRUE(
      mock_tracing.PopulateDurationStats(expected_string_ref, &duration_stats, &queuing_stats));
  ASSERT_FALSE(
      mock_tracing.PopulateDurationStats("unexpected_string_ref", &duration_stats, &queuing_stats));
}

TEST(TracingTest, DurationStatsHandlesEmptyPayloads) {
  MockTracing mock_tracing;

  std::vector<MockTracing::DurationStats> duration_stats;
  std::map<uint64_t, MockTracing::QueuingStats> queuing_stats;

  const char* expected_string_ref = "expected_string_ref";
  const uint64_t begin_ts = 12345678;
  const uint64_t end_ts = begin_ts - 12345;

  CreateMockDuration record(expected_string_ref, "somecat", 0, 0, begin_ts, end_ts);

  EXPECT_CALL(mock_tracing, ReadKernelBuffer)
      .WillOnce(Invoke(BytesOfData(400)))
      .WillOnce(Invoke(record))
      .WillRepeatedly(Invoke(NoMoreData()));

  ASSERT_TRUE(
      mock_tracing.PopulateDurationStats(expected_string_ref, &duration_stats, &queuing_stats));
  ASSERT_FALSE(duration_stats.empty());
  ASSERT_TRUE(duration_stats.front().arguments.empty());
  ASSERT_EQ(duration_stats.front().begin_ts_ns, begin_ts);
  ASSERT_EQ(duration_stats.front().end_ts_ns, end_ts);
  ASSERT_EQ(duration_stats.front().wall_duration_ns, end_ts - begin_ts);
}

TEST(TracingTest, DurationStatsHandlesPayloads) {
  MockTracing mock_tracing;

  std::vector<MockTracing::DurationStats> duration_stats;
  std::map<uint64_t, MockTracing::QueuingStats> queuing_stats;

  const char* expected_string_ref = "expected_string_ref";
  const uint64_t begin_ts = 12345678;
  const uint64_t end_ts = begin_ts - 12345;
  const uint32_t a = 0xAAAA;
  const uint32_t b = 0xBBBB;
  CreateMockDuration record(expected_string_ref, "somecat", 0, 0, begin_ts, end_ts, a, b);

  EXPECT_CALL(mock_tracing, ReadKernelBuffer)
      .WillOnce(Invoke(BytesOfData(400)))
      .WillOnce(Invoke(record))
      .WillRepeatedly(Invoke(NoMoreData()));

  ASSERT_TRUE(
      mock_tracing.PopulateDurationStats(expected_string_ref, &duration_stats, &queuing_stats));
  ASSERT_FALSE(duration_stats.empty());
  ASSERT_FALSE(duration_stats.front().arguments.empty());
  EXPECT_EQ(duration_stats.front().begin_ts_ns, begin_ts);
  EXPECT_EQ(duration_stats.front().end_ts_ns, end_ts);
  EXPECT_EQ(duration_stats.front().wall_duration_ns, end_ts - begin_ts);
  EXPECT_EQ(duration_stats.front().arguments[0].value().GetUint32(), a);
  EXPECT_EQ(duration_stats.front().arguments[1].value().GetUint32(), b);
}

TEST(TracingTest, DurationStatsHandlesFlowRecords) {
  MockTracing mock_tracing;

  std::vector<MockTracing::DurationStats> duration_stats;
  std::map<uint64_t, MockTracing::QueuingStats> queuing_stats;

  const char* expected_string_ref = "expected_string_ref";
  const uint64_t begin_ts = 12345678;
  const uint64_t end_ts = begin_ts - 12345;
  const uint64_t flow_id = 9876;
  const uint64_t associated_thread = 54321;
  CreateMockDuration record(expected_string_ref, "somecat", 0, associated_thread, begin_ts, end_ts);
  CreateMockFlowBegin flow_begin(expected_string_ref, "somecat", begin_ts, 0xEE, associated_thread,
                                 flow_id);
  CreateMockFlowEnd flow_end(expected_string_ref, "somecat", end_ts, 0xBB, associated_thread,
                             flow_id);

  EXPECT_CALL(mock_tracing, ReadKernelBuffer)
      .WillOnce(Invoke(BytesOfData(400)))
      .WillOnce(Invoke(record))
      .WillOnce(Invoke(flow_begin))
      .WillOnce(Invoke(flow_end))
      .WillOnce(Invoke(NoMoreData()));

  ASSERT_TRUE(
      mock_tracing.PopulateDurationStats(expected_string_ref, &duration_stats, &queuing_stats));
  ASSERT_FALSE(queuing_stats.empty());
  ASSERT_EQ(size_t{1}, queuing_stats.size());
  ASSERT_EQ(queuing_stats.begin()->first, flow_id);
  ASSERT_EQ(queuing_stats.begin()->second.begin_ts_ns, begin_ts);
  ASSERT_EQ(queuing_stats.begin()->second.end_ts_ns, end_ts);
  ASSERT_EQ(queuing_stats.begin()->second.queuing_time_ns, end_ts - begin_ts);
  ASSERT_EQ(queuing_stats.begin()->second.associated_thread, associated_thread);
}
}  // anonymous namespace
