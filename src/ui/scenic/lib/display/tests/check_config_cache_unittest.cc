// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/internal/check_config_cache.h"

#include <gtest/gtest.h>

namespace display::internal::test {
namespace {

using IntCache = BoundedLruCache<int, int>;
using StringCache = BoundedLruCache<std::string, int>;

constexpr types::Point2 kOrigin({.x = 0, .y = 0});
constexpr int32_t kDisplayWidth = 1280;
constexpr int32_t kDisplayHeight = 1024;
constexpr Extent2 kDisplayExtent({.width = kDisplayWidth, .height = kDisplayHeight});
constexpr Extent2 kHalfDisplayExtent({.width = kDisplayWidth / 2, .height = kDisplayHeight / 2});

TEST(BoundedLruCache, Put) {
  IntCache cache(2);

  cache.Put(1, 11);
  cache.Put(2, 22);
  EXPECT_EQ(cache.size(), 2U);

  cache.Put(3, 33);
  // `1` was evicted.
  EXPECT_EQ(cache.size(), 2U);
  EXPECT_FALSE(cache.Get(1).has_value());
  ASSERT_TRUE(cache.Get(2).has_value());
  EXPECT_EQ(cache.Get(2).value(), 22);
  ASSERT_TRUE(cache.Get(3).has_value());
  EXPECT_EQ(cache.Get(3).value(), 33);
}

TEST(BoundedLruCache, LruEviction) {
  // Least-recent put is evicted.
  {
    IntCache cache(2);

    cache.Put(1, 11);
    cache.Put(2, 22);
    cache.Put(3, 33);

    EXPECT_EQ(cache.size(), 2U);
    EXPECT_FALSE(cache.Get(1).has_value());
    EXPECT_TRUE(cache.Get(2).has_value());
    EXPECT_TRUE(cache.Get(3).has_value());
  }

  // Getting `1` just before it would be evicted makes it the most-recently used.
  {
    IntCache cache(2);

    cache.Put(1, 11);
    cache.Put(2, 22);

    ASSERT_TRUE(cache.Get(1).has_value());
    cache.Put(3, 33);

    // Now `2` is evicted before `1`.
    EXPECT_EQ(cache.size(), 2U);
    EXPECT_TRUE(cache.Get(1).has_value());
    EXPECT_FALSE(cache.Get(2).has_value());
    EXPECT_TRUE(cache.Get(3).has_value());
  }
}

TEST(BoundedLruCache, HeterogeneousAccess) {
  StringCache cache(1);

  const std::string kOneString("one");
  const char* kOneChars("one");
  const std::string kTwoString("two");
  const char* kTwoChars("two");
  const std::string kThreeString("three");
  const char* kThreeChars("three");

  cache.Put(kOneString, 1);
  auto result = cache.Get(kOneChars);
  EXPECT_TRUE(result.has_value());
  EXPECT_EQ(result.value(), 1);

  cache.Put(kOneChars, 11);
  result = cache.Get(kOneString);
  EXPECT_TRUE(result.has_value());
  EXPECT_EQ(result.value(), 11);
}

TEST(BoundedLruCache, Replacement) {
  IntCache cache(3);

  cache.Put(1, 1);
  cache.Put(2, 2);
  cache.Put(3, 3);
  cache.Put(1, 11);  // replacing makes it the most recently used (see below)
  cache.Put(4, 4);
  cache.Put(5, 5);
  EXPECT_FALSE(cache.Get(2).has_value());  // was evicted by (4,4)
  EXPECT_FALSE(cache.Get(3).has_value());  // was evicted by (5,5)

  // Verify that setting the value to 11:
  // - prevented it from being evicted
  // - properly set the value
  ASSERT_TRUE(cache.Get(1).has_value());
  EXPECT_EQ(cache.Get(1).value(), 11);
}

TEST(BoundedLruCache, StressTest) {
  IntCache cache(3);

  cache.Put(1, 1);
  EXPECT_EQ(cache.size(), 1U);
  ASSERT_TRUE(cache.Get(1).has_value());
  EXPECT_EQ(cache.Get(1).value(), 1);
  EXPECT_FALSE(cache.Get(2).has_value());   // not yet added
  EXPECT_FALSE(cache.Get(12).has_value());  // not yet added

  cache.Put(2, 2);
  EXPECT_EQ(cache.size(), 2U);
  ASSERT_TRUE(cache.Get(2).has_value());
  EXPECT_EQ(cache.Get(2).value(), 2);
  EXPECT_FALSE(cache.Get(12).has_value());  // not yet added

  cache.Put(12, 12);
  EXPECT_EQ(cache.size(), 3U);
  ASSERT_TRUE(cache.Get(12).has_value());
  EXPECT_EQ(cache.Get(12).value(), 12);
  EXPECT_FALSE(cache.Get(21).has_value());  // not yet added

  cache.Put(21, 21);
  EXPECT_EQ(cache.size(), 3U);
  ASSERT_TRUE(cache.Get(21).has_value());
  EXPECT_EQ(cache.Get(21).value(), 21);
  EXPECT_FALSE(cache.Get(1).has_value());  // evicted

  cache.Put(123, 123);
  ASSERT_TRUE(cache.Get(123).has_value());
  EXPECT_EQ(cache.Get(123).value(), 123);
  EXPECT_FALSE(cache.Get(2).has_value());  // evicted
  EXPECT_TRUE(cache.Get(12).has_value());  // not evicted
  EXPECT_TRUE(cache.Get(21).has_value());  // not evicted

  // Because the last ones we checked were `12` and `21`, `123` is now the LRU.
  cache.Put(321, 321);
  ASSERT_TRUE(cache.Get(321).has_value());
  EXPECT_EQ(cache.Get(321).value(), 321);
  // Go through all expected state.
  EXPECT_FALSE(cache.Get(1).has_value());    // evicted, as we knew
  EXPECT_FALSE(cache.Get(2).has_value());    // evicted, as we knew
  EXPECT_TRUE(cache.Get(12).has_value());    // not evicted
  EXPECT_TRUE(cache.Get(21).has_value());    // not evicted
  EXPECT_FALSE(cache.Get(123).has_value());  // evicted by most recent put
}

// This doesn't test anything that shouldn't already be covered by the combination of:
//   - the `BoundedLruCache` tests above
//   - `display_equivalence_unittest.cc`
// .. but it's still good to know there's some direct coverage of `CheckConfigCache`.
TEST(CheckConfigCache, SmokeTest) {
  // No layers.
  constexpr DisplayEquivalence empty_display{
      .display_mode = types::DisplayMode::From(WireDisplayMode{
          .active_area = {.width = 456, .height = 456}, .refresh_rate_millihertz = 60'000})};

  constexpr LayerEquivalence layer1{
      ImageLayerEquivalence{.display_destination = Rectangle::From(kOrigin, kDisplayExtent),
                            .image_source = Rectangle::From(kOrigin, kDisplayExtent),
                            .image_dimensions = kDisplayExtent,
                            .blend_mode = BlendMode::kPremultipliedAlpha()}};

  constexpr LayerEquivalence layer2{
      ImageLayerEquivalence{.display_destination = Rectangle::From(kOrigin, kHalfDisplayExtent),
                            .image_source = Rectangle::From(kOrigin, kHalfDisplayExtent),
                            .image_dimensions = kDisplayExtent,
                            .blend_mode = BlendMode::kPremultipliedAlpha()}};

  constexpr LayerEquivalence layer3{
      ColorLayerEquivalence{.display_destination = Rectangle::From(kOrigin, kHalfDisplayExtent)}};

  // The names of the `display*` variables reflect the layers they contain, and in what order.
  const DisplayEquivalence display1{.layers = {layer1}, .display_mode = empty_display.display_mode};
  const DisplayEquivalence display2{.layers = {layer2}, .display_mode = empty_display.display_mode};
  const DisplayEquivalence display3{.layers = {layer3}, .display_mode = empty_display.display_mode};
  const DisplayEquivalence display12{.layers = {layer1, layer2},
                                     .display_mode = empty_display.display_mode};
  const DisplayEquivalence display21{.layers = {layer2, layer1},
                                     .display_mode = empty_display.display_mode};
  const DisplayEquivalence display123{.layers = {layer1, layer2, layer3},
                                      .display_mode = empty_display.display_mode};
  const DisplayEquivalence display321{.layers = {layer3, layer2, layer1},
                                      .display_mode = empty_display.display_mode};

  CheckConfigCache cache(3);
  cache.Put(display1, true);
  EXPECT_EQ(cache.size(), 1U);
  ASSERT_TRUE(cache.Get(display1).has_value());
  EXPECT_EQ(cache.Get(display1).value(), true);
  EXPECT_FALSE(cache.Get(display2).has_value());   // not yet added
  EXPECT_FALSE(cache.Get(display12).has_value());  // not yet added

  cache.Put(display2, false);
  EXPECT_EQ(cache.size(), 2U);
  ASSERT_TRUE(cache.Get(display2).has_value());
  EXPECT_EQ(cache.Get(display2).value(), false);
  EXPECT_FALSE(cache.Get(display12).has_value());  // not yet added

  cache.Put(display12, true);
  EXPECT_EQ(cache.size(), 3U);
  ASSERT_TRUE(cache.Get(display12).has_value());
  EXPECT_EQ(cache.Get(display12).value(), true);
  EXPECT_FALSE(cache.Get(display21).has_value());  // not yet added

  cache.Put(display21, false);
  EXPECT_EQ(cache.size(), 3U);
  ASSERT_TRUE(cache.Get(display21).has_value());
  EXPECT_EQ(cache.Get(display21).value(), false);
  EXPECT_FALSE(cache.Get(display1).has_value());  // evicted

  cache.Put(display123, true);
  ASSERT_TRUE(cache.Get(display123).has_value());
  EXPECT_EQ(cache.Get(display123).value(), true);
  EXPECT_FALSE(cache.Get(display2).has_value());  // evicted
  EXPECT_TRUE(cache.Get(display12).has_value());  // not evicted
  EXPECT_TRUE(cache.Get(display21).has_value());  // not evicted

  // Because the last ones we checked were `display12` and `display21`, `display123` is now the LRU.
  cache.Put(display321, false);
  ASSERT_TRUE(cache.Get(display321).has_value());
  EXPECT_EQ(cache.Get(display321).value(), false);
  // Go through all expected state.
  EXPECT_FALSE(cache.Get(display1).has_value());    // evicted, as we knew
  EXPECT_FALSE(cache.Get(display2).has_value());    // evicted, as we knew
  EXPECT_TRUE(cache.Get(display12).has_value());    // not evicted
  EXPECT_TRUE(cache.Get(display21).has_value());    // not evicted
  EXPECT_FALSE(cache.Get(display123).has_value());  // evicted by most recent put
}

}  // namespace
}  // namespace display::internal::test
