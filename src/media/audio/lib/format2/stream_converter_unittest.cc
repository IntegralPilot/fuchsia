// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/lib/format2/stream_converter.h"

#include <limits>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/media/audio/lib/format2/sample_converter.h"

namespace media_audio {
namespace {

using fuchsia_audio::SampleType;
constexpr auto kUint8 = SampleType::kUint8;
constexpr auto kInt16 = SampleType::kInt16;
constexpr auto kInt32 = SampleType::kInt32;
constexpr auto kFloat32 = SampleType::kFloat32;

template <typename NumberType>
SampleType ToSampleType();

template <>
SampleType ToSampleType<uint8_t>() {
  return kUint8;
}
template <>
SampleType ToSampleType<int16_t>() {
  return kInt16;
}
template <>
SampleType ToSampleType<int32_t>() {
  return kInt32;
}
template <>
SampleType ToSampleType<float>() {
  return kFloat32;
}

// The first parameter is a value of type `fuchsia_audio::SampleType`. C++ template parameters must
// be types or primitive values. Since SampleType is a flexible enum, it's represented by a C++
// class (not a C++ enum), hence the first template parameter cannot be a value of type SampleType.
// We work-around this by using SampleType's underlying type (`uint32_t`).
template <uint32_t SampleTypeValue, typename NumberType>
void TestSilence(NumberType expected_silent_value) {
  Format format = Format::CreateOrDie({
      .sample_type = SampleType(SampleTypeValue),
      .channels = 2,
      .frames_per_second = 48000,
  });

  constexpr int64_t kNumFrames = 4;
  std::vector<NumberType> vec(kNumFrames * format.channels());

  StreamConverter converter(format, format);
  converter.WriteSilence(vec.data(), kNumFrames);

  for (size_t k = 0; k < vec.size(); k++) {
    EXPECT_EQ(vec[k], expected_silent_value) << "k=" << k;
  }
}

TEST(StreamConverterTest, SilenceUnsigned8) {
  TestSilence<fidl::ToUnderlying(kUint8), uint8_t>(0x80);
}
TEST(StreamConverterTest, SilenceSigned16) { TestSilence<fidl::ToUnderlying(kInt16), int16_t>(0); }
TEST(StreamConverterTest, SilenceSigned32) { TestSilence<fidl::ToUnderlying(kInt32), int32_t>(0); }
TEST(StreamConverterTest, SilenceFloat) { TestSilence<fidl::ToUnderlying(kFloat32), float>(0.0f); }

// When we specify source data in uint8/int16/int32 formats, it improves readability to specify
// expected values in that format as well. The expected array itself is float[], so we use this
// function to shift values expressed as uint8, int16, etc., into the [-1.0, 1.0] float range.
//
// Note: 'shift_by' values must be 1 less than might seem obvious, to account for the sign bit.
// E.g.: to shift int16 values -0x8000 and 0x7FFF into float range, shift_by must be 15 (not 16).
void ShiftRightBy(std::vector<float>& floats, int32_t shift_by) {
  for (float& val : floats) {
    for (auto shift_num = 0; shift_num < shift_by; ++shift_num) {
      val *= 0.5f;
    }
  }
}

template <typename SourceNumberType, typename DestNumberType>
void TestCopy(const std::vector<DestNumberType>& expected_dest,
              const std::vector<SourceNumberType>& source, uint32_t channels, bool clip = false) {
  ASSERT_EQ(expected_dest.size(), source.size());

  auto source_format = Format::CreateOrDie({
      .sample_type = ToSampleType<SourceNumberType>(),
      .channels = channels,
      .frames_per_second = 48000,
  });
  auto dest_format = Format::CreateOrDie({
      .sample_type = ToSampleType<DestNumberType>(),
      .channels = channels,
      .frames_per_second = 48000,
  });

  auto dest = std::vector<DestNumberType>(expected_dest.size());
  StreamConverter converter(source_format, dest_format);

  if (clip) {
    converter.CopyAndClip(source.data(), dest.data(), source.size() / channels);
  } else {
    converter.Copy(source.data(), dest.data(), source.size() / channels);
  }
  EXPECT_THAT(dest, ::testing::Pointwise(::testing::Eq(), expected_dest));
}

// With 24 precision bits (including sign), float32 has higher precision than uint8.
TEST(StreamConverterTest, CopyBetweenFloatUint8) {
  // Converting float32 -> uint8: round the 9th bit OUT (not up), clamp to int8 min/max, flip msb.
  auto float_samples_source = std::vector<float>{
      //            expected uint8 dest values:
      -0x898989,  // 0x00 (clamped to uint8 min)
      -0x800000,  // 0x00 (uint8 min)
      -0x408080,  // 0x3F (-0x0.808 rounded out / down to -0x41 = 0xBF)
      -0x000111,  // 0x80 (-0x0.0111 rounded in / up to -0x00)
      +0x000000,  // 0x80 (zero)
      +0x408080,  // 0xC1 (0x0.808 rounded out / up to 0x41)
      +0x7FFFFF,  // 0xFF (uint8 max)
      +0x898989,  // 0xFF (clamped to uint8 max)
  };

  auto uint8_samples = std::vector<uint8_t>{
      0x00, 0x00, 0x3F, 0x80, 0x80, 0xC1, 0xFF, 0xFF,
  };

  // Converting uint8 -> float32 should be bit-for-bit lossless (accounting for msb-toggle).
  auto float_samples_dest = std::vector<float>{
      //            source uint8 values:
      -0x800000,  // 0x00
      -0x800000,  // 0x00
      -0x410000,  // 0x3F
      -0x000000,  // 0x80
      +0x000000,  // 0x80
      +0x410000,  // 0xC1
      +0x7F0000,  // 0xFF
      +0x7F0000,  // 0xFF
  };

  // Shift by six hex digits minus the sign bit.
  ShiftRightBy(float_samples_source, 23);
  ShiftRightBy(float_samples_dest, 23);

  {
    SCOPED_TRACE("float -> uint8");
    TestCopy(uint8_samples, float_samples_source, 1);
  }

  {
    SCOPED_TRACE("uint8 -> float");
    TestCopy(float_samples_dest, uint8_samples, 1);
  }
}

// With 24 precision bits (including sign), float32 has higher precision than int16.
TEST(StreamConverterTest, CopyBetweenFloatInt16) {
  // Converting float32 -> int16, we round the 17th bit OUT (not up), then clamp to int16 min/max.
  auto float_samples_source = std::vector<float>{
      //            expected int16 dest values:
      -0x898989,  // -0x8000 (clamped)
      -0x800000,  // -0x8000 (int16 min)
      -0x408080,  // -0x4081 (round -0x0.80 out / down)
      -0x000111,  // -0x0001 (round -0x0.11 in / up)
      +0x000000,  //  0x0000
      +0x408080,  //  0x4081 (round 0x0.8 out / up)
      +0x7FFFFF,  //  0x7FFF (int16 max)
      +0x898989,  //  0x7FFF (clamped to int16 max)
  };

  auto int16_samples = std::vector<int16_t>{
      -0x8000, -0x8000, -0x4081, -1, 0, 0x4081, 0x7FFF, 0x7FFF,
  };

  // Converting int16 -> float32 should be bit-for-bit lossless.
  auto float_samples_dest = std::vector<float>{
      //            source int16 values:
      -0x800000,  // -0x8000
      -0x800000,  // -0x8000
      -0x408100,  // -0x4081
      -0x000100,  // -0x0001
      +0x000000,  //  0x0000
      +0x408100,  //  0x4081
      +0x7FFF00,  //  0x7FFF
      +0x7FFF00,  //  0x7FFF
  };

  // Shift by six hex digits, minus the sign bit.
  ShiftRightBy(float_samples_source, 23);
  ShiftRightBy(float_samples_dest, 23);

  {
    SCOPED_TRACE("float -> int16");
    TestCopy(int16_samples, float_samples_source, 1);
  }

  {
    SCOPED_TRACE("int16 -> float");
    TestCopy(float_samples_dest, int16_samples, 1);
  }
}

// With 24 precision bits (including sign), float32 has equivalent precision to "int24".
TEST(StreamConverterTest, CopyBetweenFloatInt24) {
  // Converting float32 -> int24, float rounds the 17th bit anyway, then we clamp to int24 min/max.
  auto float_samples_source = std::vector<float>{
      //                                 expected "int24-in-32" values:
      static_cast<float>(-0x8000010),  // -0x80000000 (clamped to int24-in-32 min)
      static_cast<float>(-0x8000000),  // -0x80000000 (int24-in-32 min)
      static_cast<float>(-0x6543210),  // -0x65432100
      static_cast<float>(-0x4080808),  // -0x40808100 (round -0x0.8 out /down)
      static_cast<float>(-0x0234567),  // -0x02345600 (round -0x0.7 in  / up)
      static_cast<float>(0),           // +0x00000000
      static_cast<float>(0x0123450),   // +0x01234500
      static_cast<float>(0x0234567),   // +0x02345600 (round 0x0.7 in  / down)
      static_cast<float>(0x4080808),   // +0x40808100 (round 0x0.8 out / up)
      static_cast<float>(0x6543210),   // +0x65432100
      static_cast<float>(0x7FFFFF0),   // +0x7FFFFF00 (int24-in-32 max)
      static_cast<float>(0x7FFFFF0),   // +0x7FFFFF00 (clamped to int24-in-32 max)
  };

  auto int24_samples = std::vector<int32_t>{
      kMinInt24In32, kMinInt24In32, -0x65432100, -0x40808100, -0x02345600,   0,
      0x01234500,    0x02345600,    0x40808100,  0x65432100,  kMaxInt24In32, kMaxInt24In32,
  };

  // Converting int24-in-32 -> float32 should be bit-for-bit lossless.
  auto float_samples_dest = std::vector<float>{
      //                                 source "int24-in-32" values:
      static_cast<float>(-0x8000000),  // -0x80000000
      static_cast<float>(-0x8000000),  // -0x80000000
      static_cast<float>(-0x6543210),  // -0x65432100
      static_cast<float>(-0x4080810),  // -0x40808100
      static_cast<float>(-0x0234560),  // -0x02345600
      static_cast<float>(0),           //  0x00000000
      static_cast<float>(0x0123450),   //  0x01234500
      static_cast<float>(0x0234560),   //  0x02345600
      static_cast<float>(0x4080810),   //  0x40808100
      static_cast<float>(0x6543210),   //  0x65432100
      static_cast<float>(0x7FFFFF0),   //  0x7FFFFF00
      static_cast<float>(0x7FFFFF0),   //  0x7FFFFF00
  };

  // Shift by seven hex digits, minus the sign bit.
  ShiftRightBy(float_samples_source, 27);
  ShiftRightBy(float_samples_dest, 27);

  {
    SCOPED_TRACE("float -> int24");
    TestCopy(int24_samples, float_samples_source, 1);
  }

  {
    SCOPED_TRACE("int24 -> float");
    TestCopy(float_samples_dest, int24_samples, 1);
  }
}

TEST(StreamConverterTest, CopyBetweenFloatFloat) {
  auto float_samples_source = std::vector<float>{
      -1.1f, 1.1f, -1.0f, 1.0f, -0.503921568f, 0.503921568f, -0.000000119f, 0.000000119f, 0,
  };

  auto float_samples_dest = std::vector<float>{
      -1.0f, 1.0f, -1.0f, 1.0f, -0.503921568f, 0.503921568f, -0.000000119f, 0.000000119f, 0,
  };

  {
    SCOPED_TRACE("float -> float without clipping");
    TestCopy(float_samples_source, float_samples_source, 1, /*clip=*/false);
  }

  {
    SCOPED_TRACE("float -> float with clipping");
    TestCopy(float_samples_dest, float_samples_source, 1, /*clip=*/true);
  }
}

TEST(StreamConverterTest, CopyBetweenFloatFloatWith2Chan) {
  constexpr uint32_t kNumChannels = 2;

  // For each frame, use different values per channel to verify channel independence.
  auto float_samples_source = std::vector<float>{
      // clang-format off
      -1.1f,          1.1f,
       1.1f,         -1.1f,
       1.0f,          1.0f,
       0.000000119f,  0,
       0,            -0.000000119f,
      -0.503921568f,  0.503921568f,
       0.0f,          0.0f,
      // clang-format on
  };

  auto float_samples_dest = std::vector<float>{
      // clang-format off
      -1.0f,          1.0f,  // clamped
       1.0f,         -1.0f,  // clamped
       1.0f,          1.0f,
       0.000000119f,  0,
       0,            -0.000000119f,
      -0.503921568f,  0.503921568f,
       0.0f,          0.0f,
      // clang-format on
  };

  ASSERT_EQ(float_samples_source.size() % kNumChannels, 0u);
  ASSERT_EQ(float_samples_dest.size() % kNumChannels, 0u);

  {
    SCOPED_TRACE("float -> float without clipping");
    TestCopy(float_samples_source, float_samples_source, kNumChannels, /*clip=*/false);
  }

  {
    SCOPED_TRACE("float -> float with clipping");
    TestCopy(float_samples_dest, float_samples_source, kNumChannels, /*clip=*/true);
  }
}

TEST(StreamConverterTest, CopyBetweenFloatFloatWith4Chan) {
  constexpr uint32_t kNumChannels = 4;

  // For each frame, use different values per channel to verify channel independence.
  auto float_samples_source = std::vector<float>{
      // clang-format off
      -1.1f,   -1.0f,    1.0f,    1.1f,
      -0.75f,  -0.25f,   0.25f,   0.75f,
      // clang-format on
  };
  auto float_samples_dest = std::vector<float>{
      // clang-format off
      -1.0f,   -1.0f,    1.0f,    1.0f,  // clamped
      -0.75f,  -0.25f,   0.25f,   0.75f,
      // clang-format on
  };

  ASSERT_EQ(float_samples_source.size() % kNumChannels, 0u);
  ASSERT_EQ(float_samples_dest.size() % kNumChannels, 0u);

  {
    SCOPED_TRACE("float -> float without clipping");
    TestCopy(float_samples_source, float_samples_source, kNumChannels, /*clip=*/false);
  }

  {
    SCOPED_TRACE("float -> float with clipping");
    TestCopy(float_samples_dest, float_samples_source, kNumChannels, /*clip=*/true);
  }
}

TEST(StreamConverterTest, CopyBetweenInt16Int16) {
  auto int16_samples =
      std::vector<int16_t>{-0x8000, -0x8000, -0x4081, -1, 0, 0x4081, 0x7FFF, 0x7FFF};

  {
    SCOPED_TRACE("mono");
    TestCopy(int16_samples, int16_samples, 1);
  }

  {
    SCOPED_TRACE("stereo");
    TestCopy(int16_samples, int16_samples, 2);
  }
}

TEST(StreamConverterTest, CopyBetweenInt32Int32) {
  auto int32_samples =
      std::vector<int32_t>{-0x8000, -0x8000, -0x4081, -1, 0, 0x4081, 0x7FFF, 0x7FFF};

  {
    SCOPED_TRACE("mono");
    TestCopy(int32_samples, int32_samples, 1);
  }

  {
    SCOPED_TRACE("stereo");
    TestCopy(int32_samples, int32_samples, 2);
  }
}

TEST(StreamConverterTest, ClipInfinitiesFloat32) {
  auto format = Format::CreateOrDie({
      .sample_type = kFloat32,
      .channels = 1,
      .frames_per_second = 48000,
  });

  auto source = std::vector<float>{
      -std::numeric_limits<float>::infinity(),
      std::numeric_limits<float>::infinity(),
  };
  auto dest = std::vector<float>(2);
  StreamConverter converter(format, format);
  converter.CopyAndClip(source.data(), dest.data(), 2);

  // Should be clamped.
  EXPECT_FLOAT_EQ(dest[0], -1.0f);
  EXPECT_FLOAT_EQ(dest[1], 1.0f);
  EXPECT_TRUE(std::isnormal(dest[0]));
  EXPECT_TRUE(std::isnormal(dest[1]));
}

// Currently, StreamConverter makes no explicit effort to detect and prevent NAN output.
// TODO(https://fxbug.dev/42165029): Consider a mode where we eliminate NANs (presumably emitting 0
// instead).
TEST(StreamConverterTest, DISABLED_NanFloat32) {
  auto format = Format::CreateOrDie({
      .sample_type = kFloat32,
      .channels = 1,
      .frames_per_second = 48000,
  });

  auto source = std::vector<float>{NAN};
  auto dest = std::vector<float>(1);
  StreamConverter converter(format, format);
  converter.Copy(source.data(), dest.data(), 1);

  // Should be clamped.
  EXPECT_FLOAT_EQ(dest[0], 0.0f);
  EXPECT_FALSE(std::isnan(dest[0]));
  EXPECT_TRUE(std::isnormal(dest[0]));
}

// Currently, StreamConverter makes no explicit effort to detect and prevent subnormal output.
// TODO(https://fxbug.dev/42165029): Consider a mode where we detect subnormals and round to zero.
TEST(StreamConverterTest, DISABLED_SubnormalsFloat32) {
  auto format = Format::CreateOrDie({
      .sample_type = kFloat32,
      .channels = 1,
      .frames_per_second = 48000,
  });

  auto source = std::vector<float>{std::numeric_limits<float>::denorm_min()};
  auto dest = std::vector<float>(1);
  StreamConverter converter(format, format);
  converter.Copy(source.data(), dest.data(), 1);

  // Should be clamped.
  EXPECT_FLOAT_EQ(dest[0], 0.0f);
  EXPECT_TRUE(std::isnormal(dest[0]));
}

}  // namespace
}  // namespace media_audio
