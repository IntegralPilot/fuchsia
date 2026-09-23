// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <lib/power/state_recorder/cpp/inspect_buffer.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>

#include <gtest/gtest.h>

namespace power_observability::internal {

constexpr size_t kTestBufferSize = 100;

// The test suites below cover three different categories of functionality:
//  - TimestampedBufferTemplateTest: Basic functionality of TimestampedBuffer, focusing on the
//    template types it supports.
//  - TimestampedByteBufferTest: Broader functionality of TimestampedBuffer, restricted to uint8_t.
//  - TimestampedBitBufferTest: Special behavior when ValueType==bool, and the specialized BitBuffer
//    is used to store data.
using TimestampedByteBuffer = TimestampedBuffer<uint8_t>;
using TimestampedBitBuffer = TimestampedBuffer<bool>;

template <typename T>
std::vector<DataPoint<T>> ReconstructSeries(TimestampedBuffer<T>& buffer) {
  std::vector<DataPoint<T>> series;
  buffer.ForEachDataPoint([&](const DataPoint<T>& data_point) { series.push_back(data_point); });
  return series;
}

TEST(TimestampedBufferTest, MultipleEntriesUnsigned) {
  auto test_body = [&](auto type_val) {
    using T = decltype(type_val);
    TimestampedBuffer<T> buffer(kTestBufferSize);
    buffer.AddEntry(10, 100);
    buffer.AddEntry(20, 200);
    buffer.AddEntry(30, 300);
    auto series = ReconstructSeries(buffer);
    ASSERT_EQ(series.size(), 3u);
    EXPECT_EQ(series[0].value, static_cast<T>(10));
    EXPECT_EQ(series[1].value, static_cast<T>(20));
    EXPECT_EQ(series[2].value, static_cast<T>(30));
  };
  test_body(uint8_t{});
  test_body(uint16_t{});
  test_body(uint32_t{});
  test_body(uint64_t{});
}

TEST(TimestampedBufferTest, MultipleEntriesSigned) {
  auto test_body = [&](auto type_val) {
    using T = decltype(type_val);
    TimestampedBuffer<T> buffer(kTestBufferSize);
    buffer.AddEntry(-10, 1000);
    buffer.AddEntry(20, 2000);
    buffer.AddEntry(-30, 3000);
    auto series = ReconstructSeries(buffer);
    ASSERT_EQ(series.size(), 3u);
    EXPECT_EQ(series[0].value, static_cast<T>(-10));
    EXPECT_EQ(series[1].value, static_cast<T>(20));
    EXPECT_EQ(series[2].value, static_cast<T>(-30));
  };
  test_body(int8_t{});
  test_body(int16_t{});
  test_body(int32_t{});
  test_body(int64_t{});
}

TEST(TimestampedBufferTest, MultipleEntriesFloatingPoint) {
  auto test_body = [&](auto type_val) {
    using T = decltype(type_val);
    TimestampedBuffer<T> buffer(kTestBufferSize);
    buffer.AddEntry(-10.1f, 1000);
    buffer.AddEntry(20.1f, 2000);
    buffer.AddEntry(-30.1f, 3000);
    auto series = ReconstructSeries(buffer);
    ASSERT_EQ(series.size(), 3u);
    EXPECT_EQ(series[0].value, static_cast<T>(-10.1f));
    EXPECT_EQ(series[1].value, static_cast<T>(20.1f));
    EXPECT_EQ(series[2].value, static_cast<T>(-30.1f));
  };
  test_body(float{});
  test_body(double{});
}

TEST(TimestampedBufferTest, MultipleEntriesEnum) {
  enum class MyEnum : uint8_t {
    kOne = 1,
    kTwo = 2,
    kThree = 3,
  };
  TimestampedBuffer<MyEnum> buffer(kTestBufferSize);
  buffer.AddEntry(MyEnum::kOne, 1000);
  buffer.AddEntry(MyEnum::kTwo, 2000);
  buffer.AddEntry(MyEnum::kThree, 3000);
  auto series = ReconstructSeries(buffer);

  ASSERT_EQ(series.size(), 3u);
  EXPECT_EQ(series[0].value, MyEnum::kOne);
  EXPECT_EQ(series[1].value, MyEnum::kTwo);
  EXPECT_EQ(series[2].value, MyEnum::kThree);
}

TEST(TimestampedByteBufferTest, SingleEntry) {
  TimestampedByteBuffer buffer(kTestBufferSize);
  buffer.AddEntry(42, 1000);
  auto series = ReconstructSeries(buffer);
  ASSERT_EQ(series.size(), 1u);
  EXPECT_EQ(series[0].value, 42);
}

TEST(TimestampedByteBufferTest, AcceptDecreasingTimestamp) {
  TimestampedByteBuffer buffer(kTestBufferSize);
  buffer.AddEntry(42, 1000);
  buffer.AddEntry(30, 500);  // Decreasing timestamp
  buffer.AddEntry(50, 2000);

  auto series = ReconstructSeries(buffer);
  ASSERT_EQ(series.size(), 3u);
  EXPECT_EQ(series[0].value, 42);
  EXPECT_EQ(series[1].value, 30);
  EXPECT_EQ(series[2].value, 50);
}

TEST(TimestampedByteBufferTest, BufferWrapAround) {
  TimestampedByteBuffer buffer(kTestBufferSize);
  for (size_t i = 0; i < kTestBufferSize + 10; ++i) {
    buffer.AddEntry(static_cast<uint8_t>(i), 1000 * i);
  }
  auto series = ReconstructSeries(buffer);
  ASSERT_EQ(series.size(), kTestBufferSize);
  for (size_t i = 0; i < kTestBufferSize; ++i) {
    EXPECT_EQ(series[i].value, static_cast<uint8_t>(i + 10));
  }
}

TEST(TimestampedByteBufferTest, DeltaOverflowResetsBuffer) {
  TimestampedByteBuffer buffer(kTestBufferSize);
  buffer.AddEntry(10, 1000);
  buffer.AddEntry(20, 2000);
  buffer.AddEntry(30, 3000);
  constexpr zx::duration kLastTimestamp = zx::msec(3000);
  constexpr zx::duration kTimeDelta =
      zx::msec(static_cast<int64_t>(std::numeric_limits<int32_t>::max()) + 500);
  const zx::duration overflow_ts = kLastTimestamp + kTimeDelta;
  buffer.AddEntry(40, overflow_ts.to_msecs());
  auto series = ReconstructSeries(buffer);
  auto reset_info = buffer.GetResetInfo();

  ASSERT_EQ(series.size(), 1u);
  EXPECT_EQ(series[0].value, 40);
  EXPECT_EQ(series[0].timestamp_ns, overflow_ts.to_nsecs());
  EXPECT_EQ(reset_info.count, 1u);
  EXPECT_EQ(reset_info.last_reset_ns, overflow_ts.to_nsecs());
}

TEST(TimestampedByteBufferTest, DeltaUnderflowResetsBuffer) {
  TimestampedByteBuffer buffer(kTestBufferSize);
  buffer.AddEntry(10, 1000);
  buffer.AddEntry(20, 2000);
  buffer.AddEntry(30, 3000);
  constexpr zx::duration kLastTimestamp = zx::msec(3000);
  constexpr zx::duration kTimeDelta =
      zx::msec(static_cast<int64_t>(std::numeric_limits<int32_t>::min()) - 500);
  const zx::duration underflow_ts = kLastTimestamp + kTimeDelta;
  buffer.AddEntry(40, underflow_ts.to_msecs());
  auto series = ReconstructSeries(buffer);
  auto reset_info = buffer.GetResetInfo();

  ASSERT_EQ(series.size(), 1u);
  EXPECT_EQ(series[0].value, 40);
  EXPECT_EQ(series[0].timestamp_ns, underflow_ts.to_nsecs());
  EXPECT_EQ(reset_info.count, 1u);
  EXPECT_EQ(reset_info.last_reset_ns, underflow_ts.to_nsecs());
}

TEST(TimestampedByteBufferTest, DeltaOverflowAtFullBuffer) {
  TimestampedByteBuffer buffer(kTestBufferSize);
  zx::duration current_ts = zx::msec(1000);
  // Fill the buffer completely.
  for (size_t i = 0; i < kTestBufferSize; ++i) {
    buffer.AddEntry(static_cast<uint8_t>(i), current_ts.to_msecs());
    current_ts += zx::msec(100);  // Increment timestamp for each entry.
  }
  // Trigger an overflow.
  const zx::duration last_ts_before_overflow = current_ts - zx::msec(100);
  constexpr zx::duration kTimeDelta =
      zx::msec(static_cast<int64_t>(std::numeric_limits<uint32_t>::max()) + 1);
  const zx::duration overflow_ts = last_ts_before_overflow + kTimeDelta;
  buffer.AddEntry(255, overflow_ts.to_msecs());
  auto series = ReconstructSeries(buffer);

  ASSERT_EQ(series.size(), 1u);
  EXPECT_EQ(series[0].value, 255);
  EXPECT_EQ(series[0].timestamp_ns, overflow_ts.to_nsecs());
}

TEST(TimestampedBitBufferTest, SingleEntry) {
  TimestampedBitBuffer buffer(kTestBufferSize);
  buffer.AddEntry(true, 1000);
  auto series = ReconstructSeries(buffer);

  ASSERT_EQ(series.size(), 1u);
  EXPECT_EQ(series[0].value, true);
}

TEST(TimestampedBitBufferTest, MultipleEntries) {
  TimestampedBitBuffer buffer(kTestBufferSize);
  buffer.AddEntry(true, 1000);
  buffer.AddEntry(false, 2000);
  buffer.AddEntry(true, 3000);
  auto series = ReconstructSeries(buffer);

  ASSERT_EQ(series.size(), 3u);
  EXPECT_EQ(series[0].value, true);
  EXPECT_EQ(series[1].value, false);
  EXPECT_EQ(series[2].value, true);
}

TEST(TimestampedBitBufferTest, BufferWrapAround) {
  TimestampedBitBuffer buffer(kTestBufferSize);
  for (size_t i = 0; i < kTestBufferSize + 10; ++i) {
    buffer.AddEntry(i % 2 == 0, 1000 * i);
  }
  auto series = ReconstructSeries(buffer);

  ASSERT_EQ(series.size(), kTestBufferSize);
  for (size_t i = 0; i < kTestBufferSize; ++i) {
    EXPECT_EQ(series[i].value, (i + 10) % 2 == 0);
  }
}

TEST(TimestampedBitBufferTest, DeltaOverflowResetsBuffer) {
  TimestampedBitBuffer buffer(kTestBufferSize);
  buffer.AddEntry(true, 1000);
  buffer.AddEntry(false, 2000);
  buffer.AddEntry(true, 3000);
  constexpr zx::duration kLastTimestamp = zx::msec(3000);
  constexpr zx::duration kTimeDelta =
      zx::msec(static_cast<int64_t>(std::numeric_limits<int32_t>::max()) + 500);
  const zx::duration overflow_ts = kLastTimestamp + kTimeDelta;
  buffer.AddEntry(false, overflow_ts.to_msecs());
  auto series = ReconstructSeries(buffer);
  auto reset_info = buffer.GetResetInfo();

  ASSERT_EQ(series.size(), 1u);
  EXPECT_EQ(series[0].value, false);
  EXPECT_EQ(series[0].timestamp_ns, overflow_ts.to_nsecs());
  EXPECT_EQ(reset_info.count, 1u);
  EXPECT_EQ(reset_info.last_reset_ns, overflow_ts.to_nsecs());
}

TEST(TimestampedBitBufferTest, DeltaUnderflowResetsBuffer) {
  TimestampedBitBuffer buffer(kTestBufferSize);
  buffer.AddEntry(true, 1000);
  buffer.AddEntry(false, 2000);
  buffer.AddEntry(true, 3000);
  constexpr zx::duration kLastTimestamp = zx::msec(3000);
  constexpr zx::duration kTimeDelta =
      zx::msec(static_cast<int64_t>(std::numeric_limits<int32_t>::min()) - 500);
  const zx::duration underflow_ts = kLastTimestamp + kTimeDelta;
  buffer.AddEntry(false, underflow_ts.to_msecs());
  auto series = ReconstructSeries(buffer);
  auto reset_info = buffer.GetResetInfo();

  ASSERT_EQ(series.size(), 1u);
  EXPECT_EQ(series[0].value, false);
  EXPECT_EQ(series[0].timestamp_ns, underflow_ts.to_nsecs());
  EXPECT_EQ(reset_info.count, 1u);
  EXPECT_EQ(reset_info.last_reset_ns, underflow_ts.to_nsecs());
}

TEST(TimestampedBitBufferTest, DeltaOverflowAtFullBuffer) {
  TimestampedBitBuffer buffer(kTestBufferSize);
  zx::duration current_ts = zx::msec(1000);
  // Fill the buffer completely.
  for (size_t i = 0; i < kTestBufferSize; ++i) {
    buffer.AddEntry(i % 2 == 0, current_ts.to_msecs());
    current_ts += zx::msec(100);  // Increment timestamp for each entry.
  }
  // Trigger an overflow.
  const zx::duration last_ts_before_overflow = current_ts - zx::msec(100);
  constexpr zx::duration kTimeDelta =
      zx::msec(static_cast<int64_t>(std::numeric_limits<uint32_t>::max()) + 1);
  const zx::duration overflow_ts = last_ts_before_overflow + kTimeDelta;
  buffer.AddEntry(true, overflow_ts.to_msecs());
  auto series = ReconstructSeries(buffer);
  auto reset_info = buffer.GetResetInfo();

  ASSERT_EQ(series.size(), 1u);
  EXPECT_EQ(series[0].value, true);
  EXPECT_EQ(series[0].timestamp_ns, overflow_ts.to_nsecs());
  EXPECT_EQ(reset_info.count, 1u);
  EXPECT_EQ(reset_info.last_reset_ns, overflow_ts.to_nsecs());
}

}  // namespace power_observability::internal
