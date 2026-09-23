// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/client-priority.h"

#include <fidl/fuchsia.hardware.display/cpp/wire.h>

#include <format>

#include <gtest/gtest.h>

namespace display {

namespace {

constexpr ClientPriority kCompositor2(
    fuchsia_hardware_display::wire::kCompositorClientPriorityValue);

TEST(ClientPriorityTest, EqualityIsReflexive) {
  EXPECT_EQ(ClientPriority::kCompositor, ClientPriority::kCompositor);
  EXPECT_EQ(kCompositor2, kCompositor2);
  EXPECT_EQ(ClientPriority::kVirtcon, ClientPriority::kVirtcon);
}

TEST(ClientPriorityTest, EqualityIsSymmetric) {
  EXPECT_EQ(ClientPriority::kCompositor, kCompositor2);
  EXPECT_EQ(kCompositor2, ClientPriority::kCompositor);
}

TEST(ClientPriorityTest, EqualityForDifferentValues) {
  EXPECT_NE(ClientPriority::kCompositor, ClientPriority::kVirtcon);
  EXPECT_NE(ClientPriority::kVirtcon, ClientPriority::kCompositor);
  EXPECT_NE(kCompositor2, ClientPriority::kVirtcon);
  EXPECT_NE(ClientPriority::kVirtcon, kCompositor2);
}

TEST(ClientPriorityTest, Ordering) {
  static_assert(fuchsia_hardware_display::wire::kCompositorClientPriorityValue >
                fuchsia_hardware_display::wire::kVirtconClientPriorityValue);

  EXPECT_GT(ClientPriority::kCompositor, ClientPriority::kVirtcon);
  EXPECT_FALSE(ClientPriority::kCompositor > ClientPriority::kCompositor);
  EXPECT_FALSE(ClientPriority::kVirtcon > ClientPriority::kCompositor);

  EXPECT_GE(ClientPriority::kCompositor, ClientPriority::kVirtcon);
  EXPECT_GE(ClientPriority::kCompositor, ClientPriority::kCompositor);
  EXPECT_FALSE(ClientPriority::kVirtcon >= ClientPriority::kCompositor);

  EXPECT_LT(ClientPriority::kVirtcon, ClientPriority::kCompositor);
  EXPECT_FALSE(ClientPriority::kVirtcon < ClientPriority::kVirtcon);
  EXPECT_FALSE(ClientPriority::kCompositor < ClientPriority::kVirtcon);

  EXPECT_LE(ClientPriority::kVirtcon, ClientPriority::kCompositor);
  EXPECT_LE(ClientPriority::kCompositor, ClientPriority::kCompositor);
  EXPECT_FALSE(ClientPriority::kCompositor <= ClientPriority::kVirtcon);
}

TEST(ClientPriorityTest, ToFidlClientPriority) {
  EXPECT_EQ(fuchsia_hardware_display::wire::kCompositorClientPriorityValue,
            ClientPriority::kCompositor.ToFidl().value);
  EXPECT_EQ(fuchsia_hardware_display::wire::kVirtconClientPriorityValue,
            ClientPriority::kVirtcon.ToFidl().value);
}

TEST(ClientPriorityTest, ToFidlValue) {
  EXPECT_EQ(fuchsia_hardware_display::wire::kCompositorClientPriorityValue,
            ClientPriority::kCompositor.ToFidlValue());
  EXPECT_EQ(fuchsia_hardware_display::wire::kVirtconClientPriorityValue,
            ClientPriority::kVirtcon.ToFidlValue());
}

TEST(ClientPriorityTest, ToClientPriorityWithFidlValue) {
  EXPECT_EQ(ClientPriority::kCompositor,
            ClientPriority(fuchsia_hardware_display::wire::kCompositorClientPriorityValue));
  EXPECT_EQ(ClientPriority::kVirtcon,
            ClientPriority(fuchsia_hardware_display::wire::kVirtconClientPriorityValue));
}

TEST(ClientPriorityTest, ValueForLogging) {
  EXPECT_EQ(static_cast<uint32_t>(fuchsia_hardware_display::wire::kCompositorClientPriorityValue),
            ClientPriority::kCompositor.ValueForLogging());
  EXPECT_EQ(static_cast<uint32_t>(fuchsia_hardware_display::wire::kVirtconClientPriorityValue),
            ClientPriority::kVirtcon.ValueForLogging());
}

TEST(ClientPriorityTest, FidlConversionRoundtrip) {
  EXPECT_EQ(ClientPriority::kCompositor,
            ClientPriority(ClientPriority::kCompositor.ToFidl().value));
  EXPECT_EQ(ClientPriority::kVirtcon, ClientPriority(ClientPriority::kVirtcon.ToFidl().value));

  EXPECT_EQ(ClientPriority::kCompositor, ClientPriority(ClientPriority::kCompositor.ToFidlValue()));
  EXPECT_EQ(ClientPriority::kVirtcon, ClientPriority(ClientPriority::kVirtcon.ToFidlValue()));
}

TEST(ClientPriority, Format) {
  EXPECT_EQ(std::format("{}", ClientPriority::kCompositor),
            std::format("{}", fuchsia_hardware_display::wire::kCompositorClientPriorityValue));
  EXPECT_EQ(std::format("{}", ClientPriority::kVirtcon),
            std::format("{}", fuchsia_hardware_display::wire::kVirtconClientPriorityValue));
  EXPECT_EQ(std::format("{:>2}", ClientPriority::kCompositor),
            std::format("{:>2}", fuchsia_hardware_display::wire::kCompositorClientPriorityValue));
}

}  // namespace

}  // namespace display
