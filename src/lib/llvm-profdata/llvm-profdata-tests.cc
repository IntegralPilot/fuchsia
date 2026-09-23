// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/note.h>
#include <lib/elfldltl/self.h>
#include <lib/llvm-profdata/llvm-profdata.h>

#include <cstdint>
#include <memory>
#include <span>

#include <gtest/gtest.h>

#include "coverage-example.h"

#include <profile/InstrProfData.inc>

namespace {

// The compiler doesn't support relocatable mode on macOS.
#ifdef __APPLE__
constexpr bool kRelocatableCounters = false;
#else
constexpr bool kRelocatableCounters = true;
#endif

std::span<const std::byte> MyBuildId() {
  // TODO(mcgrathr): For these unit tests, it doesn't matter what the ID is.
  // For end-to-end tests using the offline tools, this will need to be the
  // real build ID of the test module.
  static constexpr std::byte kId[] = {std::byte{0xaa}, std::byte{0xbb}};
  return kId;
}

TEST(LlvmProfdataTests, SizeBytes) {
  LlvmProfdata data;
  data.Init(MyBuildId());
  EXPECT_GT(data.size_bytes(), size_t{0});
}

TEST(LlvmProfdataTests, CountersOffsetAndSizeBytes) {
  LlvmProfdata data;
  data.Init(MyBuildId());
  EXPECT_GT(data.counters_offset(), size_t{0});
  EXPECT_GT(data.counters_size_bytes(), size_t{0});
  EXPECT_LT(data.counters_offset(), data.size_bytes());
  EXPECT_LE(data.counters_size_bytes(), data.size_bytes() - data.counters_offset());
}

TEST(LlvmProfdataTests, FixedData) {
  LlvmProfdata data;
  data.Init(MyBuildId());

  const size_t buffer_size = data.size_bytes();
  ASSERT_GT(buffer_size, size_t{0});
  std::unique_ptr<std::byte[]> buffer(new std::byte[buffer_size]);
  const std::span buffer_span(buffer.get(), buffer_size);

  auto live_data = data.WriteFixedData(buffer_span);
  ASSERT_FALSE(live_data.counters.empty());

  EXPECT_TRUE(data.Match(buffer_span));

  auto matched_data = data.VerifyMatch(buffer_span);
  EXPECT_EQ(matched_data.counters.data(), live_data.counters.data());
  EXPECT_EQ(matched_data.counters.size_bytes(), live_data.counters.size_bytes());

  // Also test LiveDataForBounds.
  auto bounds = data.bounds();
  EXPECT_GT(bounds.size, size_t{0});
  EXPECT_LE(bounds.offset, buffer_size);
  EXPECT_LE(bounds.size, buffer_size - bounds.offset);

  auto resolved_live_data = data.LiveDataForBounds(buffer_span.subspan(bounds.offset, bounds.size));
  EXPECT_EQ(std::vector<std::byte>(resolved_live_data.counters.begin(),
                                   resolved_live_data.counters.end()),
            std::vector<std::byte>(live_data.counters.begin(), live_data.counters.end()));
  EXPECT_EQ(
      std::vector<std::byte>(resolved_live_data.bitmap.begin(), resolved_live_data.bitmap.end()),
      std::vector<std::byte>(live_data.bitmap.begin(), live_data.bitmap.end()));
#if INSTR_PROF_RAW_VERSION >= 11
  EXPECT_EQ(
      std::vector<std::byte>(resolved_live_data.uniform_counters.begin(),
                             resolved_live_data.uniform_counters.end()),
      std::vector<std::byte>(live_data.uniform_counters.begin(), live_data.uniform_counters.end()));
#endif
}

TEST(LlvmProfdataTests, CopyLiveData) {
  if (LlvmProfdata::UsingSingleByteCounters())
    GTEST_SKIP() << "Not supported in single byte counters mode";

  LlvmProfdata data;
  data.Init(MyBuildId());

  const size_t buffer_size = data.size_bytes();
  ASSERT_GT(buffer_size, size_t{0});
  std::unique_ptr<std::byte[]> buffer(new std::byte[buffer_size]);
  const std::span buffer_span(buffer.get(), buffer_size);

  auto live_data = data.WriteFixedData(buffer_span);
  ASSERT_FALSE(live_data.counters.empty());

  const std::span<uint64_t> counters{
      reinterpret_cast<uint64_t*>(live_data.counters.data()),
      live_data.counters.size_bytes() / sizeof(uint64_t),
  };

  // Fill the buffer with unreasonable counter values.
  std::fill(counters.begin(), counters.end(), ~uint64_t{});

  // Now copy out the current values.
  data.CopyLiveData({
      .counters = std::as_writable_bytes(counters),
      .bitmap = live_data.bitmap,
      .uniform_counters = live_data.uniform_counters,
  });

  // None of the real values should be the unreasonable value.
  for (size_t i = 0; i < counters.size(); ++i) {
    EXPECT_NE(counters[i], ~uint64_t{}) << "counter " << i;
  }

  // In case the normal profile runtime is also active, reset the bias.
  LlvmProfdata::UseLinkTimeLiveData();

  // Now run some instrumented code that should be sure to touch a counter.
  RunTimeCoveredFunction();

  std::unique_ptr<uint64_t[]> new_buffer(new uint64_t[counters.size()]);
  const std::span new_counters(new_buffer.get(), counters.size());

  // Fill the buffer with unreasonable counter values.
  std::fill(new_counters.begin(), new_counters.end(), ~uint64_t{});

  // Now copy out the new values after running covered code.
  data.CopyLiveData({
      .counters = std::as_writable_bytes(new_counters),
      .bitmap = live_data.bitmap,
      .uniform_counters = live_data.uniform_counters,
  });

  uint64_t increase = 0;
  for (size_t i = 0; i < counters.size(); ++i) {
    // None of the real values should be the unreasonable value.
    EXPECT_NE(new_counters[i], ~uint64_t{}) << "counter " << i;

    // No counter should have decreased.
    EXPECT_GE(new_counters[i], counters[i]);

    // Accumulate all the increased hit counts together.
    increase += new_counters[i] - counters[i];
  }

  // At least one counter in RunTimeCoveredFunction should have increased.
  EXPECT_GT(increase, uint64_t{0});
}

TEST(LlvmProfdataTests, CopyLiveDataSingleByteCounters) {
  if (!LlvmProfdata::UsingSingleByteCounters())
    GTEST_SKIP() << "Not supported in eight byte counters mode";

  LlvmProfdata data;
  data.Init(MyBuildId());

  const size_t buffer_size = data.size_bytes();
  ASSERT_GT(buffer_size, size_t{0});
  std::unique_ptr<std::byte[]> buffer(new std::byte[buffer_size]);
  const std::span buffer_span(buffer.get(), buffer_size);

  auto live_data = data.WriteFixedData(buffer_span);
  ASSERT_FALSE(live_data.counters.empty());

  const std::span<uint8_t> counters{
      reinterpret_cast<uint8_t*>(live_data.counters.data()),
      live_data.counters.size_bytes() / sizeof(uint8_t),
  };

  // Fill the buffer with unreasonable counter values.
  // A value of zero means the function is covered.
  // A value of 0xff means the function is not covered.
  std::fill(counters.begin(), counters.end(), 1);

  // Now copy out the current values.
  data.CopyLiveData({
      .counters = std::as_writable_bytes(counters),
      .bitmap = live_data.bitmap,
      .uniform_counters = live_data.uniform_counters,
  });

  // None of the real values should be the unreasonable value.
  for (size_t i = 0; i < counters.size(); ++i) {
    EXPECT_NE(counters[i], 1) << "counter " << i;
  }

  // In case the normal profile runtime is also active, reset the bias.
  LlvmProfdata::UseLinkTimeLiveData();

  // Now run some instrumented code that should be sure to touch a counter.
  RunTimeCoveredFunction();

  std::unique_ptr<uint8_t[]> new_buffer(new uint8_t[counters.size()]);
  const std::span new_counters(new_buffer.get(), counters.size());

  // Fill the buffer with unreasonable counter values.
  std::fill(new_counters.begin(), new_counters.end(), 1);

  // Now copy out the new values after running covered code.
  data.CopyLiveData({
      .counters = std::as_writable_bytes(new_counters),
      .bitmap = live_data.bitmap,
      .uniform_counters = live_data.uniform_counters,
  });

  for (size_t i = 0; i < counters.size(); ++i) {
    // None of the real values should be the unreasonable value.
    EXPECT_NE(new_counters[i], 1) << "counter " << i;
  }

  uint8_t covered = 0;
  for (size_t i = 0; i < counters.size(); ++i) {
    // Accumulate all the covered hits together.
    if (new_counters[i] != counters[i])
      covered &= new_counters[i];
  }

  // At least one counter in RunTimeCoveredFunction should have covered.
  EXPECT_NE(covered, uint8_t{1});
}

TEST(LlvmProfdataTests, MergeLiveData) {
  if (LlvmProfdata::UsingSingleByteCounters())
    GTEST_SKIP() << "Not supported in single byte counters mode";

  static constexpr uint64_t kOldCounters[] = {1, 2, 3, 4};
  uint64_t new_counters[] = {5, 6, 7, 8};
  static_assert(std::size(kOldCounters) == std::size(new_counters));

  static constexpr uint8_t kOldBitmap[] = {0, 0x01, 0x02, 0x03};
  uint8_t new_bitmap[] = {1, 0x11, 0x20, 0x31};

  static constexpr uint64_t kOldUniformCounters[] = {10, 20};
  uint64_t new_uniform_counters[] = {30, 40};
  static_assert(std::size(kOldUniformCounters) == std::size(new_uniform_counters));

  constexpr auto as_falsely_writable = [](auto span) {
    std::span<const std::byte> bytes = std::as_bytes(span);
    return std::span<std::byte>{
        const_cast<std::byte*>(bytes.data()),
        bytes.size(),
    };
  };

  LlvmProfdata::MergeLiveData(
      {
          .counters = std::as_writable_bytes(std::span(new_counters)),
          .bitmap = std::as_writable_bytes(std::span(new_bitmap)),
          .uniform_counters = std::as_writable_bytes(std::span(new_uniform_counters)),
      },
      {
          .counters = as_falsely_writable(std::span(kOldCounters)),
          .bitmap = as_falsely_writable(std::span(kOldBitmap)),
          .uniform_counters = as_falsely_writable(std::span(kOldUniformCounters)),
      });

  EXPECT_EQ(new_counters[0], 6u);
  EXPECT_EQ(new_counters[1], 8u);
  EXPECT_EQ(new_counters[2], 10u);
  EXPECT_EQ(new_counters[3], 12u);

  EXPECT_EQ(new_bitmap[0], 0x01u);
  EXPECT_EQ(new_bitmap[1], 0x11u);
  EXPECT_EQ(new_bitmap[2], 0x22u);
  EXPECT_EQ(new_bitmap[3], 0x33u);

#if INSTR_PROF_RAW_VERSION >= 11
  EXPECT_EQ(new_uniform_counters[0], 40u);
  EXPECT_EQ(new_uniform_counters[1], 60u);
#else
  EXPECT_EQ(new_uniform_counters[0], 30u);
  EXPECT_EQ(new_uniform_counters[1], 40u);
#endif

  LlvmProfdata data;
  data.Init(MyBuildId());

  const size_t buffer_size = data.size_bytes();
  ASSERT_GT(buffer_size, size_t{0});
  std::unique_ptr<std::byte[]> buffer(new std::byte[buffer_size]);
  const std::span buffer_span(buffer.get(), buffer_size);

  auto live_data = data.WriteFixedData(buffer_span);
  ASSERT_FALSE(live_data.counters.empty());

  const std::span<uint64_t> counters{
      reinterpret_cast<uint64_t*>(live_data.counters.data()),
      live_data.counters.size_bytes() / sizeof(uint64_t),
  };

  // In case the normal profile runtime is also active, reset the bias.
  LlvmProfdata::UseLinkTimeLiveData();

  // Run some instrumented code that should be sure to touch a counter.
  RunTimeCoveredFunction();

  // Set initial values for each counter in our buffer.
  for (size_t i = 0; i < counters.size(); ++i) {
    counters[i] = i;
  }

  // Now merge the current data into our synthetic starting data.
  data.MergeLiveData({
      .counters = std::as_writable_bytes(counters),
      .bitmap = live_data.bitmap,
      .uniform_counters = live_data.uniform_counters,
  });

  uint64_t increase = 0;
  for (size_t i = 0; i < counters.size(); ++i) {
    // No counter should have decreased.
    EXPECT_GE(counters[i], i);

    // Accumulate all the increased hit counts together.
    increase += counters[i] - i;
  }

  // At least one counter in RunTimeCoveredFunction should have increased.
  EXPECT_GT(increase, uint64_t{0});
}

TEST(LlvmProfdataTests, MergeLiveDataSingleByteCounters) {
  if (!LlvmProfdata::UsingSingleByteCounters())
    GTEST_SKIP() << "Not supported in eight byte counters mode";

  static constexpr uint8_t kOldCounters[] = {0, 0, 0, 0};
  uint8_t new_counters[] = {0, 0, 0, 0};
  static_assert(std::size(kOldCounters) == std::size(new_counters));

  static constexpr uint8_t kOldBitmap[] = {0, 0x01, 0x02, 0x03};
  uint8_t new_bitmap[] = {1, 0x11, 0x20, 0x31};

  constexpr auto as_falsely_writable = [](auto span) {
    std::span<const std::byte> bytes = std::as_bytes(span);
    return std::span<std::byte>{
        const_cast<std::byte*>(bytes.data()),
        bytes.size(),
    };
  };

  LlvmProfdata::MergeLiveData(
      {
          .counters = std::as_writable_bytes(std::span(new_counters)),
          .bitmap = std::as_writable_bytes(std::span(new_bitmap)),
      },
      {
          .counters = as_falsely_writable(std::span(kOldCounters)),
          .bitmap = as_falsely_writable(std::span(kOldBitmap)),
      });

  EXPECT_EQ(new_counters[0], uint8_t{0});
  EXPECT_EQ(new_counters[1], uint8_t{0});
  EXPECT_EQ(new_counters[2], uint8_t{0});
  EXPECT_EQ(new_counters[3], uint8_t{0});

  EXPECT_EQ(new_bitmap[0], 0x01u);
  EXPECT_EQ(new_bitmap[1], 0x11u);
  EXPECT_EQ(new_bitmap[2], 0x22u);
  EXPECT_EQ(new_bitmap[3], 0x33u);

  LlvmProfdata data;
  data.Init(MyBuildId());

  const size_t buffer_size = data.size_bytes();
  ASSERT_GT(buffer_size, size_t{0});
  std::unique_ptr<std::byte[]> buffer(new std::byte[buffer_size]);
  const std::span buffer_span(buffer.get(), buffer_size);

  auto live_data = data.WriteFixedData(buffer_span);
  ASSERT_FALSE(live_data.counters.empty());

  const std::span<uint8_t> counters{
      reinterpret_cast<uint8_t*>(live_data.counters.data()),
      live_data.counters.size_bytes() / sizeof(uint8_t),
  };

  // In case the normal profile runtime is also active, reset the bias.
  LlvmProfdata::UseLinkTimeLiveData();

  // Run some instrumented code that should be sure to touch a counter.
  RunTimeCoveredFunction();

  // Set initial values for each counter in our buffer.
  for (size_t i = 0; i < counters.size(); ++i) {
    counters[i] = 1;
  }

  // Now merge the current data into our synthetic starting data.
  data.MergeLiveData({
      .counters = std::as_writable_bytes(counters),
      .bitmap = live_data.bitmap,
      .uniform_counters = live_data.uniform_counters,
  });

  uint8_t covered = 1;
  for (size_t i = 0; i < counters.size(); ++i) {
    // Accumulate all the covered hit counts together.
    covered &= counters[i];
  }

  // At least one counter in RunTimeCoveredFunction should have covered.
  EXPECT_NE(covered, uint8_t{1});
}

TEST(LlvmProfdataTests, UseLiveData) {
  if (LlvmProfdata::UsingSingleByteCounters())
    GTEST_SKIP() << "Not supported in single byte counters mode";

  LlvmProfdata data;
  data.Init(MyBuildId());

  const size_t buffer_size = data.size_bytes();
  ASSERT_GT(buffer_size, size_t{0});
  std::unique_ptr<std::byte[]> buffer(new std::byte[buffer_size]);
  const std::span buffer_span(buffer.get(), buffer_size);

  auto live_data = data.WriteFixedData(buffer_span);
  ASSERT_FALSE(live_data.counters.empty());

  const std::span<uint64_t> counters{
      reinterpret_cast<uint64_t*>(live_data.counters.data()),
      live_data.counters.size_bytes() / sizeof(uint64_t),
  };

  // Start all counters at zero.
  std::fill(counters.begin(), counters.end(), 0);

  if (kRelocatableCounters) {
    LlvmProfdata::UseLiveData(live_data);

    // Now run some instrumented code that should be sure to touch a counter.
    RunTimeCoveredFunction();

    // Go back to writing into the statically-allocated data.  Note that if the
    // normal profile runtime is enabled and using relocatable mode (as it
    // always does on Fuchsia), this will skew down the coverage numbers for
    // this test code itself.
    LlvmProfdata::UseLinkTimeLiveData();

    uint64_t hits = 0;
    for (uint64_t count : counters) {
      hits += count;
    }

    // At least one counter in RunTimeCoveredFunction should have increased.
    EXPECT_GT(hits, uint64_t{0});
  }
}

TEST(LlvmProfdataTests, UseLiveDataSingleByteCounters) {
  if (!LlvmProfdata::UsingSingleByteCounters())
    GTEST_SKIP() << "Not supported in eight byte counters mode";

  LlvmProfdata data;
  data.Init(MyBuildId());

  const size_t buffer_size = data.size_bytes();
  ASSERT_GT(buffer_size, size_t{0});
  std::unique_ptr<std::byte[]> buffer(new std::byte[buffer_size]);
  const std::span buffer_span(buffer.get(), buffer_size);

  auto live_data = data.WriteFixedData(buffer_span);
  ASSERT_FALSE(live_data.counters.empty());

  const std::span<uint8_t> counters{
      reinterpret_cast<uint8_t*>(live_data.counters.data()),
      live_data.counters.size_bytes() / sizeof(uint8_t),
  };

  // Start all counters at one.
  // A value of zero means the function is covered.
  // A value of 0xff means the function is not covered.
  std::fill(counters.begin(), counters.end(), 1);

  if (kRelocatableCounters) {
    LlvmProfdata::UseLiveData(live_data);

    // Now run some instrumented code that should be sure to touch a counter.
    RunTimeCoveredFunction();

    // Go back to writing into the statically-allocated data.  Note that if the
    // normal profile runtime is enabled and using relocatable mode (as it
    // always does on Fuchsia), this will skew down the coverage numbers for
    // this test code itself.
    LlvmProfdata::UseLinkTimeLiveData();

    uint8_t hits = 0;
    for (uint8_t count : counters) {
      hits &= count;
    }

    // At least one counter in RunTimeCoveredFunction should have covered.
    EXPECT_NE(hits, uint8_t{1});
  }
}

}  // namespace
