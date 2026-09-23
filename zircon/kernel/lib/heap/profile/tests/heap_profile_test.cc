// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/heap_profile.h"

#include <lib/stdcompat/span.h>
#include <sys/types.h>
#include <zircon/types.h>

#include <cstdint>
#include <limits>

#include <zxtest/zxtest.h>
namespace heap_profile {
namespace {
template <typename T>
bool equals(cpp20::span<T> span1, cpp20::span<const T> span2) {
  return std::equal(span1.begin(), span1.end(), span2.begin(), span2.end());
}
template <typename T>
bool equals(cpp20::span<T> span1, cpp20::span<T> span2) {
  return std::equal(span1.begin(), span1.end(), span2.begin(), span2.end());
}

TEST(HeapProfile, DataTest) {
  constexpr size_t kSize = 42;
  Buffer<kSize> buf;
  EXPECT_EQ(kSize, buf.data().size());
}

std::span<const uint8_t> as_u8(std::span<const uint64_t> values) {
  return {reinterpret_cast<const uint8_t *>(values.data()), values.size_bytes()};
}
TEST(HeapProfile, AllocateAndAppendTest) {
  constexpr size_t kSize = sizeof(uint64_t[5]);
  Buffer<kSize> buf;

  cpp20::span<uint64_t> s1 = buf.allocate<uint64_t>(2).value();
  EXPECT_TRUE(equals(s1, {{0, 0}}));
  EXPECT_TRUE(equals(buf.data(), as_u8({{0, 0, 0, 0, 0}})));
  // Change the span, also changes the buffer.
  s1[0] = 1;
  s1[1] = 2;
  EXPECT_TRUE(equals(buf.data(), as_u8({{1, 2, 0, 0, 0}})));

  cpp20::span<uint64_t> s2 = buf.append<uint64_t>({{33, 44, 55}}).value();
  EXPECT_TRUE(equals(s2, {{33, 44, 55}}));
  EXPECT_TRUE(equals(buf.data(), as_u8({{1, 2, 33, 44, 55}})));

  s2[0] = 3;
  s2[1] = 4;
  s2[2] = 5;

  EXPECT_TRUE(equals(buf.data(), as_u8({{1, 2, 3, 4, 5}})));
}

TEST(HeapProfile, AllocateFailedTest) {
  Buffer<sizeof(uint64_t[3])> buf;
  ASSERT_TRUE(!!buf.allocate<uint64_t>(2));
  ASSERT_FALSE(!!buf.allocate<uint64_t>(2));
  ASSERT_TRUE(!!buf.allocate<uint64_t>(1));
  ASSERT_FALSE(!!buf.allocate<uint64_t>(1));
}

TEST(HeapProfile, AppendFailedTest) {
  Buffer<sizeof(uint64_t[3])> buf;
  ASSERT_TRUE(!!buf.append<uint64_t>({{1, 2}}));
  ASSERT_FALSE(!!buf.append<uint64_t>({{3, 4}}));
  ASSERT_TRUE(!!buf.append<uint64_t>({{5}}));
  ASSERT_FALSE(!!buf.append<uint64_t>({{6}}));
  EXPECT_TRUE(equals(buf.data(), as_u8({{1, 2, 5}})));
}

TEST(HeapProfile, MoreOffsetTest) {
  constexpr size_t bucket_count = 2;
  HeapProfileMap</*ValuesSize=*/1024, /*Capacity=*/128, /*BuckeCount=*/bucket_count> map;

  // A number larger than the number of buckets, so that Entry::next is used.
  constexpr size_t insert_count = bucket_count * 2;
  for (size_t i = 0; i < insert_count; i++) {
    auto [v, handle] = map.try_get({{i, 2, 3}});
    ASSERT_TRUE(v);
    ASSERT_TRUE(handle);
    EXPECT_EQ(0UL, v->live_count);
    EXPECT_EQ(0UL, v->live_bytes);
    EXPECT_EQ(0UL, v->total_count);
    EXPECT_EQ(0UL, v->total_bytes);
    v->live_count = i;
  }

  // Getting the same values again.
  for (size_t i = 0; i < insert_count; i++) {
    auto [v, o] = map.try_get({{i, 2, 3}});

    EXPECT_EQ(i, v->live_count);
    EXPECT_EQ(0UL, v->live_bytes);
    EXPECT_EQ(0UL, v->total_count);
    EXPECT_EQ(0UL, v->total_bytes);

    Counters *vv = map.try_by_handle(o);
    ASSERT_TRUE(vv);
    EXPECT_EQ(i, vv->live_count);
  }
}

TEST(HeapProfile, MapTest) {
  HeapProfileMap<sizeof(uint64_t[8]), 128, 128> map;
  auto [counters, handle] = map.try_get({{0x1122334455667788}});
  ASSERT_NOT_NULL(counters);
  counters->allocate(42);

  // No event dropped.
  EXPECT_TRUE(equals(map.data().subspan(0, sizeof(Header)), as_u8({{0x0000000000000001}})));
  // This is not dropping an element.
  ASSERT_NULL(std::get<0>(map.try_get({{0xaabbccdd}})));
  // No event dropped.
  EXPECT_TRUE(equals(map.data().subspan(0, sizeof(Header)), as_u8({{0x0000000000000001}})));
  map.event_dropped();
  EXPECT_TRUE(equals(map.data().subspan(0, sizeof(Header)), as_u8({{0x0000000100000001}})));
  map.event_dropped();

  // Event dropped and buffer with trailing zeros.
  EXPECT_TRUE(equals(map.data(), as_u8({{/*header         */ 0x0000000200000001,
                                         /*live_count     */ 1,
                                         /*live_bytes     */ 42,
                                         /*total_count    */ 1,
                                         /*total_bytes    */ 42,
                                         /*backtrace_size */ 1,
                                         /*backtrace      */ 0x1122334455667788,
                                         /*zeros          */ 0x0000000000000000}})));
}

TEST(HeapProfile, AlignmentTest) {
  HeapProfileMap<24, 2, 2> v1;
  // The buffer is at the same address and same alignment as the HeapProfileMap.
  EXPECT_EQ((void *)&v1, (void *)v1.data().data());
}

}  // namespace
}  // namespace heap_profile
