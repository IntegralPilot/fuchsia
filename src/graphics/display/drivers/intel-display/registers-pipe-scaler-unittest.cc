// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/registers-pipe-scaler.h"

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mock-mmio/cpp/globally-ordered-region.h>

#include <cmath>
#include <limits>

#include <gtest/gtest.h>
#include <hwreg/bitfields.h>

#include "src/graphics/display/drivers/intel-display/hardware-common.h"

namespace intel_display {

namespace {

TEST(PipeScalerControlTigerLake, SetYPlaneBinding) {
  // Valid values
  EXPECT_NO_FATAL_FAILURE({
    registers::PipeScalerControlTigerLake pipe_scaler_control;
    pipe_scaler_control.set_y_plane_binding(6);
  });
  EXPECT_NO_FATAL_FAILURE({
    registers::PipeScalerControlTigerLake pipe_scaler_control;
    pipe_scaler_control.set_y_plane_binding(7);
  });
  // Invalid values
  EXPECT_DEATH_IF_SUPPORTED(
      {
        registers::PipeScalerControlTigerLake pipe_scaler_control;
        pipe_scaler_control.set_y_plane_binding(0);
      },
      "non-Y plane");
  EXPECT_DEATH_IF_SUPPORTED(
      {
        registers::PipeScalerControlTigerLake pipe_scaler_control;
        pipe_scaler_control.set_y_plane_binding(1);
      },
      "non-Y plane");
  EXPECT_DEATH_IF_SUPPORTED(
      {
        registers::PipeScalerControlTigerLake pipe_scaler_control;
        pipe_scaler_control.set_y_plane_binding(2);
      },
      "non-Y plane");
  EXPECT_DEATH_IF_SUPPORTED(
      {
        registers::PipeScalerControlTigerLake pipe_scaler_control;
        pipe_scaler_control.set_y_plane_binding(3);
      },
      "non-Y plane");
  EXPECT_DEATH_IF_SUPPORTED(
      {
        registers::PipeScalerControlTigerLake pipe_scaler_control;
        pipe_scaler_control.set_y_plane_binding(4);
      },
      "non-Y plane");
  EXPECT_DEATH_IF_SUPPORTED(
      {
        registers::PipeScalerControlTigerLake pipe_scaler_control;
        pipe_scaler_control.set_y_plane_binding(5);
      },
      "non-Y plane");
}

TEST(PipeScalerScalingFactor, ToFloat) {
  // All the comparisons must be exact equal, since ToFloat() is not
  // supposed to lose any precision when using C float type.

  // Integer part
  {
    registers::PipeScalerScalingFactor zero;
    zero.set_integer_part(0).set_fraction_part(0);
    EXPECT_EQ(zero.ToFloat(), 0.0f);
  }

  {
    registers::PipeScalerScalingFactor five;
    five.set_integer_part(5).set_fraction_part(0);
    EXPECT_EQ(five.ToFloat(), 5.0f);
  }

  {
    registers::PipeScalerScalingFactor seven;
    seven.set_integer_part(7).set_fraction_part(0);
    EXPECT_EQ(seven.ToFloat(), 7.0f);
  }

  // Fraction part
  {
    registers::PipeScalerScalingFactor decimal_0_25;
    decimal_0_25.set_integer_part(0).set_fraction_part(0b010'000'000'000'000);
    EXPECT_EQ(decimal_0_25.ToFloat(), 0.25f);
  }

  {
    registers::PipeScalerScalingFactor min_positive_decimal;
    min_positive_decimal.set_integer_part(0).set_fraction_part(0b000'000'000'000'001);
    EXPECT_EQ(min_positive_decimal.ToFloat(), 1.0f / 32768.0f);
  }

  {
    registers::PipeScalerScalingFactor max_decimal_less_than_1;
    max_decimal_less_than_1.set_integer_part(0).set_fraction_part(0b111'111'111'111'111);
    EXPECT_EQ(max_decimal_less_than_1.ToFloat(), 32767.0f / 32768.0f);
  }

  {
    registers::PipeScalerScalingFactor max_decimal;
    max_decimal.set_integer_part(0b111).set_fraction_part(0b111'111'111'111'111);
    EXPECT_EQ(max_decimal.ToFloat(), 7.0f + 32767.0f / 32768.0f);
  }
}

TEST(PipeScalerInitialPhase, HorizontalPhaseUvValidity) {
  // Valid values should be between -0.5 and 1.5 (inclusive on both ends).
  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(0.25f));
  }

  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(-0.5f));
  }

  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(0.0f));
  }

  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(0.5f));
  }

  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(1.0f));
  }

  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(1.5f));
  }

  // Values out of range, NaN and infinite values are invalid.
  {
    registers::PipeScalerHorizontalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetUvInitialPhase(std::numeric_limits<float>::quiet_NaN()), "");
  }

  {
    registers::PipeScalerHorizontalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetUvInitialPhase(std::numeric_limits<float>::infinity()), "");
  }

  {
    registers::PipeScalerHorizontalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetUvInitialPhase(-std::numeric_limits<float>::infinity()), "");
  }

  {
    registers::PipeScalerHorizontalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(invalid_phase.SetUvInitialPhase(-0.6f), "");
  }

  {
    registers::PipeScalerHorizontalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(invalid_phase.SetUvInitialPhase(1.6f), "");
  }
}

