// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sysmem/server/pad_for_block_size.h"

#include <lib/fit/defer.h>
#include <lib/zx/clock.h>

#include <cinttypes>
#include <random>

#include <gtest/gtest.h>

// In this unit test we focus on covering PaddedSizeFromBlockSize.

namespace {

using sysmem_service::PaddedSizeFromBlockSize;

auto complain_to_stdout = [](sysmem_service::Location location,
                             fuchsia_logging::LogSeverity severity, const char* message) {
  printf("%s:%d severity: %u message: %s\n", location.file(), location.line(), severity, message);
};
auto mute_complain = [](sysmem_service::Location location, fuchsia_logging::LogSeverity severity,
                        const char* message) {};

// leave this comment and test first please
//
// As a motivating example, consider a participant specifying `PixelFormat.R8G8B8A8` and `min_size`
// {450, 450} and `max_size` {450, 450}, with `bytes_per_row_divisor` of 64. For this example, let's
// assume the aggregated `bytes_per_row_divisor` is also 64, resulting in `bytes_per_row` of 1856.
// For this participant, when reading and/or writing a (PixelFormatModifier.Linear) pixel (typically
// as part of a larger operation targeting other pixels also), this participant needs to access a
// whole aligned 4x4 "pixel" block at a time (for valid performance reasons), even if some of those
// "pixels" are not specifically targeted, and even if some of those "pixels" are not actually
// located within the image's `fuchsia.images2.ImageFormat.size`, and even if some of those "pixels"
// have their data stored at offsets that are beyond the image's size in bytes as calculated by
// `ImageFormatImageSize`.
//
// Some padding bytes are needed, and the number of padding bytes needed depends on the aggregated
// constraints as influenced by other participants. No other non-pad-requesting participant needs to
// treat the buffer as being larger than 1856x450 bytes. Only sysmem and the participant with the
// 4x4 block access constraint need to know about the participant's aligned 4x4 block padding
// constraint.
//
// In this example, the 4x4 block access participant needs to access additional "pixels" to the
// right of and below the 450x450 image (450/4 is 112.5). In particular, the last byte of the bottom
// right "pixel" of the bottom right 4x4 block is the last byte this participant needs to be able to
// access, implying a minimum VMO size that's at least the offset of the last byte of that pixel
// plus one. This way, all the participant's 4x4 "pixel" block accesses will be within the buffer
// VMO's size, but not necessarily within the BufferMemorySettings.size_bytes which does not include
// padding bytes flowing from pad_* constraints. Specifically, the last byte of the bottom-right
// image pixel at x y 449 449 is in 4KiB page 203. That image pixel is in an aligned 4x4 block whose
// bottom-right "pixel" has x y 451 451. The last byte of "pixel" at x y 451 451 is in 4KiB page
// 204. For this reason sysmem will make sure the VMO size is at least 205 4KiB pages, and will set
// `BufferMemorySettings.size_bytes` to 1856x450 bytes.
TEST(PadForBlockSize, PageForPaddingWouldBeProvided) {
  // This is intentionally not using zx_system_get_page_size(); this test isn't about the system
  // page size, but ensuring sufficient space when using blocks implies more space needed.
  constexpr uint64_t kPageSize = 4 * 1024;

  constexpr uint32_t kWidth = 450;
  constexpr uint32_t kHeight = 450;
  constexpr uint32_t kBytesPerPixel = 4;
  constexpr uint32_t kBytesPerRowDivisor = 64;
  fuchsia_sysmem2::ImageFormatConstraints image_constraints;
  image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kR8G8B8A8;
  image_constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  image_constraints.min_size() = {kWidth, kHeight};
  image_constraints.max_size() = {kWidth, kHeight};
  image_constraints.bytes_per_row_divisor() = kBytesPerRowDivisor;
  // PaddedSizeFromBlockSize can assert these are already set.
  image_constraints.size_alignment() = {1, 1};
  image_constraints.max_bytes_per_row() = 0xFFFFFFFF;

  auto image_format_result = ImageConstraintsToFormat(image_constraints, kWidth, kHeight);
  ZX_DEBUG_ASSERT(image_format_result.is_ok());
  auto& image_format = image_format_result.value();

  // Given constraints above, this is what goes into BufferMemorySettings.size_bytes.
  uint64_t without_blocks_bytes = ImageFormatImageSize(image_format);
  constexpr uint64_t kExpectedWithoutBlocksBytes =
      fbl::round_up(kBytesPerPixel * kWidth, kBytesPerRowDivisor) * kHeight;
  ASSERT_EQ(kExpectedWithoutBlocksBytes, without_blocks_bytes);
  uint64_t buffer_settings_size_bytes = without_blocks_bytes;

  const fuchsia_math::SizeU kBlockSize = {4, 4};
  image_constraints.pad_for_block_size() = kBlockSize;
  auto with_blocks_bytes_result =
      PaddedSizeFromBlockSize(image_constraints, buffer_settings_size_bytes, complain_to_stdout);
  ZX_DEBUG_ASSERT(with_blocks_bytes_result.is_ok());
  uint64_t with_blocks_bytes = with_blocks_bytes_result.value();

  uint64_t pages_without_blocks = fbl::round_up(without_blocks_bytes, kPageSize) / kPageSize;
  uint64_t pages_with_blocks = fbl::round_up(with_blocks_bytes, kPageSize) / kPageSize;

  ASSERT_EQ(204u, pages_without_blocks);
  // While PaddedSizeFromBlockSize in general is only guaranteed to be an upper bound, when
  // min_size == max_size, it's an exact answer, at least given the specific constraints set above
  // and no future-added constraints set.
  ASSERT_EQ(205u, pages_with_blocks);
}

TEST(PadForBlockSize, MinSizeOnlyGivesExactAnswer) {
  // When no extra buffers_settings_size_bytes is created by any other constraint other than what's
  // needed by min_size, the answer from PaddedSizeFromBlockSize will be exactly what's needed by
  // min_size.

  // This is intentionally not using zx_system_get_page_size(); this test isn't about the system
  // page size, but ensuring sufficient space when using blocks implies more space needed.
  constexpr uint64_t kPageSize = 4 * 1024;

  constexpr uint32_t kWidth = 450;
  constexpr uint32_t kHeight = 450;
  constexpr uint32_t kBytesPerPixel = 4;
  constexpr uint32_t kBytesPerRowDivisor = 64;
  fuchsia_sysmem2::ImageFormatConstraints image_constraints;
  image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kR8G8B8A8;
  image_constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  // The min_size is set without required_max_size, required_max_size_list, or
  // BufferMemoryConstraints.min_size_bytes, so the buffer_settings_size_bytes will be only large
  // enough to hold min_size, and PaddedSizeFromBlockSize() will be only block-aligned up from
  // min_size.
  image_constraints.min_size() = {kWidth, kHeight};
  // pad_for_block_size requires max width < 0xFFFFFFFF but for this test we don't want to constrain
  // with max_size, we want to constrain only with BufferMemorySettings.size_bytes; with no
  // reasonable max set here, the only things preventing substantial size overhead are
  // min_size.height and BufferMemorySettings.size_bytes (buffer_settings_size_bytes below)
  image_constraints.max_size() = {0xFFFFFFFE, 0xFFFFFFFF};
  image_constraints.bytes_per_row_divisor() = kBytesPerRowDivisor;
  // PaddedSizeFromBlockSize can assert these are already set.
  image_constraints.size_alignment() = {1, 1};
  // don't want to constrain with max_bytes_per_row in this test, but PaddedSizeFromBlockSize
  // requires this is set
  image_constraints.max_bytes_per_row() = 0xFFFFFFFF;

  auto image_format_result = ImageConstraintsToFormat(image_constraints, kWidth, kHeight);
  ZX_DEBUG_ASSERT(image_format_result.is_ok());
  auto& image_format = image_format_result.value();

  // Given constraints above, this is what goes into BufferMemorySettings.size_bytes.
  uint64_t without_blocks_bytes = ImageFormatImageSize(image_format);
  constexpr uint64_t kExpectedWithoutBlocksBytes =
      fbl::round_up(kBytesPerPixel * kWidth, kBytesPerRowDivisor) * kHeight;
  ASSERT_EQ(kExpectedWithoutBlocksBytes, without_blocks_bytes);
  uint64_t buffer_settings_size_bytes = without_blocks_bytes;

  const fuchsia_math::SizeU kBlockSize = {4, 4};
  image_constraints.pad_for_block_size() = kBlockSize;
  auto with_blocks_bytes_result =
      PaddedSizeFromBlockSize(image_constraints, buffer_settings_size_bytes, complain_to_stdout);
  ZX_DEBUG_ASSERT(with_blocks_bytes_result.is_ok());
  uint64_t with_blocks_bytes = with_blocks_bytes_result.value();

  uint64_t pages_without_blocks = fbl::round_up(without_blocks_bytes, kPageSize) / kPageSize;
  uint64_t pages_with_blocks = fbl::round_up(with_blocks_bytes, kPageSize) / kPageSize;

  ASSERT_EQ(204u, pages_without_blocks);

  // While in general PaddedSizeFromBlockSize is only guaranteed to be an upper bound, in this case
  // the min_size constraint and buffer_settings_size_bytes allow for an exact answer.
  ASSERT_EQ(205u, pages_with_blocks);
  {
    auto blocks_image_format = image_format;
    blocks_image_format.size()->width() =
        fbl::round_up(blocks_image_format.size()->width(), kBlockSize.width());
    blocks_image_format.size()->height() =
        fbl::round_up(blocks_image_format.size()->height(), kBlockSize.height());
    uint64_t with_blocks_bytes_expected = ImageFormatImageSize(blocks_image_format);
    ASSERT_EQ(with_blocks_bytes_expected, with_blocks_bytes);
  }
}

TEST(PadForBlockSize, MinSizeHeightSetLimitsVmoBytesOverhead) {
  // When no extra buffers_settings_size_bytes is created by any other constraint other than what's
  // needed by min_size, the answer from PaddedSizeFromBlockSize will be exactly what's needed by
  // min_size.

  constexpr uint32_t kWidth = 450;
  // have min_size.height fully populate a block row with valid pixels, to give the second-widest
  // width within PaddedSizeFromBlockSize something to do; we have size_alignment.height set to 1 so
  // the second-widest width will have more overhead (3 rows of "pixels" not populated with valid
  // pixels) than the widest width which has no overhead due to all pixel rows of the last block
  // fully populated with valid pixels (as much as is permitted by kBytesPerRowDivisor)
  constexpr uint32_t kHeight = fbl::round_up(450u, 4u);
  constexpr uint32_t kBytesPerPixel = 4;
  constexpr uint32_t kBytesPerRowDivisor = 64;
  fuchsia_sysmem2::ImageFormatConstraints image_constraints;
  image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kR8G8B8A8;
  image_constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  // We separately require buffer_settings_size_bytes sufficient to store kWidth, kHeight using
  // BufferMemoryConstraints.min_size_bytes (essentially).
  image_constraints.min_size() = {1, kHeight};
  // pad_for_block_size requires max width < 0xFFFFFFFF but for this test we don't want to constrain
  // with max_size, we want to constrain only with BufferMemorySettings.size_bytes; with no
  // reasonable max set here, the only things preventing substantial size overhead are
  // min_size.height and BufferMemorySettings.size_bytes (buffer_settings_size_bytes below)
  image_constraints.max_size() = {0xFFFFFFFE, 0xFFFFFFFF};
  image_constraints.bytes_per_row_divisor() = kBytesPerRowDivisor;
  // PaddedSizeFromBlockSize can assert these are already set.
  image_constraints.size_alignment() = {1, 1};
  // don't want to constrain with max_bytes_per_row in this test, but PaddedSizeFromBlockSize
  // requires this is set
  image_constraints.max_bytes_per_row() = 0xFFFFFFFF;

  // This is essentially causing buffer_settings_size_bytes to be large enough to store kWidth,
  // kHeight, analogous to setting BufferMemoryConstraints.min_size_bytes or putting kWidth, kHeight
  // in required_max_size_list (but we're not running full sysmem here so we specify kWidth instead
  // of 1 here).
  auto image_format_result = ImageConstraintsToFormat(image_constraints, kWidth, kHeight);
  ZX_DEBUG_ASSERT(image_format_result.is_ok());
  auto& image_format = image_format_result.value();

  // Given constraints above, this is what would go into BufferMemorySettings.size_bytes if it
  // weren't for kBufferMemoryConstraintsMinSizeBytes.
  uint64_t without_blocks_bytes = ImageFormatImageSize(image_format);
  constexpr uint64_t kExpectedWithoutBlocksBytes =
      fbl::round_up(kBytesPerPixel * kWidth, kBytesPerRowDivisor) * kHeight;
  ASSERT_EQ(kExpectedWithoutBlocksBytes, without_blocks_bytes);

  // This is large enough to store kWidth, kHeight, but min_size only requires a width at least 1.
  // This means a second-widest width will be possible within PaddedSizeFromBlockSize() below.
  uint64_t buffer_settings_size_bytes = without_blocks_bytes;

  const fuchsia_math::SizeU kBlockSize = {4, 4};
  image_constraints.pad_for_block_size() = kBlockSize;
  // A second-widest width (in blocks) will be possible, but since
  auto with_blocks_bytes_result =
      PaddedSizeFromBlockSize(image_constraints, buffer_settings_size_bytes, complain_to_stdout);
  ZX_DEBUG_ASSERT(with_blocks_bytes_result.is_ok());
  uint64_t with_blocks_bytes = with_blocks_bytes_result.value();

  {
    auto blocks_image_format = image_format;
    blocks_image_format.size()->width() =
        fbl::round_up(blocks_image_format.size()->width(), kBlockSize.width());
    blocks_image_format.size()->height() =
        fbl::round_up(blocks_image_format.size()->height(), kBlockSize.height());
    uint64_t with_blocks_bytes_gt_this = ImageFormatImageSize(blocks_image_format);
    // Because the second-widest width in PaddedSizeFromBlockSize has more padding than the widest
    // width, this will be GT not EQ or GE. We don't attempt to replicate the
    // PaddedSizeFromBlockSize calculation of what the second-widest width is here to assert
    // equality. That would be an overly-brittle test anyway. Instead we check GT not EQ here, then
    // also check that the size overhead is reasonable given that min_size.height was set quite a
    // bit greater than 1 (450).
    ASSERT_GT(with_blocks_bytes, with_blocks_bytes_gt_this);
    // as an upper bound on what with_blocks_bytes_at_least will be, we can do a simpler calculation
    // here than PaddedSizeFromBlockSize uses for the second-widest width - here we compute the size
    // bytes we'd get given kWidth block-aligned up (same as just above) and one more block row than
    // just above. This is more width than the second-widest width, but this is easier to calculate
    // and avoids duplicating the code under test in the test essentially.
    blocks_image_format.size()->height() += kBlockSize.height();
    uint64_t with_blocks_bytes_lt_this = ImageFormatImageSize(blocks_image_format);
    // The actual second-widest with in PaddedSizeFromBlockSize will be less than
    // blocks_image_format.size()->width(), so this can be LT not LE.
    //
    // This is also asserting that the bytes overhead is not excessive when min_size.height is set
    // quite a bit larger than 1.
    ASSERT_LT(with_blocks_bytes, with_blocks_bytes_lt_this);
  }

  // This threshold of this assert is obtained from the printf below, with a tiny epsilon manually
  // added. This serves as an anti-regression check that can be tighter than other checks above, but
  // without replicating PaddedSizeFromBlockSize code here.
  double block_padded_over_not_padded =
      static_cast<double>(with_blocks_bytes) / static_cast<double>(without_blocks_bytes);
  printf("block_padded_over_not_padded: %g", block_padded_over_not_padded);
  ASSERT_LE(block_padded_over_not_padded, 1.0065);
}

TEST(PadForBlockSize, SizeAlignmentHeightSetLimitsVmoBytesOverhead) {
  // When no extra buffers_settings_size_bytes is created by any other constraint other than what's
  // needed by min_size, the answer from PaddedSizeFromBlockSize will be exactly what's needed by
  // min_size.

  constexpr uint32_t kWidth = 450;
  constexpr uint32_t kHeight = 450;
  constexpr uint32_t kBytesPerPixel = 4;
  constexpr uint32_t kBytesPerRowDivisor = 64;
  fuchsia_sysmem2::ImageFormatConstraints image_constraints;
  image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kR8G8B8A8;
  image_constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  // We separately require buffer_settings_size_bytes sufficient to store kWidth, kHeight using an
  // entry in required_max_size_list below.
  image_constraints.min_size() = {1, 1};
  image_constraints.required_max_size_list() = std::vector<fuchsia_math::SizeU>{{kWidth, kHeight}};
  // pad_for_block_size requires max width < 0xFFFFFFFF but for this test we don't want to constrain
  // with max_size, we want to constrain only with BufferMemorySettings.size_bytes and
  // size_alignment.height; with no reasonable max set here, the only things limiting overhead are
  // size_alignment.height and BufferMemorySettings.size_bytes (buffer_settings_size_bytes below).
  image_constraints.max_size() = {0xFFFFFFFE, 0xFFFFFFFF};
  image_constraints.bytes_per_row_divisor() = kBytesPerRowDivisor;
  // PaddedSizeFromBlockSize can assert these are already set.
  //
  // Setting the height to 2 here will limit overhead somewhat, but the overhead will still be
  // fairly high in this test/example.
  constexpr uint32_t kSizeAlignmentHeight = 2;
  image_constraints.size_alignment() = {1, kSizeAlignmentHeight};
  // don't want to constrain with max_bytes_per_row in this test, but PaddedSizeFromBlockSize
  // requires this is set
  image_constraints.max_bytes_per_row() = 0xFFFFFFFF;

  // This is essentially causing buffer_settings_size_bytes to be large enough to store kWidth,
  // kHeight, analogous to setting BufferMemoryConstraints.min_size_bytes or putting kWidth, kHeight
  // in required_max_size_list (but we're not running full sysmem here so we specify kWidth instead
  // of 1 here).
  auto image_format_result = ImageConstraintsToFormat(image_constraints, kWidth, kHeight);
  ZX_DEBUG_ASSERT(image_format_result.is_ok());
  auto& image_format = image_format_result.value();

  // Given constraints above, this is what would go into BufferMemorySettings.size_bytes.
  uint64_t without_blocks_bytes = ImageFormatImageSize(image_format);
  constexpr uint64_t kExpectedWithoutBlocksBytes =
      fbl::round_up(kBytesPerPixel * kWidth, kBytesPerRowDivisor) * kHeight;
  ASSERT_EQ(kExpectedWithoutBlocksBytes, without_blocks_bytes);

  // This is large enough to store kWidth, kHeight, but min_size.width is only 1. This means a
  // second-widest width will be possible within PaddedSizeFromBlockSize() below.
  uint64_t buffer_settings_size_bytes = without_blocks_bytes;

  const fuchsia_math::SizeU kBlockSize = {4, 4};
  // this test is intentionally not having size_alignment.height force populating full block rows
  // with valid pixels
  ZX_ASSERT(kBlockSize.height() > kSizeAlignmentHeight);
  image_constraints.pad_for_block_size() = kBlockSize;
  // A second-widest width (in blocks) will be possible, but since
  auto with_blocks_bytes_result =
      PaddedSizeFromBlockSize(image_constraints, buffer_settings_size_bytes, complain_to_stdout);
  ZX_DEBUG_ASSERT(with_blocks_bytes_result.is_ok());
  uint64_t with_blocks_bytes = with_blocks_bytes_result.value();

  {
    auto blocks_image_format = image_format;
    blocks_image_format.size()->width() =
        fbl::round_up(blocks_image_format.size()->width(), kBlockSize.width());
    blocks_image_format.size()->height() =
        fbl::round_up(blocks_image_format.size()->height(), kBlockSize.height());
    uint64_t with_blocks_bytes_gt_this = ImageFormatImageSize(blocks_image_format);
    // Because a very wide image with height only kSizeAlignmentHeight is possible given above
    // constraints, this will be GT not EQ.
    ASSERT_GT(with_blocks_bytes, with_blocks_bytes_gt_this);

    constexpr uint64_t kWhatIfHeight1 = 1;
    ZX_ASSERT(kSizeAlignmentHeight > kWhatIfHeight1);
    ZX_ASSERT((kBytesPerRowDivisor % kBytesPerPixel) == 0);
    ZX_ASSERT(kBlockSize.height() > kSizeAlignmentHeight);
    uint64_t max_width_if_height_were_1 =
        fbl::round_down(buffer_settings_size_bytes / kBytesPerPixel / kWhatIfHeight1,
                        kBytesPerRowDivisor / kBytesPerPixel);
    // Note the multiplication by a factor a little greater than 1/2 at the end of this statement.
    // There is a factor of 2 because the actual height is at least 2 thanks to
    // kSizeAlignmentHeight, which limits the bytes used due to width to about half as wide worth of
    // bytes. In addition we use kBlockSize.height() minus kSizeAlignmentHeight not
    // kBlockSize.height() minus 1 here, to account for the fewer padding rows of "pixels" to fill
    // up the rest of the block height.
    uint64_t padding_lt_this = max_width_if_height_were_1 *
                               (kBlockSize.height() - kSizeAlignmentHeight) * kBytesPerPixel * 6 /
                               10;

    uint64_t padded_size_lt_this = without_blocks_bytes + padding_lt_this;

    ASSERT_LT(with_blocks_bytes, padded_size_lt_this);
  }

  // This threshold of this assert is obtained from the printf below, with a tiny epsilon manually
  // added. This serves as an anti-regression check that can be tighter than other checks above, but
  // without replicating PaddedSizeFromBlockSize code here.
  double block_padded_over_not_padded =
      static_cast<double>(with_blocks_bytes) / static_cast<double>(without_blocks_bytes);
  printf("block_padded_over_not_padded: %g", block_padded_over_not_padded);
  // This is a lot of overhead still, but it's less than 4 which would be the overhead if we didn't
  // account for size_alignment.height when computing the min height in PaddedSizeFromBlockSize.
  // Overhead this high or higher is why it's required to also set max_size.width when setting
  // pad_for_block_size.
  //
  // The block_padded_over_not_padded is exactly 2.0 as of this comment. We're comparing this way
  // for consistentcy with other tests, despite IEEE 754 being able to represent 2.0 exactly.
  ASSERT_LE(block_padded_over_not_padded, 2.001);
}

TEST(PadForBlockSize, MaxSizeWidthSetLimitsVmoBytesOverhead) {
  // When no extra buffers_settings_size_bytes is created by any other constraint other than what's
  // needed by min_size, the answer from PaddedSizeFromBlockSize will be exactly what's needed by
  // min_size.

  constexpr uint32_t kWidth = 450;
  constexpr uint32_t kWidthMultiple = 4;
  constexpr uint32_t kHeight = 450;
  constexpr uint32_t kBytesPerPixel = 4;
  constexpr uint32_t kBytesPerRowDivisor = 64;
  fuchsia_sysmem2::ImageFormatConstraints image_constraints;
  image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kR8G8B8A8;
  image_constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  // We separately require buffer_settings_size_bytes sufficient to store kWidth, kHeight using
  // an entry in required_max_size_list.
  image_constraints.min_size() = {1, 1};
  image_constraints.required_max_size_list() = std::vector<fuchsia_math::SizeU>{{kWidth, kHeight}};
  // By setting max_size.width, excessive overhead is prevented, despite min_size {1, 1} and
  // required_max_size_list having {kWidth, kHeight}. This way the max_size.width prevents making
  // block padding room for an image that's very wide and only 1 valid pixel tall (given
  // min_size.height is 1).
  image_constraints.max_size() = {kWidthMultiple * kWidth, 0xFFFFFFFF};
  image_constraints.bytes_per_row_divisor() = kBytesPerRowDivisor;
  // PaddedSizeFromBlockSize can assert these are already set.
  image_constraints.size_alignment() = {1, 1};
  // don't want to constrain with max_bytes_per_row in this test, but PaddedSizeFromBlockSize
  // requires this is set
  image_constraints.max_bytes_per_row() = 0xFFFFFFFF;

  // This is the size that sysmem would use in computing BufferMemorySettings.size_bytes given
  // required_max_size_list entry above and nothing else making BufferMemorySettings.size_bytes
  // bigger.
  auto image_format_result = ImageConstraintsToFormat(image_constraints, kWidth, kHeight);
  ZX_DEBUG_ASSERT(image_format_result.is_ok());
  auto& image_format = image_format_result.value();
  // This without_blocks_bytes would allow for a very wide image that's only one valid pixel tall
  // if it weren't for max_size.width set above.
  uint64_t without_blocks_bytes = ImageFormatImageSize(image_format);
  constexpr uint64_t kExpectedWithoutBlocksBytes =
      fbl::round_up(kBytesPerPixel * kWidth, kBytesPerRowDivisor) * kHeight;
  ASSERT_EQ(kExpectedWithoutBlocksBytes, without_blocks_bytes);

  // This is large enough to store kWidth, kHeight (and various other sizes including sizes with
  // greater width but smaller height and vice versa).
  uint64_t buffer_settings_size_bytes = without_blocks_bytes;

  const fuchsia_math::SizeU kBlockSize = {4, 4};
  image_constraints.pad_for_block_size() = kBlockSize;
  // A second-widest width (in blocks) will be possible, but since
  auto with_blocks_bytes_result =
      PaddedSizeFromBlockSize(image_constraints, buffer_settings_size_bytes, complain_to_stdout);
  ZX_DEBUG_ASSERT(with_blocks_bytes_result.is_ok());
  uint64_t with_blocks_bytes = with_blocks_bytes_result.value();

  {
    auto blocks_image_format = image_format;
    blocks_image_format.size()->width() =
        fbl::round_up(blocks_image_format.size()->width(), kBlockSize.width());
    blocks_image_format.size()->height() =
        fbl::round_up(blocks_image_format.size()->height(), kBlockSize.height());
    uint64_t with_blocks_bytes_gt_this = ImageFormatImageSize(blocks_image_format);
    // Because of kWidthMultiple and min_size {1, 1}, the amount of space will be GT needed by
    // kWidth and kHeight, not just EQ.
    ASSERT_GT(with_blocks_bytes, with_blocks_bytes_gt_this);

    // For this specific example, this is the amount of padding for a kWidthMultiple wide image
    // that's only 1 valid pixel tall. This is an image with one row of valid pixels that's
    // kWidthMultiple * kWidth wide (the same as set by max_size.width). The additional "pixel" rows
    // in block row 0 are padding.
    uint64_t max_padding =
        fbl::round_up(kWidthMultiple * kWidth * kBytesPerPixel, kBytesPerRowDivisor) *
        (kBlockSize.height() - 1);

    uint64_t padded_size_expected = without_blocks_bytes + max_padding;

    ASSERT_EQ(padded_size_expected, with_blocks_bytes);
  }

  // This threshold of this assert is obtained from the printf below, with a tiny epsilon manually
  // added. This serves as an anti-regression check that can be tighter than other checks above, but
  // without replicating PaddedSizeFromBlockSize code here.
  double block_padded_over_not_padded =
      static_cast<double>(with_blocks_bytes) / static_cast<double>(without_blocks_bytes);
  printf("block_padded_over_not_padded: %g", block_padded_over_not_padded);
  ASSERT_LE(block_padded_over_not_padded, 1.026);
}

TEST(PadForBlockSize, SizeAlignmentEqualToBlockSizeGivesZeroOverhead) {
  // When no extra buffers_settings_size_bytes is created by any other constraint other than what's
  // needed by min_size, the answer from PaddedSizeFromBlockSize will be exactly what's needed by
  // min_size.

  constexpr uint32_t kBlockWidth = 4;
  constexpr uint32_t kBlockHeight = 4;
  const fuchsia_math::SizeU kBlockSize = {kBlockWidth, kBlockHeight};
  constexpr uint32_t kWidth = fbl::round_up(450u, kBlockWidth);
  constexpr uint32_t kHeight = fbl::round_up(450u, kBlockHeight);
  constexpr uint32_t kBytesPerPixel = 4;
  constexpr uint32_t kBytesPerRowDivisor = 64;
  fuchsia_sysmem2::ImageFormatConstraints image_constraints;
  image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kR8G8B8A8;
  image_constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  // We separately require buffer_settings_size_bytes sufficient to store kWidth, kHeight using
  // an entry in required_max_size_list.
  image_constraints.min_size() = {1, 1};
  image_constraints.required_max_size_list() = std::vector<fuchsia_math::SizeU>{{kWidth, kHeight}};
  image_constraints.max_size() = {0xFFFFFFFE, 0xFFFFFFFF};
  image_constraints.bytes_per_row_divisor() = kBytesPerRowDivisor;
  // By setting size_alignment to kBlockSize, there will be zero padding needed.
  image_constraints.size_alignment() = kBlockSize;
  // don't want to constrain with max_bytes_per_row in this test, but PaddedSizeFromBlockSize
  // requires this is set
  image_constraints.max_bytes_per_row() = 0xFFFFFFFF;

  // This is the size that sysmem would use in computing BufferMemorySettings.size_bytes given
  // required_max_size_list entry above and nothing else making BufferMemorySettings.size_bytes
  // bigger.
  auto image_format_result = ImageConstraintsToFormat(image_constraints, kWidth, kHeight);
  ZX_DEBUG_ASSERT(image_format_result.is_ok());
  auto& image_format = image_format_result.value();
  // This without_blocks_bytes would allow for a very wide image that's only one valid pixel tall
  // if it weren't for max_size.width set above.
  uint64_t without_blocks_bytes = ImageFormatImageSize(image_format);
  constexpr uint64_t kExpectedWithoutBlocksBytes =
      fbl::round_up(kBytesPerPixel * kWidth, kBytesPerRowDivisor) * kHeight;
  ASSERT_EQ(kExpectedWithoutBlocksBytes, without_blocks_bytes);

  // This is large enough to store kWidth, kHeight (and various other sizes including sizes with
  // greater width but smaller height and vice versa).
  uint64_t buffer_settings_size_bytes = without_blocks_bytes;

  image_constraints.pad_for_block_size() = kBlockSize;
  // A second-widest width (in blocks) will be possible, but since
  auto with_blocks_bytes_result =
      PaddedSizeFromBlockSize(image_constraints, buffer_settings_size_bytes, complain_to_stdout);
  ZX_DEBUG_ASSERT(with_blocks_bytes_result.is_ok());
  uint64_t with_blocks_bytes = with_blocks_bytes_result.value();

  // Because size_alignment == kBlocksize, there's no way to create an image where padding for
  // blocks is needed, since the ImageFormat.size will always be complete blocks which require no
  // padding.
  ASSERT_EQ(without_blocks_bytes, with_blocks_bytes);
}

TEST(PadForBlockSize, MiniStress) {
  // This is to avoid taking a lot longer on sanitizer builds or slow HW. We'll get most of our
  // probes from the faster HW and that's fine given the code under test isn't really HW-specific.
  constexpr zx::duration kMaxDuration = zx::msec(3000);
  // Feel free to reduce this if a given device is just taking too long to do this many, but we want
  // to force some probes to avoid this test "passing" without doing much.
  constexpr uint64_t kMinProbes = 100000;
  // Don't do more than this many probes no matter how fast the HW is.
  constexpr uint64_t kMaxProbes = 10000000;

  // How many unexpected failures from a given cause we collect before failing the test. More than
  // one example can help narrow down why it's failing.
  constexpr uint32_t kCollectUnexpectedExamplesCount = 5;

  // Very roughly around a quarter of the probes are useful (last I checked). The rest are not
  // within buffer_settings_size_bytes aka BufferMemorySettings.size_bytes if we were running the
  // rest of sysmem.
  const uint64_t kInnerTriesPerOuterTry = 80;

  // We're not actually allocating buffers, so the below ranges can create situations where buffers
  // would be quite large without risking OOM.

  constexpr uint32_t kMinBlockSizeDimLog2 = 1;
  constexpr uint32_t kMaxBlockSizeDimLog2 = 12;
  constexpr uint32_t kMinDimension = 1;
  // While it's tempting to make this larger, keeping this within reason may increase the odds of
  // finding a bug by more frequently hitting cases along the edges of what's possible to allocate
  // within BufferMemorySettings.size_bytes.
  //
  // This does not always apply to max_size, which sometimes has max of {0x7FFFFFFF, 0xFFFFFFFF}.
  constexpr uint32_t kMaxDimension = 16384;
  constexpr uint32_t kMinSizeAlignmentDimLog2 = 1;
  constexpr uint32_t kMaxSizeAlignmentDimLog2 = 12;
  // bytes_per_row_divisor is intentionally not required to be a power of 2
  constexpr uint32_t kMinBytesPerRowDivisor = 1;
  // the intent here is to throw in plenty of curveball; this has "factor" in the name because the
  // block width * stride_bytes_per_width_pixel is the other factor
  constexpr uint32_t kMaxBytesPerRowDivisorFactor = 23;

  std::random_device random_device{};
  auto seed = random_device();
  printf("MiniStress seed: %u\n", seed);
  std::mt19937 prng(seed);

  std::uniform_int_distribution<uint32_t> block_dim_log2_distribution(kMinBlockSizeDimLog2,
                                                                      kMaxBlockSizeDimLog2);
  std::uniform_int_distribution<uint32_t> dimension_distribution(kMinDimension, kMaxDimension);
  std::uniform_int_distribution<uint32_t> size_alignment_dim_log2_distribution(
      kMinSizeAlignmentDimLog2, kMaxSizeAlignmentDimLog2);
  // As of this comment, the PixelFormat values which can return true from
  // ImageFormatIsNonTiledSinglePlane are as follows (it'd be nice if fidl C++ codegen would allow
  // for some introspection here short of leaning _way_ too hard on IsUnknown()):
  const std::vector<fuchsia_images2::PixelFormat> kPixelFormats = {
      fuchsia_images2::PixelFormat::kR8G8B8A8,    fuchsia_images2::PixelFormat::kR8G8B8X8,
      fuchsia_images2::PixelFormat::kB8G8R8A8,    fuchsia_images2::PixelFormat::kB8G8R8X8,
      fuchsia_images2::PixelFormat::kB8G8R8,      fuchsia_images2::PixelFormat::kR5G6B5,
      fuchsia_images2::PixelFormat::kR3G3B2,      fuchsia_images2::PixelFormat::kR2G2B2X2,
      fuchsia_images2::PixelFormat::kL8,          fuchsia_images2::PixelFormat::kR8,
      fuchsia_images2::PixelFormat::kR8G8,        fuchsia_images2::PixelFormat::kA2R10G10B10,
      fuchsia_images2::PixelFormat::kA2B10G10R10, fuchsia_images2::PixelFormat::kR8G8B8,
  };
  // Currently there are no fuchsia_images2::PixelFormatModifier values which could return true from
  // ImageFormatIsNonTiledSinglePlane other than Linear (whether that's by definition doesn't need
  // to be decided yet), so we always specify PixelFormatModifier.Linear.
  const fuchsia_images2::PixelFormatModifier kLinear =
      fuchsia_images2::PixelFormatModifier::kLinear;
  for (auto& pixel_format : kPixelFormats) {
    ASSERT_TRUE(ImageFormatIsNonTiledSinglePlane(PixelFormatAndModifier(pixel_format, kLinear)));
  }
  std::uniform_int_distribution<uint32_t> pixel_format_index_distribution(
      0, static_cast<uint32_t>(kPixelFormats.size() - 1));
  std::uniform_int_distribution<uint32_t> bytes_per_row_divisor_distribution(
      kMinBytesPerRowDivisor, kMaxBytesPerRowDivisorFactor);
  const fuchsia_math::SizeU kMaxSizeNotCapped = {0x7FFFFFFF, 0xFFFFFFFF};
  std::uniform_int_distribution<uint32_t> is_max_size_capped_distribution(0, 1);
  std::uniform_int_distribution<uint32_t> is_bytes_per_row_capped_distribution(0, 1);

  struct ResultCounts {
    uint64_t padded_size_from_block_size_failed = 0;
    uint64_t probe_image_constraints_to_format_non_aligned_failed_count = 0;
    uint64_t probe_image_constraints_to_format_aligned_failed_count = 0;
    uint64_t probe_exceeds_buffer_settings_size_bytes_count = 0;
    uint64_t probe_image_with_blocks_bytes_too_low = 0;

    uint64_t probe_success_count = 0;
  };
  ResultCounts result_counts{};
  auto print_final_test_output = fit::defer([&result_counts] {
    printf("padded_size_from_block_size_failed: %" PRIu64 "\n",
           result_counts.padded_size_from_block_size_failed);
    printf("probe_image_constraints_to_format_non_aligned_failed_count: %" PRIu64 "\n",
           result_counts.probe_image_constraints_to_format_non_aligned_failed_count);
    printf("probe_image_constraints_to_format_aligned_failed_count: %" PRIu64 "\n",
           result_counts.probe_image_constraints_to_format_aligned_failed_count);
    printf("probe_exceeds_buffer_settings_size_bytes_count: %" PRIu64 "\n",
           result_counts.probe_exceeds_buffer_settings_size_bytes_count);
    printf("probe_image_with_blocks_bytes_too_low: %" PRIu64 "\n",
           result_counts.probe_image_with_blocks_bytes_too_low);
    printf("probe_success_count: %" PRIu64 "\n", result_counts.probe_success_count);
  });

  zx::duration spent_in_function_under_test = zx::msec(0);
  uint64_t probe_count = 0;
  zx::time begin_time = zx::clock::get_monotonic();
  uint64_t outer_try;
  for (outer_try = 0;
       result_counts.probe_success_count == 0 || probe_count < kMinProbes ||
       (probe_count < kMaxProbes && (zx::clock::get_monotonic() - begin_time) < kMaxDuration);
       ++outer_try) {
    const uint32_t block_width = 1 << block_dim_log2_distribution(prng);
    const uint32_t block_height = 1 << block_dim_log2_distribution(prng);
    const fuchsia_math::SizeU block_size = {block_width, block_height};
    const fuchsia_images2::PixelFormat pixel_format =
        kPixelFormats.at(pixel_format_index_distribution(prng));
    const uint32_t stride_bytes_per_width_pixel =
        ImageFormatStrideBytesPerWidthPixel(PixelFormatAndModifier(pixel_format, kLinear));
    const uint32_t bytes_per_row_divisor =
        std::lcm(bytes_per_row_divisor_distribution(prng),
                 stride_bytes_per_width_pixel * block_size.width());
    const uint32_t size_alignment_width = 1 << size_alignment_dim_log2_distribution(prng);
    const uint32_t size_alignment_height = 1 << size_alignment_dim_log2_distribution(prng);
    const uint32_t allocate_width =
        fbl::round_up(dimension_distribution(prng), size_alignment_width);
    const uint32_t allocate_height =
        fbl::round_up(dimension_distribution(prng), size_alignment_height);
    std::uniform_int_distribution<uint32_t> min_size_width_distribution(kMinDimension,
                                                                        allocate_width);
    std::uniform_int_distribution<uint32_t> min_size_height_distribution(kMinDimension,
                                                                         allocate_height);
    const uint32_t min_size_width = min_size_width_distribution(prng);
    const uint32_t min_size_height = min_size_height_distribution(prng);
    const bool is_max_size_capped = !!is_max_size_capped_distribution(prng);
    std::uniform_int_distribution<uint32_t> max_size_width_distribution(allocate_width,
                                                                        kMaxDimension);
    std::uniform_int_distribution<uint32_t> max_size_height_distribution(allocate_height,
                                                                         kMaxDimension);
    const uint32_t max_size_width =
        is_max_size_capped ? max_size_width_distribution(prng) : kMaxSizeNotCapped.width();
    const uint32_t max_size_height =
        is_max_size_capped ? max_size_height_distribution(prng) : kMaxSizeNotCapped.height();
    const bool is_bytes_per_row_capped = is_bytes_per_row_capped_distribution(prng);
    std::uniform_int_distribution<uint32_t> max_bytes_per_row_distribution(
        fbl::round_up(allocate_width * stride_bytes_per_width_pixel, bytes_per_row_divisor),
        kMaxDimension * stride_bytes_per_width_pixel);
    const uint32_t max_bytes_per_row =
        is_bytes_per_row_capped ? max_bytes_per_row_distribution(prng) : 0xFFFFFFFF;

    fuchsia_sysmem2::ImageFormatConstraints image_constraints;
    image_constraints.pixel_format() = pixel_format;
    image_constraints.pixel_format_modifier() = kLinear;
    // We separately require buffer_settings_size_bytes sufficient to store kWidth, kHeight using
    // an entry in required_max_size_list.
    image_constraints.min_size() = {min_size_width, min_size_height};
    // This doesn't actually accomplish anything in this unit test (other than printing this when/if
    // we hit an unexpected failure), but this is what a client would send to sysmem if this were a
    // subsystem test instead of unit test.
    image_constraints.required_max_size_list() =
        std::vector<fuchsia_math::SizeU>{{allocate_width, allocate_height}};
    // For test logging, and for pretending in this unit test like we're running the rest of sysmem.
    image_constraints.pad_for_block_size() = block_size;
    image_constraints.max_size() = {max_size_width, max_size_height};
    image_constraints.bytes_per_row_divisor() = bytes_per_row_divisor;
    image_constraints.size_alignment() = {size_alignment_width, size_alignment_height};
    image_constraints.max_bytes_per_row() = max_bytes_per_row;

    auto print_unexpected_failure_params = [&image_constraints](
                                               const char* failure_name,
                                               std::optional<fuchsia_math::SizeU> probe_size) {
      printf("#### unexpected failure: %s ####\n", failure_name);
      printf("pixel_format: %u\n", static_cast<uint32_t>(*image_constraints.pixel_format()));
      printf("pixel_format_modifier: 0x%" PRIx64 "\n",
             static_cast<uint64_t>(*image_constraints.pixel_format_modifier()));
      printf("min_size: {%u, %u}\n", image_constraints.min_size()->width(),
             image_constraints.min_size()->height());
      auto& required_0 = image_constraints.required_max_size_list()->at(0);
      printf("required_max_size_list[0]: {%u, %u}\n", required_0.width(), required_0.height());
      auto& pad_for_block_size = *image_constraints.pad_for_block_size();
      printf("pad_for_block_size: {%u, %u}\n", pad_for_block_size.width(),
             pad_for_block_size.height());
      printf("max_size: {%u, %u}\n", image_constraints.max_size()->width(),
             image_constraints.max_size()->height());
      printf("bytes_per_row_divisor: 0x%x %u\n", *image_constraints.bytes_per_row_divisor(),
             *image_constraints.bytes_per_row_divisor());
      auto& size_alignment = *image_constraints.size_alignment();
      printf("size_alignment: {%u, %u}\n", size_alignment.width(), size_alignment.height());
      printf("max_bytes_per_row: %u\n", *image_constraints.max_bytes_per_row());
      if (probe_size.has_value()) {
        printf("probe_size: {%u, %u}\n", probe_size->width(), probe_size->height());
      }
    };

    auto image_format_result =
        ImageConstraintsToFormat(image_constraints, allocate_width, allocate_height);
    if (!image_format_result.is_ok()) {
      print_unexpected_failure_params("ImageConstraintsToFormat(allocate_width, allocate_height)",
                                      std::nullopt);
      ASSERT_TRUE(image_format_result.is_ok());
    }
    auto& image_format = image_format_result.value();

    uint64_t without_blocks_bytes = ImageFormatImageSize(image_format);
    const uint64_t expected_without_blocks_bytes =
        fbl::round_up(static_cast<uint64_t>(stride_bytes_per_width_pixel) * allocate_width,
                      bytes_per_row_divisor) *
        allocate_height;
    if (expected_without_blocks_bytes != without_blocks_bytes) {
      printf("expected_without_blocks_bytes != without_blocks_bytes -- %" PRIu64 " %" PRIu64 "\n",
             expected_without_blocks_bytes, without_blocks_bytes);
      print_unexpected_failure_params("expected_without_blocks_bytes != without_blocks_bytes",
                                      std::nullopt);
      ASSERT_EQ(expected_without_blocks_bytes, without_blocks_bytes);
    }

    // This is large enough to store allocate_width, allocate_height (and possibly various other
    // sizes including sizes with greater width but smaller height and vice versa).
    uint64_t buffer_settings_size_bytes = without_blocks_bytes;

    image_constraints.pad_for_block_size() = {block_width, block_height};
    zx::time before_call = zx::clock::get_monotonic();
    auto with_blocks_bytes_result =
        PaddedSizeFromBlockSize(image_constraints, buffer_settings_size_bytes,
                                outer_try == 304 ? complain_to_stdout : mute_complain);
    zx::time after_call = zx::clock::get_monotonic();
    spent_in_function_under_test += (after_call - before_call);
    if (!with_blocks_bytes_result.is_ok()) {
      ++result_counts.padded_size_from_block_size_failed;
      print_unexpected_failure_params("PaddedSizeFromBlockSize", std::nullopt);
      if (result_counts.padded_size_from_block_size_failed >= kCollectUnexpectedExamplesCount) {
        ASSERT_TRUE(with_blocks_bytes_result.is_ok());
      }
      continue;
    }
    const auto& with_blocks_bytes = with_blocks_bytes_result.value();

    auto constraints_for_block_aligned = image_constraints;
    constraints_for_block_aligned.max_size().reset();
    constraints_for_block_aligned.max_bytes_per_row().reset();
    constraints_for_block_aligned.max_width_times_height().reset();
    // the official size still satisfies size_alignment, but the padding from pad_for_block_size
    // is just padding, not official size
    constraints_for_block_aligned.size_alignment().reset();

    uint64_t min_probe_width = fbl::round_up(image_constraints.min_size()->width(),
                                             image_constraints.size_alignment()->width());
    ZX_ASSERT(min_probe_width <= std::numeric_limits<uint32_t>::max());
    uint64_t max_probe_width =
        (buffer_settings_size_bytes / stride_bytes_per_width_pixel) + (2ull * block_width);
    max_probe_width = std::min(max_probe_width, static_cast<uint64_t>(max_size_width));
    max_probe_width =
        std::min(max_probe_width,
                 fbl::round_down(static_cast<uint64_t>(max_bytes_per_row), bytes_per_row_divisor) /
                     stride_bytes_per_width_pixel);
    max_probe_width = fbl::round_down(max_probe_width, size_alignment_width);
    ZX_ASSERT(max_probe_width <= std::numeric_limits<uint32_t>::max());
    std::uniform_int_distribution<uint32_t> probe_width_distribution(
        static_cast<uint32_t>(min_probe_width), static_cast<uint32_t>(max_probe_width));

    uint64_t min_probe_height = fbl::round_up(image_constraints.min_size()->height(),
                                              image_constraints.size_alignment()->height());
    ZX_ASSERT(min_probe_height <= std::numeric_limits<uint32_t>::max());
    // we have no min_bytes_per_row, so we use bytes_per_row_divisor here because it does also act
    // as a min_bytes_per_row in a sense, and this can avoid the probe space being a lot larger than
    // needed
    uint64_t max_probe_height =
        (buffer_settings_size_bytes / bytes_per_row_divisor) + (2ull * block_height);
    ZX_ASSERT(max_probe_height <= std::numeric_limits<uint32_t>::max());
    max_probe_height = std::min(max_probe_height, static_cast<uint64_t>(max_size_height));
    max_probe_height = fbl::round_down(max_probe_height, size_alignment_height);
    std::uniform_int_distribution<uint32_t> probe_height_distribution(
        static_cast<uint32_t>(min_probe_height), static_cast<uint32_t>(max_probe_height));

    for (uint64_t inner_try = 0; inner_try < kInnerTriesPerOuterTry; ++inner_try) {
      if ((probe_count % 100000) == 0) {
        printf("probe_count: %" PRIu64 " delta_ms: %" PRId64 "\n", probe_count,
               (zx::clock::get_monotonic() - begin_time).to_msecs());
      }
      ++probe_count;

      // The goal is to find an image size that fits in buffer_settings_size_bytes but when aligned
      // up to blocks, doesn't fit in with_blocks_bytes_result. Unlike PaddedSizeFromBlockSize which
      // is meant to find an upper bound for with_blocks_bytes_result quickly, this is a brute force
      // approach to hopefully notice any cases that PaddedSizeFromBlockSize didn't account for, in
      // terms of both size returned and in terms of potential PaddedSizeFromBlockSize crashes or
      // sanitizer issues.

      // These dimensions don't necessarily fit within buffer_settings_size_bytes. We're just
      // sampling random (somewhat-constrained) image sizes where the distribution overlaps the
      // sizes that can fit in buffer_settings_size_bytes.
      const uint32_t probe_width =
          fbl::round_up(probe_width_distribution(prng), size_alignment_width);
      const uint32_t probe_height =
          fbl::round_up(probe_height_distribution(prng), size_alignment_height);
      fuchsia_math::SizeU probe_size = {probe_width, probe_height};

      ZX_DEBUG_ASSERT(probe_width >= constraints_for_block_aligned.min_size()->width());
      ZX_DEBUG_ASSERT(probe_height >= constraints_for_block_aligned.min_size()->height());
      ZX_DEBUG_ASSERT(probe_width <= constraints_for_block_aligned.max_size()->width());
      ZX_DEBUG_ASSERT(probe_height <= constraints_for_block_aligned.max_size()->height());
      ZX_DEBUG_ASSERT(probe_width % size_alignment_width == 0);
      ZX_DEBUG_ASSERT(probe_height % size_alignment_height == 0);
      ZX_DEBUG_ASSERT(fbl::round_up(probe_width * stride_bytes_per_width_pixel,
                                    bytes_per_row_divisor) <= max_bytes_per_row);
      auto non_aligned_image_format_result =
          ImageConstraintsToFormat(image_constraints, probe_width, probe_height);
      if (!non_aligned_image_format_result.is_ok()) {
        ++result_counts.probe_image_constraints_to_format_non_aligned_failed_count;
        print_unexpected_failure_params("!non_aligned_image_format_result.is_ok()", probe_size);
        if (result_counts.probe_image_constraints_to_format_non_aligned_failed_count >=
            kCollectUnexpectedExamplesCount) {
          ASSERT_TRUE(non_aligned_image_format_result.is_ok());
        }
        continue;
      }
      auto& non_aligned_image_format = non_aligned_image_format_result.value();

      uint64_t non_aligned_size_bytes = ImageFormatImageSize(non_aligned_image_format);
      if (non_aligned_size_bytes > buffer_settings_size_bytes) {
        // This is expected to happen a lot in this test and it's fine.
        //
        // This probe is already bigger than buffer_settings_size_bytes so the required VMO space
        // returned from PaddedSizeFromBlockSize accounting for blocks is not required to be large
        // enough to hold the aligned_image_format below. This will happen more than 50% of the
        // time, which is worth it to keep the probing simple for this test. We intentionally don't
        // have a max pixel area heuristic for the probing nor any more sophisticated sampling
        // distribution to focus near the boundaries. Random numbers are our friend in this test.
        ++result_counts.probe_exceeds_buffer_settings_size_bytes_count;
        continue;
      }

      const uint32_t block_aligned_probe_width = fbl::round_up(probe_width, block_width);
      const uint32_t block_aligned_probe_height = fbl::round_up(probe_height, block_height);

      ZX_DEBUG_ASSERT(block_aligned_probe_width >=
                      constraints_for_block_aligned.min_size()->width());
      ZX_DEBUG_ASSERT(block_aligned_probe_height >=
                      constraints_for_block_aligned.min_size()->height());
      ZX_DEBUG_ASSERT(!constraints_for_block_aligned.max_size().has_value());
      ZX_DEBUG_ASSERT(!constraints_for_block_aligned.max_bytes_per_row().has_value());
      ZX_DEBUG_ASSERT(!constraints_for_block_aligned.size_alignment().has_value());
      auto aligned_image_format_result = ImageConstraintsToFormat(
          constraints_for_block_aligned, block_aligned_probe_width, block_aligned_probe_height);
      if (!aligned_image_format_result.is_ok()) {
        ++result_counts.probe_image_constraints_to_format_aligned_failed_count;
        print_unexpected_failure_params("probe_image_constraints_to_format_aligned_failed",
                                        probe_size);
        if (result_counts.probe_image_constraints_to_format_aligned_failed_count >=
            kCollectUnexpectedExamplesCount) {
          ASSERT_TRUE(aligned_image_format_result.is_ok());
        }
        continue;
      }
      auto& aligned_image_format = aligned_image_format_result.value();

      uint64_t aligned_size_bytes = ImageFormatImageSize(aligned_image_format);
      if (aligned_size_bytes > with_blocks_bytes) {
        // finding this error case is the main point of this test
        ++result_counts.probe_image_with_blocks_bytes_too_low;
        print_unexpected_failure_params(
            "PaddedSizeFromBlockSize returned too-low size in bytes (test will FAIL)", probe_size);
        printf("aligned_size_bytes: 0x%" PRIx64 " with_blocks_bytes: 0x%" PRIx64 "\n",
               aligned_size_bytes, with_blocks_bytes);
        printf("stride_bytes_per_width_pixel: 0x%x %u\n", stride_bytes_per_width_pixel,
               stride_bytes_per_width_pixel);
        printf("non_aligned_size_bytes: 0x%" PRIx64 "\n", non_aligned_size_bytes);
        printf("without_blocks_bytes:   0x%" PRIx64 "\n", without_blocks_bytes);
        // try to get a few examples per run output to stdout before failing the test, in case a few
        // examples help clarify why the examples fail
        if (result_counts.probe_image_with_blocks_bytes_too_low >=
            kCollectUnexpectedExamplesCount) {
          // if we don't end up here, we'll fail at the end of the test because
          // probe_image_with_blocks_bytes_too_low isn't 0
          ASSERT_LE(aligned_size_bytes, with_blocks_bytes);
        }
        continue;
      }

      ++result_counts.probe_success_count;
    }
  }

  // these stats don't account for potential unlucky scheduling etc; just here to make sure the
  // duration of the call isn't too high, since the whole point of the way it's implemented is to
  // avoid being expensive, aside from using safemath for everything (which is probably worth it)
  printf("probe_count: %" PRIu64 " total_ms: %" PRId64 " spent_in_function_under_test_ms: %" PRIu64
         " microseconds_per_call: %" PRIu64 "\n",
         probe_count, (zx::clock::get_monotonic() - begin_time).to_msecs(),
         spent_in_function_under_test.to_msecs(),
         (spent_in_function_under_test / static_cast<int64_t>(outer_try)).to_usecs());

  // see example failing params printf-ed to stdout; we may fail the test here, or we may fail
  // when the number of failing params examples hits 10 above; this is the main failure this test
  // is trying to find
  ASSERT_EQ(0u, result_counts.probe_image_with_blocks_bytes_too_low);

  ASSERT_GT(result_counts.probe_success_count, 0u);
}

}  // namespace
