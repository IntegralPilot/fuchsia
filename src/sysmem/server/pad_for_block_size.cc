// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sysmem/server/pad_for_block_size.h"

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/image-format/image_format.h>
#include <zircon/assert.h>

#include <algorithm>
#include <cinttypes>
#include <limits>

#include <fbl/algorithm.h>
#include <fbl/string_printf.h>
#include <safemath/safe_math.h>
#include <src/sysmem/server/macros.h>

namespace sysmem_service {

// Setting pad_for_block_size requires a participant to also set max_size.width to a value less than
// 0xFFFFFFFF (and less than 0x80000000 to avoid a WARNING in the log). As long as the participant
// sets a sane value for max_size.width considering use of blocks, that'll help a lot to limit space
// overhead of using blocks.
//
// In some cases a participant may set max_size.width to 0x7FFFFFFF to avoid the WARNING and if a
// participant is doing that, it's that participant's responsibility to know what it's doing and
// avoid excessive space overhead, and/or that participant is why the space overhead is excessive.
//
// In other words, excessive VMO size is not sysmem's fault if a participant setting
// pad_for_block_size is only nominally satisfying the requirement to also set max_size.width and/or
// setting a max_size.width barely low enough to avoid the WARNING in the log.
//
// The exact worst case for pad_for_block_size is a bit subtle, in that the maximal amount of
// padding beyond BufferMemorySettings.size_bytes does not necessarily correspond to the
// maximally-wide image given the rest of the constraints. For example, if the maximally-wide image
// has min_size.height acting to pack all pixel rows of block row 0, then a somewhat narrower image
// with an additional top pixel row of block row 1 can require more padding beyond
// BufferMemorySettings.size_bytes than the maximally-wide image (at least in some cases).
//
// Adding a future constraint can only decrease the number of actually-needed padding bytes, since
// if we find the maximal presumed-needed padding bytes without that future constraint we can only
// get presumed-needed padding bytes greater than or equal to actually-needed padding bytes. The
// same goes for currently-existing constraints that we don't take into account below, as a
// less-constrained worst case is only ever at least as bad as a more-constrained worst case. In
// short, this upper bound calculation may over-allocate somewhat without causing any big problems.
// This upper-bound calculation should take into consideration all the constraints that can serve to
// significantly decrease presumed-needed padding bytes, and since actaully-needed padding bytes <=
// upper-bound padding bytes (assuming the calculation below is correct of course), we'll be ok in
// terms of participants not faulting or doing out-of-bounds DMA, and ok-enough on space overhead.
//
// This author would discourage attempting to always take every potentially-somewhat-helpful
// constraint into consideration in this calculation, as it's not necessary to avoid faults or
// out-of-bounds DMA, nor is it necessarily worth extra code complexity to save a small percent of
// space; depends on how much the particular constraint can help re. space overhead; correctness in
// terms of not faulting and not doing out of bounds DMA does not require every constraint to be
// considered.
//
// Not all candidate images in this computation are necessarily "real" in the sense of actually
// being allowed by all the constraints. See ImageConstraintsToFormat for a utility function that is
// authoritative in terms of taking all constraints into account other than
// BufferMemorySettings.size_bytes, but requires an input width and height as parameters and can
// fail if that width and height aren't possible given the constraints. See also
// ImageFormatImageSize and BufferMemorySettings.size_bytes. In particular, it's not necessarily
// indicative of a bug in this code if ImageConstraintsToFormat would reject a hypothetical image
// under consideration in the computation below.
//
// As of this comment, this author is convinced that checking an image of maximal width given
// minimum allowed image height and max_size.width, but then treating only the rows required by
// size_alignment.height as present in the non-block-aligned height, and same for maximal height
// (and taking the max), will be sufficient to get an upper bound on the difference between
// ImageFormatImageSize() with full blocks minus ImageFormatImageSize() with partial blocks across
// all allowed non-block-aligned widths and heights. Here's a rough sketch of why (please do feel
// free to attempt to find any issues with this reasoning):
//   * Elsewhere we enforce that bytes_per_row_divisor is at least the block width times
//     stride_bytes_per_width_pixel. We do this because (a) this avoids handling things differently
//     for block-using participants that intend to write vs. block-using participants that only need
//     to read, and (b) this allows us to use ImageFormatImageSize of block-aligned-up image
//     dimensions minus the ImageFormatImageSize of the non-block-aligned-up image to compute the
//     padding for a particular non-block-aligned-up width and height, and (c) this along with the
//     next bullet makes it not possible to create a situation where a large height and small width
//     requires more padding than large width and small height, given the next point.
//   * All foreseeable formats (including tiled formats) are more row major than they are column
//     major. This isn't to say that tiled formats are necessarily one or the other within each
//     tile, but to any degree that ImageFormatImageSize cares, all formats are row major.
//   * The "best" way for block usage to "waste" the most space is for the rows to be as wide as
//     possible, and for as many extra rows as possible to be needed.
//   * The min_size.height along with BufferMemorySettings.size_bytes can potentially act as the
//     most-restrictive clamp on the maximum width of the non-block-aligned images.
//   * The max_size.width can potentially act as the most-restrictive clamp on the maximum width of
//     the non-block-aligned images.
//   * If a tiled format has metadata that is much larger per row of tiles than per column of tiles,
//     and the padding_for_block_size is > 1 tile in height, then checking one additional image size
//     that's maximal height is sufficient to cover that. Normally this will be computed as less
//     padding than the large width case(s), but at least hypothetically it could result in more
//     given a format that has a lot of per-row overhead.
//
// As a refinement, we can use two image sizes around max width and two around max height. For
// clarity I'll describe in terms of max width pair of images only here. We'd consider one where we
// allow min_size.height to populate pixel rows (along with size_alignment.height), and then move to
// the next block row greater than that which is permitted by size_alignment.height, and populate
// the minimum number of pixel rows of that block row that are permitted by size_alignemnt.height,
// still taking block-aligned ImageFormatImageSize() minus non-block-algned ImageFormatImageSize()
// as the amount of extra padding needed, and taking the max of the amount of padding needed for
// these two image sizes (and the ones for max height). This might save more than one block of
// space, so it seems like it's at least marginally justified as a refinement to the prior-described
// approach.
//
// At least for now, we only allow pad_for_block_size for single-plane non-tiled formats. Re. other
// cases, the main issues are as follows:
//   * Potential for tiled format to have aspects of the overall layout which can't be made
//     transparent to other participants, requiring ImageFormat.size aligned up to a tile boundary
//     to be the tile-aligned size used by all participants to find pixel data, even those
//     participants using blocks that otherwise could be larger than a tile in one or both width and
//     height. Instead a participant using blocks bigger than tile size in either direction needs to
//     set size_alignment width and height to at least the block size in each dimension (at least
//     for now). A participant using block size less than or equal to tile size can just notice this
//     and not set pad_for_block_size because the effective size will already contain complete
//     blocks.
//   * Similarly, for multi-plane linear formats, the offset to the start of plane 1 is calculated
//     based on ImageFormat.bytes_per_row and ImageFormat.size.height. There's no way to make a
//     participant's use of blocks transparent to other participants in this case because sufficient
//     space after the last row of plane 0 needs to exist before plane 1, or block writes near the
//     bottom of plane 0 will corrupt the top of plane 1. For this reason, a producer using blocks
//     really needs the space after plane 0 to exist, or needs to guarantee that all writes to the
//     top of plane 1 will occur after writes to the bottom of plane 0. In contrast a consumer using
//     blocks could relatively easily tolerate lack of space after plane 0. As these usages of
//     blocks are more subtle, for now we don't support them, partly so the doc comments on
//     pad_for_block_size can be more clear.
//
// See the unit test for this code to actually be convinced that this stuff works.

namespace {

using safemath::CheckAdd;
using safemath::CheckDiv;
using safemath::CheckedNumeric;
using safemath::CheckMax;
using safemath::CheckMin;
using safemath::CheckMod;
using safemath::CheckMul;
using safemath::CheckSub;

using fuchsia_logging::LogSeverity::Debug;
using fuchsia_logging::LogSeverity::Error;
using fuchsia_logging::LogSeverity::Info;
using fuchsia_logging::LogSeverity::Warn;

template <typename T, typename U>
auto CheckRoundDown(T a, U b) {
  return CheckMul(CheckDiv(a, b), b);
}

template <typename T, typename U>
auto CheckRoundUp(T a, U b) {
  return CheckMul(CheckDiv(CheckAdd(a, CheckSub(b, 1)), b), b);
}

bool AccumulateMaxPaddingBytesFromHeightLowerBound(
    const CheckedNumeric<uint64_t>& height_lower_bound, fuchsia_math::SizeU block_size_param,
    const CheckedNumeric<uint64_t>& buffer_settings_size_bytes,
    const CheckedNumeric<uint64_t>& stride_bytes_per_width_pixel,
    const CheckedNumeric<uint64_t>& bytes_per_row_per_block,
    const fuchsia_sysmem2::ImageFormatConstraints& constraints,
    const fuchsia_sysmem2::ImageFormatConstraints& constraints_for_block_aligned,
    CheckedNumeric<uint64_t>& max_padding_bytes_so_far, const ComplainFunction& complain) {
  if (!height_lower_bound.IsValid()) {
    complain(FROM_HERE, Warn, "!height_lower_bound.IsValid()");
    return false;
  }
  if (!buffer_settings_size_bytes.IsValid()) {
    complain(FROM_HERE, Warn, "!buffer_settings_size_bytes.IsValid()");
    return false;
  }
  if (!stride_bytes_per_width_pixel.IsValid()) {
    complain(FROM_HERE, Warn, "!stride_bytes_per_width_pixel.IsValid()");
    return false;
  }
  if (!bytes_per_row_per_block.IsValid()) {
    complain(FROM_HERE, Warn, "!bytes_per_row_per_block.IsValid()");
    return false;
  }
  if (!max_padding_bytes_so_far.IsValid()) {
    complain(FROM_HERE, Warn, "!max_padding_bytes_so_far.IsValid()");
    return false;
  }
  const auto block_size_width = CheckedNumeric<uint64_t>(block_size_param.width());
  const auto block_size_height = CheckedNumeric<uint64_t>(block_size_param.height());
  const auto constraints_min_size_width = CheckedNumeric<uint64_t>(constraints.min_size()->width());
  const auto constraints_max_size_width = CheckedNumeric<uint64_t>(constraints.max_size()->width());
  const auto constraints_size_alignment_width =
      CheckedNumeric<uint64_t>(constraints.size_alignment()->width());
  const auto constraints_size_alignment_height =
      CheckedNumeric<uint64_t>(constraints.size_alignment()->height());
  const auto constraints_max_bytes_per_row =
      CheckedNumeric<uint64_t>(*constraints.max_bytes_per_row());
  const auto constraints_bytes_per_row_divisor =
      CheckedNumeric<uint64_t>(*constraints.bytes_per_row_divisor());

  // in pixels
  auto max_width_upper_bound = CheckedNumeric<uint64_t>(0xFFFFFFFF);

  // max width based on height_lower_bound, space within buffer_settings_size_bytes, and
  // bytes_per_row_divisor
  max_width_upper_bound =
      CheckMin(max_width_upper_bound,
               CheckDiv(CheckDiv(buffer_settings_size_bytes, stride_bytes_per_width_pixel),
                        height_lower_bound));
  max_width_upper_bound =
      CheckDiv(CheckRoundDown(CheckMul(max_width_upper_bound, stride_bytes_per_width_pixel),
                              constraints_bytes_per_row_divisor),
               stride_bytes_per_width_pixel);

  // account for max_size.width
  max_width_upper_bound = CheckMin(max_width_upper_bound, constraints_max_size_width);

  // account for max_bytes_per_row
  max_width_upper_bound = CheckMin(
      max_width_upper_bound,
      CheckDiv(CheckRoundDown(constraints_max_bytes_per_row, constraints_bytes_per_row_divisor),
               stride_bytes_per_width_pixel));

  // account for size_alignment.width
  max_width_upper_bound = CheckRoundDown(max_width_upper_bound, constraints_size_alignment_width);

  if (!max_width_upper_bound.IsValid()) {
    complain(FROM_HERE, Warn, "!max_width_upper_bound.IsValid()");
    return false;
  }
  if (!constraints_min_size_width.IsValid()) {
    complain(FROM_HERE, Warn, "!constraints_min_size_width.IsValid()");
    return false;
  }
  if (max_width_upper_bound.ValueOrDie() < constraints_min_size_width.ValueOrDie()) {
    // If this height_lower_bound can't satisfy min_size.width, this height_lower_bound isn't
    // actually possible; this is still a success case, but doesn't influence
    // max_padding_bytes_so_far
    return true;
  }

  ZX_DEBUG_ASSERT(CheckMod(height_lower_bound, constraints_size_alignment_height).ValueOrDie() ==
                  0);

  ZX_DEBUG_ASSERT(max_width_upper_bound.ValueOrDie() <= constraints_max_size_width.ValueOrDie());
  ZX_DEBUG_ASSERT(CheckMod(max_width_upper_bound, constraints_size_alignment_width).ValueOrDie() ==
                  0);
  // at this point max_width is allowed to still be an over-estimate here due to constraints not
  // taken into account above, but is not allowed to be an under-estimate here; also, the above
  // must not take block_size into account, as max_width at this point is an upper bound what
  // other non-block-using participants will think can fit within buffer_settings_size_bytes and
  // the constraints

  // we don't need to worry about partial right-most block occupancy in terms of width, because
  // bytes_per_row_divisor will already cause the non-block-aligned-up ImageFormatImageSize to see
  // the complete block width
  ZX_DEBUG_ASSERT(constraints_bytes_per_row_divisor.ValueOrDie() >=
                  bytes_per_row_per_block.ValueOrDie());
  ZX_DEBUG_ASSERT(
      CheckMod(constraints_bytes_per_row_divisor, bytes_per_row_per_block).ValueOrDie() == 0);

  // This width and height may not actually be possible per constraints.max_* fields, but we can
  // still use it to derive an upper-bound on the amount of padding needed.
  if (!max_width_upper_bound.IsValid<uint32_t>()) {
    complain(FROM_HERE, Warn, "!max_width_upper_bound.IsValid<uint32_t>()");
    return false;
  }
  if (!height_lower_bound.IsValid<uint32_t>()) {
    complain(FROM_HERE, Warn, "!height_lower_bound.IsValid<uint32_t>()");
    return false;
  }
  auto non_block_aligned_format_result =
      ImageConstraintsToFormat(constraints, max_width_upper_bound.ValueOrDie<uint32_t>(),
                               height_lower_bound.ValueOrDie<uint32_t>());
  // success is guaranteed because we've removed the max_* fields and obeyed the min_* fields;
  // this check is intended to catch any issues from adding additional constraints not yet
  // accounted for here or in the unit tests
  ZX_DEBUG_ASSERT(non_block_aligned_format_result.is_ok());
  if (!non_block_aligned_format_result.is_ok()) {
    complain(FROM_HERE, Warn, "!non_block_aligned_format_result.is_ok()");
    return false;
  }
  auto& non_block_aligned_format = non_block_aligned_format_result.value();

  auto block_aligned_width = CheckRoundUp(max_width_upper_bound, block_size_width);
  auto block_aligned_height = CheckRoundUp(height_lower_bound, block_size_height);
  if (!block_aligned_width.IsValid<uint32_t>()) {
    complain(FROM_HERE, Warn, "!block_aligned_width.IsValid<uint32_t>()");
    return false;
  }
  if (!block_aligned_height.IsValid<uint32_t>()) {
    complain(FROM_HERE, Warn, "!block_aligned_height.IsValid<uint32_t>()");
    return false;
  }
  auto block_aligned_format_result = ImageConstraintsToFormat(
      constraints_for_block_aligned, block_aligned_width.ValueOrDie<uint32_t>(),
      block_aligned_height.ValueOrDie<uint32_t>());
  ZX_DEBUG_ASSERT(block_aligned_format_result.is_ok());
  if (!block_aligned_format_result.is_ok()) {
    complain(FROM_HERE, Warn, "!block_aligned_format_result.is_ok()");
    return false;
  }
  auto& block_aligned_format = block_aligned_format_result.value();

  auto image_bytes_non_block_aligned =
      CheckedNumeric<uint64_t>(ImageFormatImageSize(non_block_aligned_format));
  auto image_bytes_block_aligned =
      CheckedNumeric<uint64_t>(ImageFormatImageSize(block_aligned_format));
  if (!image_bytes_non_block_aligned.IsValid()) {
    complain(FROM_HERE, Warn, "!image_bytes_non_block_aligned.IsValid()");
    return false;
  }
  if (!image_bytes_block_aligned.IsValid()) {
    complain(FROM_HERE, Warn, "!image_bytes_block_aligned.IsValid()");
    return false;
  }
  if (image_bytes_block_aligned.ValueOrDie() < image_bytes_non_block_aligned.ValueOrDie()) {
    // this is unexpected / supposed to be impossible
    auto log_image_format = [&complain](const fuchsia_images2::ImageFormat& image_format) {
      complain(FROM_HERE, Error,
               fbl::StringPrintf("  pixel_format: %u",
                                 static_cast<uint32_t>(*image_format.pixel_format()))
                   .c_str());
      complain(FROM_HERE, Error,
               fbl::StringPrintf("  pixel_format_modifier: 0x%" PRIx64,
                                 static_cast<uint64_t>(*image_format.pixel_format_modifier()))
                   .c_str());
      complain(FROM_HERE, Error,
               fbl::StringPrintf("  size: {%u, %u}", image_format.size()->width(),
                                 image_format.size()->height())
                   .c_str());
      complain(FROM_HERE, Error,
               fbl::StringPrintf("  bytes_per_row: %u", *image_format.bytes_per_row()).c_str());
    };
    complain(FROM_HERE, Error, "non_block_aligned_format:");
    log_image_format(non_block_aligned_format);
    complain(FROM_HERE, Error, "block_aligned_format:");
    log_image_format(block_aligned_format);
    ZX_DEBUG_ASSERT_MSG(
        image_bytes_block_aligned.ValueOrDie() >= image_bytes_non_block_aligned.ValueOrDie(),
        " -- %" PRIu64 " %" PRIu64, static_cast<uint64_t>(image_bytes_block_aligned.ValueOrDie()),
        static_cast<uint64_t>(image_bytes_non_block_aligned.ValueOrDie()));
    return false;
  }

  auto padding_bytes = CheckSub(image_bytes_block_aligned, image_bytes_non_block_aligned);

  auto new_max_padding_bytes_so_far = CheckMax(max_padding_bytes_so_far, padding_bytes);
  if (!new_max_padding_bytes_so_far.IsValid<uint64_t>()) {
    complain(FROM_HERE, Warn, "!new_max_padding_bytes_so_far.IsValid<uint64_t>()");
    return false;
  }
  max_padding_bytes_so_far = new_max_padding_bytes_so_far.ValueOrDie();

  return true;
}

}  // namespace

// See above for a rough algorithm justification / outline, and the pad_for_block_size_test.cc for
// unit tests.
//
// If you're considering making this a tighter upper bound, please be careful not to make it way
// more complicated or way slower. And warning, this can be a nerd snipe, so it may be best to walk
// away slowly.
fit::result<fit::failed, uint64_t> PaddedSizeFromBlockSize(
    const fuchsia_sysmem2::ImageFormatConstraints& constraints_param,
    uint64_t buffer_settings_size_bytes_param, const ComplainFunction& complain) {
  const auto buffer_settings_size_bytes = CheckedNumeric(buffer_settings_size_bytes_param);
  ZX_DEBUG_ASSERT(constraints_param.pad_for_block_size().has_value());
  const auto& pad_for_block_size_param = *constraints_param.pad_for_block_size();
  const auto block_size_width = CheckedNumeric<uint64_t>(pad_for_block_size_param.width());
  const auto block_size_height = CheckedNumeric<uint64_t>(pad_for_block_size_param.height());
  ZX_DEBUG_ASSERT(constraints_param.pixel_format().has_value());
  ZX_DEBUG_ASSERT(constraints_param.pixel_format_modifier().has_value());
  ZX_DEBUG_ASSERT(!constraints_param.pixel_format_and_modifiers().has_value() ||
                  constraints_param.pixel_format_and_modifiers()->empty());
  ZX_DEBUG_ASSERT(*constraints_param.pixel_format() != fuchsia_images2::PixelFormat::kDoNotCare);
  ZX_DEBUG_ASSERT(*constraints_param.pixel_format_modifier() !=
                  fuchsia_images2::PixelFormatModifier::kDoNotCare);
  ZX_DEBUG_ASSERT(constraints_param.min_size().has_value());
  const auto& constraints_min_size_param = *constraints_param.min_size();
  const auto constraints_min_size_height =
      CheckedNumeric<uint64_t>(constraints_min_size_param.height());
  ZX_DEBUG_ASSERT(constraints_param.max_size().has_value());
  const auto& constraints_max_size_param = *constraints_param.max_size();
  const auto constraints_max_size_width =
      CheckedNumeric<uint64_t>(constraints_max_size_param.width());
  const auto constraints_max_size_height =
      CheckedNumeric<uint64_t>(constraints_max_size_param.height());
  ZX_DEBUG_ASSERT(constraints_param.size_alignment().has_value());
  const auto& constraints_size_alignment_param = *constraints_param.size_alignment();
  const auto constraints_size_alignment_height =
      CheckedNumeric<uint64_t>(constraints_size_alignment_param.height());
  ZX_DEBUG_ASSERT(constraints_param.max_bytes_per_row().has_value());
  auto pixel_format_and_modifier = PixelFormatAndModifierFromConstraints(constraints_param);
  auto stride_bytes_per_width_pixel =
      CheckedNumeric<uint64_t>(ImageFormatStrideBytesPerWidthPixel(pixel_format_and_modifier));
  auto bytes_per_row_per_block = CheckMul(stride_bytes_per_width_pixel, block_size_width);
  ZX_DEBUG_ASSERT(constraints_param.bytes_per_row_divisor().has_value());
  auto constraints_bytes_per_row_divisor =
      CheckedNumeric<uint64_t>(*constraints_param.bytes_per_row_divisor());
  ZX_DEBUG_ASSERT(constraints_bytes_per_row_divisor.ValueOrDie() >=
                  bytes_per_row_per_block.ValueOrDie());
  ZX_DEBUG_ASSERT(
      CheckMod(constraints_bytes_per_row_divisor, bytes_per_row_per_block).ValueOrDie() == 0);
  // The lower this value specified by the client, the faster the search for max width below will
  // be.
  ZX_DEBUG_ASSERT(constraints_max_size_width.ValueOrDie() <= 0xFFFFFFFE);

  // intentional copy/clone; no_upper_bounds_constraints is to be able to call
  // ImageConstraintsToFormat for width and height that are aligned up to block size without hitting
  // caps defined by "max_*" constraint fields
  auto constraints_for_block_aligned = constraints_param;
  constraints_for_block_aligned.max_size().reset();
  constraints_for_block_aligned.max_bytes_per_row().reset();
  constraints_for_block_aligned.max_width_times_height().reset();
  // the official size will still satisfy size_alignment, but the padding for pad_for_block_size is
  // just padding, so the padding-for-blocks calculation can align up to block size as part of its
  // computation without the block size needing to be <= size_alignment
  constraints_for_block_aligned.size_alignment().reset();

  CheckedNumeric<uint64_t> max_padding_so_far = 0;

  // first, the widest image
  //
  // The "widest image" is an image with width >= max achievable width given constraints and
  // buffer_settings_size_bytes, and height <= min achievable height.
  CheckedNumeric<uint64_t> min_height_lower_bound = 0;
  {
    // in pixels
    min_height_lower_bound = CheckMax(min_height_lower_bound, constraints_min_size_height);
    min_height_lower_bound =
        CheckRoundUp(min_height_lower_bound, constraints_size_alignment_height);
    if (!min_height_lower_bound.IsValid<uint32_t>()) {
      complain(FROM_HERE, Warn, "!min_height_lower_bound.IsValid<uint32_t>()");
      return fit::failed();
    }
    ZX_DEBUG_ASSERT(min_height_lower_bound.ValueOrDie() >=
                    constraints_min_size_height.ValueOrDie());
    ZX_DEBUG_ASSERT(
        CheckMod(min_height_lower_bound, constraints_size_alignment_height).ValueOrDie() == 0);
    // at this point min_height is allowed to be an under-estimate, but not an over-estimate

    // AccumulateMaxPaddingBytesFromHeightLowerBound checks IsValid() for all
    // CheckedNumeric parameters.
    if (!AccumulateMaxPaddingBytesFromHeightLowerBound(
            min_height_lower_bound, pad_for_block_size_param, buffer_settings_size_bytes,
            stride_bytes_per_width_pixel, bytes_per_row_per_block, constraints_param,
            constraints_for_block_aligned, max_padding_so_far, complain)) {
      complain(FROM_HERE, Warn, "!AccumulateMaxPaddingBytesFromHeightLowerBound()");
      return fit::failed();
    }
    // max_padding_so_far can still be 0 here if the max width has the last block row fully
    // populated with valid pixels; this is among the reasons the next-widest block exists below
  }

  // second, next-widest (in blocks) image
  //
  // The "next-widest (in blocks) image" is the image with a new height greater than
  // min_height_lower_bound, and where the new height is the lowest height that's in a block after
  // min_height_lower_bound's last block, and where the new height is also permitted by
  // size_alignment.height, such that padding calculated here is an upper bound assuming the new
  // height. However, this new height may not be possible given other constraints, in which case
  // AccumulateMaxPaddingBytesFromHeightLowerBound may return true without touching
  // max_padding_so_far in this block.
  {
    // This gets to the last pixel row of the last occupied block, then to the base row of that
    // block, then to the base of row 0 of the next block, then adds 1 to count that row. The next
    // statement will round up to the lowest actually-available count of rows per
    // constraints_size_alignment_height.
    auto second_lowest_height_lower_bound =
        CheckAdd(CheckAdd(CheckRoundDown(CheckSub(min_height_lower_bound, 1), block_size_height),
                          block_size_height),
                 1);
    second_lowest_height_lower_bound =
        CheckRoundUp(second_lowest_height_lower_bound, constraints_size_alignment_height);
    if (!second_lowest_height_lower_bound.IsValid<uint32_t>()) {
      complain(FROM_HERE, Warn, "!second_lowest_height_lower_bound.IsValid<uint32_t>()");
      return fit::failed();
    }

    // We won't always have a second height to try, if the second height would exceed
    // max_size.height.
    if (second_lowest_height_lower_bound.ValueOrDie() <= constraints_max_size_height.ValueOrDie()) {
      if (!AccumulateMaxPaddingBytesFromHeightLowerBound(
              second_lowest_height_lower_bound, pad_for_block_size_param,
              buffer_settings_size_bytes, stride_bytes_per_width_pixel, bytes_per_row_per_block,
              constraints_param, constraints_for_block_aligned, max_padding_so_far, complain)) {
        complain(FROM_HERE, Warn, "!AccumulateMaxPaddingBytesFromHeightLowerBound()");
        return fit::failed();
      }
    }
  }

  // Since we only currently support linear single-plane formats for now, we can stop here for now.
  // For linear single-plane images these steps aren't necessary because extra rows cost more
  // padding than extra columns, given that bytes_per_row is already included in the
  // non-block-aligned image size in bytes.
  //
  // third, max height image (similar to above for max width)
  //
  // fourth, check next-tallest image (similar to above for "next-widest" image)

  auto result = CheckAdd(buffer_settings_size_bytes, max_padding_so_far);
  if (!result.IsValid<uint64_t>()) {
    complain(FROM_HERE, Warn, "!result.IsValid<uint64_t>()");
    return fit::failed();
  }
  return fit::ok(result.ValueOrDie());
}

}  // namespace sysmem_service