TEST(PipeScalerInitialPhase, HorizontalPhaseYValidity) {
  // Valid values should be between -0.5 and 1.5 (inclusive on both ends).
  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(0.25f));
  }

  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(-0.5f));
  }

  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(0.0f));
  }

  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(0.5f));
  }

  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(1.0f));
  }

  {
    registers::PipeScalerHorizontalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(1.5f));
  }

  // Values out of range, NaN and infinite values are invalid.
  {
    registers::PipeScalerHorizontalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetYInitialPhase(std::numeric_limits<float>::quiet_NaN()), "");
  }

  {
    registers::PipeScalerHorizontalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetYInitialPhase(std::numeric_limits<float>::infinity()), "");
  }

  {
    registers::PipeScalerHorizontalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetYInitialPhase(-std::numeric_limits<float>::infinity()), "");
  }

  {
    registers::PipeScalerHorizontalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(invalid_phase.SetYInitialPhase(-0.6f), "");
  }

  {
    registers::PipeScalerHorizontalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(invalid_phase.SetYInitialPhase(1.6f), "");
  }
}

TEST(PipeScalerInitialPhase, HorizontalPhaseValue) {
  // YUV 420 Chroma Siting = Top Left
  // Horizontal Phase = 0.25
  {
    registers::PipeScalerHorizontalInitialPhase phase_top_left;
    phase_top_left.SetUvInitialPhase(0.25f);
    EXPECT_EQ(phase_top_left.uv_initial_phase_fraction(), 1u << 11);
    EXPECT_EQ(phase_top_left.uv_initial_phase_int(), 0u);
    EXPECT_EQ(phase_top_left.uv_initial_phase_trip_enabled(), true);

    phase_top_left.SetYInitialPhase(0.25f);
    EXPECT_EQ(phase_top_left.y_initial_phase_fraction(), 1u << 11);
    EXPECT_EQ(phase_top_left.y_initial_phase_int(), 0u);
    EXPECT_EQ(phase_top_left.y_initial_phase_trip_enabled(), true);
  }

  // YUV 420 Chroma Siting = Bottom Right
  // Horizontal Phase = -0.25
  {
    registers::PipeScalerHorizontalInitialPhase phase_bottom_right;
    phase_bottom_right.SetUvInitialPhase(-0.25f);
    EXPECT_EQ(phase_bottom_right.uv_initial_phase_fraction(), 3u << 11);
    EXPECT_EQ(phase_bottom_right.uv_initial_phase_int(), 0u);
    EXPECT_EQ(phase_bottom_right.uv_initial_phase_trip_enabled(), false);

    phase_bottom_right.SetYInitialPhase(-0.25f);
    EXPECT_EQ(phase_bottom_right.y_initial_phase_fraction(), 3u << 11);
    EXPECT_EQ(phase_bottom_right.y_initial_phase_int(), 0u);
    EXPECT_EQ(phase_bottom_right.y_initial_phase_trip_enabled(), false);
  }

  // YUV 420 Chroma Siting = Bottom Center
  // Horizontal Phase = 0
  {
    registers::PipeScalerHorizontalInitialPhase phase_bottom_center;
    phase_bottom_center.SetUvInitialPhase(0.0f);
    EXPECT_EQ(phase_bottom_center.uv_initial_phase_fraction(), 0u);
    EXPECT_EQ(phase_bottom_center.uv_initial_phase_int(), 0u);
    EXPECT_EQ(phase_bottom_center.uv_initial_phase_trip_enabled(), true);

    phase_bottom_center.SetYInitialPhase(0.0f);
    EXPECT_EQ(phase_bottom_center.y_initial_phase_fraction(), 0u);
    EXPECT_EQ(phase_bottom_center.y_initial_phase_int(), 0u);
    EXPECT_EQ(phase_bottom_center.y_initial_phase_trip_enabled(), true);
  }
}

