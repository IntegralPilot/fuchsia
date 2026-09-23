// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/arm64/apple-aic.h>

#include <utility>
#include <vector>

#include <gtest/gtest.h>

namespace {
using arch::apple::Aic3Layout;
using arch::apple::Aic3State;

TEST(AppleAic, DiscoversSingleDieLayout) {
  auto layout = Aic3Layout::Create(1600, 4096, 0x184000);
  ASSERT_TRUE(layout);
  EXPECT_EQ(layout->num_irqs, 1600u);
  EXPECT_EQ(layout->mask_set, 0x14400u);
  EXPECT_EQ(layout->mask_clear, 0x14600u);
}

TEST(AppleAic, RejectsUnsupportedCountsAndTruncatedMmio) {
  EXPECT_FALSE(Aic3Layout::Create(0, 4096, 0x184000));
  EXPECT_FALSE(Aic3Layout::Create(1601, 4096, 0x184000));
  EXPECT_FALSE(Aic3Layout::Create(1600, 1024, 0x184000));
  EXPECT_FALSE(Aic3Layout::Create(1600, 4095, 0x184000));
  EXPECT_FALSE(Aic3Layout::Create(1600, 8192, 0x184000));
  EXPECT_FALSE(Aic3Layout::Create(0x01000640, 4096, 0x184000));
  EXPECT_FALSE(Aic3Layout::Create(1600, 4096, 0x147ff));
}

TEST(AppleAic, ValidatesEventMmioRange) {
  EXPECT_TRUE(Aic3Layout::ValidMmio(0x2d1000000, 0x184000, 0x40000));
  EXPECT_FALSE(Aic3Layout::ValidMmio(0x2d1000001, 0x184000, 0x40000));
  EXPECT_FALSE(Aic3Layout::ValidMmio(0x2d1000000, 0x184001, 0x40000));
  EXPECT_FALSE(Aic3Layout::ValidMmio(0x2d1000000, 0x184000, 0x40001));
  EXPECT_FALSE(Aic3Layout::ValidMmio(0x2d1000000, 0x184000, 0x184000));
  EXPECT_FALSE(Aic3Layout::ValidMmio(UINT64_MAX - 4095, 4096, 0));
}

TEST(AppleAic, DecodesHardwareEventsAndRejectsOtherDiesAndTypes) {
  auto layout = *Aic3Layout::Create(1600, 4096, 0x184000);
  EXPECT_EQ(layout.DecodeEvent(0x10000 | 757), 757u);
  EXPECT_EQ(layout.DecodeEvent(0x10000 | 1599), 1599u);
  EXPECT_FALSE(layout.DecodeEvent(0));
  EXPECT_FALSE(layout.DecodeEvent(0x10000 | 1600));
  EXPECT_FALSE(layout.DecodeEvent(0x1010000 | 757));
  EXPECT_FALSE(layout.DecodeEvent(0x20000 | 757));
}

struct MockIo {
  std::vector<std::pair<uint32_t, uint32_t>> writes;
  void Write(uint32_t offset, uint32_t value) { writes.emplace_back(offset, value); }
};

TEST(AppleAic, CompletionHonorsHandlerMaskAndMissingHandler) {
  Aic3State state(*Aic3Layout::Create(1600, 4096, 0x184000));
  MockIo io;
  state.SetMask(io, 31, false);
  state.Complete(io, 31, true);
  state.SetMask(io, 31, true);
  state.Complete(io, 31, true);
  state.SetMask(io, 32, false);
  state.Complete(io, 32, false);
  state.Complete(io, 32, true);
  EXPECT_EQ(
      io.writes,
      (std::vector<std::pair<uint32_t, uint32_t>>{
          {0x14600, 0x80000000}, {0x14600, 0x80000000}, {0x14400, 0x80000000}, {0x14604, 1}}));
}
}  // namespace
