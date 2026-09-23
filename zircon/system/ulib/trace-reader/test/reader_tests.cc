// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "reader_tests.h"

#include <lib/trace-engine/fields.h>
#include <stdint.h>

#include <iterator>
#include <string>
#include <vector>

#include <trace-reader/reader.h>
#include <zxtest/zxtest.h>

namespace trace {
namespace {

TEST(TraceReader, NonEmptyChunk) {
  uint64_t kData[] = {
      // uint64 values
      0,
      UINT64_MAX,
      // int64 values
      test::ToWord(INT64_MIN),
      test::ToWord(INT64_MAX),
      // double values
      test::ToWord(1.5),
      test::ToWord(-3.14),
      // string values (will be filled in)
      0,
      0,
      // sub-chunk values
      123,
      456,
      // more stuff beyond sub-chunk
      789,
  };
  memcpy(kData + 6, "Hello World!----", 16);

  trace::Chunk chunk(kData, std::size(kData));
  EXPECT_EQ(std::size(kData), chunk.remaining_words());

  {
    std::optional value = chunk.ReadUint64();
    EXPECT_TRUE(value.has_value());
    EXPECT_EQ(0, value.value());
    EXPECT_EQ(10u, chunk.remaining_words());
  }

  {
    std::optional value = chunk.ReadUint64();
    EXPECT_TRUE(value.has_value());
    EXPECT_EQ(UINT64_MAX, value.value());
    EXPECT_EQ(9u, chunk.remaining_words());
  }

  {
    std::optional value = chunk.ReadInt64();
    EXPECT_TRUE(value.has_value());
    EXPECT_EQ(INT64_MIN, value.value());
    EXPECT_EQ(8u, chunk.remaining_words());
  }

  {
    std::optional value = chunk.ReadInt64();
    EXPECT_TRUE(value.has_value());
    EXPECT_EQ(INT64_MAX, value.value());
    EXPECT_EQ(7u, chunk.remaining_words());
  }

  {
    std::optional value = chunk.ReadDouble();
    EXPECT_TRUE(value.has_value());
    EXPECT_EQ(1.5, value.value());
    EXPECT_EQ(6u, chunk.remaining_words());
  }

  {
    std::optional value = chunk.ReadDouble();
    EXPECT_TRUE(value.has_value());
    EXPECT_EQ(-3.14, value.value());
    EXPECT_EQ(5u, chunk.remaining_words());
  }

  {
    std::optional value = chunk.ReadString(0);
    EXPECT_TRUE(value.has_value());
    EXPECT_TRUE(value.value().empty());
    EXPECT_EQ(5u, chunk.remaining_words());
  }

  {
    std::optional value = chunk.ReadString(12);
    EXPECT_TRUE(value.has_value());
    EXPECT_EQ(12, value.value().length());
    EXPECT_EQ(reinterpret_cast<const char*>(kData + 6), value.value().data());
    EXPECT_EQ(value.value(), "Hello World!");
    EXPECT_EQ(3u, chunk.remaining_words());
  }

  {
    std::optional subchunk = chunk.ReadChunk(2);
    EXPECT_TRUE(subchunk.has_value());
    EXPECT_EQ(2u, subchunk.value().remaining_words());

    {
      std::optional value = subchunk.value().ReadUint64();
      EXPECT_TRUE(value.has_value());
      EXPECT_EQ(123, value.value());
      EXPECT_EQ(1u, subchunk.value().remaining_words());
    }

    {
      std::optional value = chunk.ReadUint64();
      EXPECT_TRUE(value.has_value());
      EXPECT_EQ(789, value.value());
      EXPECT_EQ(0u, chunk.remaining_words());
    }

    {
      std::optional value = subchunk.value().ReadUint64();
      EXPECT_TRUE(value.has_value());
      EXPECT_EQ(456, value.value());
      EXPECT_EQ(0u, subchunk.value().remaining_words());
    }

    {
      EXPECT_FALSE(subchunk.value().ReadUint64().has_value());
      EXPECT_FALSE(chunk.ReadUint64().has_value());
    }
  }
}

TEST(TraceReader, InitialState) {
  std::vector<trace::Record> records;
  std::string_view error;
  trace::TraceReader reader(test::MakeRecordConsumer(&records), test::MakeErrorHandler(&error));

  EXPECT_EQ(0, reader.current_provider_id());
  EXPECT_TRUE(reader.current_provider_name() == "");
  EXPECT_TRUE(reader.GetProviderName(0) == "");
  EXPECT_EQ(0, records.size());
  EXPECT_TRUE(error.empty());
}

// NOTE: Most of the reader is covered by the libtrace tests.

TEST(TraceReader, ProfilerRecords) {
  std::vector<trace::Record> records;
  std::string_view error;
  trace::TraceReader reader(test::MakeRecordConsumer(&records), test::MakeErrorHandler(&error));

  constexpr zx_koid_t kProcessKoid = 1;
  constexpr zx_koid_t kThreadKoid = 2;

  static constexpr std::byte build_id_data[] = {
      std::byte{0x33}, std::byte{0x3e}, std::byte{0x89}, std::byte{0xf0}, std::byte{0xc1},
      std::byte{0x75}, std::byte{0x00}, std::byte{0x0c}, std::byte{0xee}, std::byte{0x9b},
      std::byte{0x7e}, std::byte{0x20}, std::byte{0x1f}, std::byte{0xed}, std::byte{0xcd},
      std::byte{0x6f}, std::byte{0x9b}, std::byte{0x4b}, std::byte{0xa8}, std::byte{0xae}};

  const std::vector<std::byte> new_build_id(std::begin(build_id_data), std::end(build_id_data));

  uint64_t kData[21];

  // Module record (8 words)
  size_t next = 0;
  kData[next++] = (trace::RecordFields::RecordSize::Make(8) |
                   trace::ToUnderlyingType(trace::RecordType::kProfiler) |
                   trace::ProfilerRecordFields::ThreadRef::Make(TRACE_ENCODED_THREAD_REF_INLINE) |
                   trace::ProfilerRecordFields::ProfilerType::Make(
                       trace::ToUnderlyingType(trace::ProfilerRecordType::kModule)) |
                   trace::ProfilerModuleRecordFields::ModuleId::Make(1) |
                   trace::ProfilerModuleRecordFields::NameLength::Make(4) |
                   trace::ProfilerModuleRecordFields::BuildIdLength::Make(new_build_id.size()));
  kData[next++] = 0;  // timestamp
  kData[next++] = kProcessKoid;
  kData[next++] = kThreadKoid;
  memcpy(&kData[next], "test", 4);
  next++;  // for "test"
  memcpy(&kData[next], new_build_id.data(), new_build_id.size());
  next += 3;  // for new_build_id

  // Mmap record (7 words)
  kData[next++] = (trace::RecordFields::RecordSize::Make(7) |
                   trace::ToUnderlyingType(trace::RecordType::kProfiler) |
                   trace::ProfilerRecordFields::ThreadRef::Make(TRACE_ENCODED_THREAD_REF_INLINE) |
                   trace::ProfilerRecordFields::ProfilerType::Make(
                       trace::ToUnderlyingType(trace::ProfilerRecordType::kMmap)) |
                   trace::ProfilerMmapRecordFields::ModuleId::Make(1) |
                   trace::ProfilerMmapRecordFields::Flags::Make(7));
  kData[next++] = 0;  // timestamp
  kData[next++] = kProcessKoid;
  kData[next++] = kThreadKoid;
  kData[next++] = 0x1000;  // start_address
  kData[next++] = 0x2000;  // address_range
  kData[next++] = 0x3000;  // vaddr

  // Backtrace record (6 words)
  kData[next++] = (trace::RecordFields::RecordSize::Make(6) |
                   trace::ToUnderlyingType(trace::RecordType::kProfiler) |
                   trace::ProfilerRecordFields::ThreadRef::Make(TRACE_ENCODED_THREAD_REF_INLINE) |
                   trace::ProfilerRecordFields::ProfilerType::Make(
                       trace::ToUnderlyingType(trace::ProfilerRecordType::kBacktrace)) |
                   trace::ProfilerBacktraceRecordFields::BacktraceCount::Make(2));
  kData[next++] = 0;  // timestamp
  kData[next++] = kProcessKoid;
  kData[next++] = kThreadKoid;
  kData[next++] = 0x4000;  // pc 1
  kData[next++] = 0x5000;  // pc 2

  ASSERT_EQ(next, std::size(kData));

  trace::Chunk chunk(kData, std::size(kData));
  EXPECT_TRUE(reader.ReadRecords(chunk));
  EXPECT_EQ(3, records.size());
  EXPECT_TRUE(error.empty());

  const auto& module = records[0].GetProfiler().module();
  EXPECT_EQ(1, module.module_id);
  EXPECT_EQ(trace::ProcessThread(1, 2), module.process_thread);
  EXPECT_EQ("test", module.name);
  EXPECT_TRUE(std::ranges::equal(new_build_id, module.build_id));

  const auto& mmap = records[1].GetProfiler().mmap();
  EXPECT_EQ(1, mmap.module_id);
  EXPECT_EQ(trace::ProcessThread(1, 2), mmap.process_thread);
  EXPECT_EQ(0x1000, mmap.start_address);
  EXPECT_EQ(0x2000, mmap.address_range);
  EXPECT_EQ(0x3000, mmap.vaddr);
  EXPECT_EQ(7, mmap.flags);

  const auto& backtrace = records[2].GetProfiler().backtrace();
  EXPECT_EQ(trace::ProcessThread(1, 2), backtrace.process_thread);
  EXPECT_EQ(2, backtrace.backtrace.size());
  EXPECT_EQ(0x4000, backtrace.backtrace[0]);
  EXPECT_EQ(0x5000, backtrace.backtrace[1]);
}

TEST(TraceReader, KernelObjectNonUtf8Name) {
  std::vector<trace::Record> records;
  std::string_view error;
  trace::TraceReader reader(test::MakeRecordConsumer(&records), test::MakeErrorHandler(&error));

  // KernelObject record: 3 words
  // Header word: type=7 (kKernelObject), size=3 words (24 bytes), obj_type=1 (ZX_OBJ_TYPE_PROCESS),
  //              name_ref = inline flag (0x8000) | length (5) -> 0x8005, arg_count=0
  uint64_t kData[3] = {};
  kData[0] = (trace::RecordFields::RecordSize::Make(3) |
              trace::ToUnderlyingType(trace::RecordType::kKernelObject) |
              trace::KernelObjectRecordFields::ObjectType::Make(1) |
              trace::KernelObjectRecordFields::NameStringRef::Make(0x8005) |
              trace::KernelObjectRecordFields::ArgumentCount::Make(0));
  kData[1] = 1234;  // koid

  // Non-UTF-8 5-byte string: "he\xfflo" plus 3 zero padding bytes
  char raw_name[8] = {'h', 'e', static_cast<char>(0xff), 'l', 'o', 0, 0, 0};
  memcpy(&kData[2], raw_name, 8);

  trace::Chunk chunk(kData, std::size(kData));
  EXPECT_TRUE(reader.ReadRecords(chunk));
  EXPECT_EQ(1, records.size());
  EXPECT_TRUE(error.empty());

  const auto& kobj = records[0].GetKernelObject();
  EXPECT_EQ(1234, kobj.koid);
  EXPECT_EQ(1, kobj.object_type);
  EXPECT_EQ(std::string("he\xfflo", 5), kobj.name);
}

}  // namespace
}  // namespace trace