TEST(PipeScalerInitialPhase, VerticalPhaseUvValidity) {
  // Valid values should be between -0.5 and 1.5 (inclusive on both ends).
  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(0.25f));
  }

  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(-0.5f));
  }

  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(0.0f));
  }

  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(0.5f));
  }

  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(1.0f));
  }

  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetUvInitialPhase(1.5f));
  }

  // Values out of range, NaN and infinite values are invalid.
  {
    registers::PipeScalerVerticalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetUvInitialPhase(std::numeric_limits<float>::quiet_NaN()), "");
  }

  {
    registers::PipeScalerVerticalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetUvInitialPhase(std::numeric_limits<float>::infinity()), "");
  }

  {
    registers::PipeScalerVerticalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetUvInitialPhase(-std::numeric_limits<float>::infinity()), "");
  }

  {
    registers::PipeScalerVerticalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(invalid_phase.SetUvInitialPhase(-0.6f), "");
  }

  {
    registers::PipeScalerVerticalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(invalid_phase.SetUvInitialPhase(1.6f), "");
  }
}

TEST(PipeScalerInitialPhase, VerticalPhaseYValidity) {
  // Valid values should be between -0.5 and 1.5 (inclusive on both ends).
  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(0.25f));
  }

  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(-0.5f));
  }

  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(0.0f));
  }

  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(0.5f));
  }

  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(1.0f));
  }

  {
    registers::PipeScalerVerticalInitialPhase valid_phase;
    EXPECT_NO_FATAL_FAILURE(valid_phase.SetYInitialPhase(1.5f));
  }

  // Values out of range, NaN and infinite values are invalid.
  {
    registers::PipeScalerVerticalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetYInitialPhase(std::numeric_limits<float>::quiet_NaN()), "");
  }

  {
    registers::PipeScalerVerticalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetYInitialPhase(std::numeric_limits<float>::infinity()), "");
  }

  {
    registers::PipeScalerVerticalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(
        invalid_phase.SetYInitialPhase(-std::numeric_limits<float>::infinity()), "");
  }

  {
    registers::PipeScalerVerticalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(invalid_phase.SetYInitialPhase(-0.6f), "");
  }

  {
    registers::PipeScalerVerticalInitialPhase invalid_phase;
    EXPECT_DEATH_IF_SUPPORTED(invalid_phase.SetYInitialPhase(1.6f), "");
  }
}

TEST(PipeScalerInitialPhase, VerticalPhaseValue) {
  // YUV 420 Chroma Siting = Top Left
  // Vertical Phase = 0.25
  {
    registers::PipeScalerVerticalInitialPhase phase_top_left;
    phase_top_left.SetUvInitialPhase(0.25f);
    EXPECT_EQ(phase_top_left.uv_initial_phase_fraction(), 1u << 11);
    EXPECT_EQ(phase_top_left.uv_initial_phase_int(), 0u);
    EXPECT_EQ(phase_top_left.uv_initial_phase_trip_enabled(), true);

    phase_top_left.SetYInitialPhase(0.25f);
    EXPECT_EQ(phase_top_left.y_initial_phase_fraction(), 1u << 11);
    EXPECT_EQ(phase_top_left.y_initial_phase_int(), 0u);
    EXPECT_EQ(phase_top_left.y_initial_phase_trip_enabled(), true);
  }

  // YUV 420 Chroma Siting = Bottom Right / Bottom Center
  // Vertical Phase = -0.25
  {
    registers::PipeScalerVerticalInitialPhase phase_bottom_right;
    phase_bottom_right.SetUvInitialPhase(-0.25f);
    EXPECT_EQ(phase_bottom_right.uv_initial_phase_fraction(), 3u << 11);
    EXPECT_EQ(phase_bottom_right.uv_initial_phase_int(), 0u);
    EXPECT_EQ(phase_bottom_right.uv_initial_phase_trip_enabled(), false);

    phase_bottom_right.SetYInitialPhase(-0.25f);
    EXPECT_EQ(phase_bottom_right.y_initial_phase_fraction(), 3u << 11);
    EXPECT_EQ(phase_bottom_right.y_initial_phase_int(), 0u);
    EXPECT_EQ(phase_bottom_right.y_initial_phase_trip_enabled(), false);
  }
}

TEST(PipeScalerCoefficientDataFormat, ToCanonical) {
  {
    registers::PipeScalerCoefficientFormat zero;
    zero.set_is_negative(false).set_exponent(1).set_mantissa(0);
    EXPECT_EQ(zero.x2048(), 0);
  }

  {
    registers::PipeScalerCoefficientFormat one;
    one.set_is_negative(false).set_exponent(0).set_mantissa(0b100'000'000);
    EXPECT_EQ(one.x2048(), 2048);
  }

  {
    registers::PipeScalerCoefficientFormat minus_one;
    minus_one.set_is_negative(true).set_exponent(0).set_mantissa(0b100'000'000);
    EXPECT_EQ(minus_one.x2048(), -2048);
  }

  {
    registers::PipeScalerCoefficientFormat half;
    half.set_is_negative(false).set_exponent(1).set_mantissa(0b100'000'000);
    EXPECT_EQ(half.x2048(), 1024);
  }

  {
    // Minimum positive: (0.00000000001) = 1 / 2048
    registers::PipeScalerCoefficientFormat minimum_positive;
    minimum_positive.set_is_negative(false).set_exponent(3).set_mantissa(1);
    EXPECT_EQ(minimum_positive.x2048(), 1);
  }

  {
    // Maximum positive: (1.11111111)b = 511 / 256 = 4088 / 2048
    registers::PipeScalerCoefficientFormat minimum_positive;
    minimum_positive.set_is_negative(false).set_exponent(0).set_mantissa(0b111'111'111);
    EXPECT_EQ(minimum_positive.x2048(), 4088);
  }
}

TEST(PipeScalerCoefficients, Write) {
  constexpr uint16_t kNumCoefficients = 17 * 7;
  std::vector<registers::PipeScalerCoefficientFormat> coefficients(kNumCoefficients);
  for (uint16_t i = 0; i < kNumCoefficients; i++) {
    coefficients[i].set_is_negative(0).set_mantissa(i).set_exponent(0);
  }

  std::vector<mock_mmio::GloballyOrderedRegion::Access> expected_access_list = {
      {.address = 0x68198, .value = 0x00000400, .write = true},
  };
  for (size_t i = 0; i < kNumCoefficients; i += 2) {
    uint32_t value = 0u;
    if (i + 1 < kNumCoefficients) {
      value |= ((coefficients[i + 1].reg_value()) & 0xffffu) << 16;
    }
    value |= (coefficients[i].reg_value()) & 0xffffu;
    expected_access_list.push_back({.address = 0x6819C, .value = value, .write = true});
  }

  static constexpr int kMmioRangeSize = 0x140000;
  mock_mmio::GloballyOrderedRegion mmio_range{kMmioRangeSize,
                                              mock_mmio::GloballyOrderedRegion::Size::k32};
  mmio_range.Expect(expected_access_list);
  fdf::MmioBuffer mmio_buffer(mmio_range.GetMmioBuffer());

  registers::PipeScalerRegs pipe_scaler_regs(PipeId::PIPE_A, /* scaler_index= */ 0);
  auto pipe_scaler_coefficients = pipe_scaler_regs.PipeScalerCoefficients();

  pipe_scaler_coefficients.WriteToRegister(coefficients, &mmio_buffer);
}

TEST(PipeScalerCoefficients, Read) {
  constexpr uint16_t kNumCoefficients = 17 * 7;
  std::vector<registers::PipeScalerCoefficientFormat> expected_coefficients(kNumCoefficients);
  for (uint16_t i = 0; i < kNumCoefficients; i++) {
    expected_coefficients[i].set_is_negative(0).set_mantissa(i).set_exponent(0);
  }

  std::vector<mock_mmio::GloballyOrderedRegion::Access> expected_access_list = {
      {.address = 0x68198, .value = 0x00000400, .write = true},
  };
  for (size_t i = 0; i < kNumCoefficients; i += 2) {
    uint32_t value = 0u;
    if (i + 1 < kNumCoefficients) {
      value |= ((expected_coefficients[i + 1].reg_value()) & 0xffffu) << 16;
    }
    value |= (expected_coefficients[i].reg_value()) & 0xffffu;
    expected_access_list.push_back({.address = 0x6819C, .value = value});
  }

  static constexpr int kMmioRangeSize = 0x140000;
  mock_mmio::GloballyOrderedRegion mmio_range{kMmioRangeSize,
                                              mock_mmio::GloballyOrderedRegion::Size::k32};
  mmio_range.Expect(expected_access_list);
  fdf::MmioBuffer mmio_buffer(mmio_range.GetMmioBuffer());

  registers::PipeScalerRegs pipe_scaler_regs(PipeId::PIPE_A, /* scaler_index= */ 0);
  auto pipe_scaler_coefficients = pipe_scaler_regs.PipeScalerCoefficients();

  auto coefficients_read = pipe_scaler_coefficients.ReadFromRegister(&mmio_buffer);
  EXPECT_EQ(coefficients_read.size(), kNumCoefficients);
  for (size_t i = 0; i < kNumCoefficients; i++) {
    EXPECT_EQ(coefficients_read[i].reg_value(), expected_coefficients[i].reg_value());
  }
}

}  // namespace

}  // namespace intel_display
