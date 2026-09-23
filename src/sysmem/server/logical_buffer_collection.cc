// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "logical_buffer_collection.h"

#include <dirent.h>
#include <fcntl.h>
#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <inttypes.h>
#include <lib/async_patterns/cpp/task_scope.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/fidl/cpp/wire/status.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/fit/defer.h>
#include <lib/fpromise/result.h>
#include <lib/image-format/image_format.h>
#include <lib/memory_barriers/memory_barriers.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/trace/event.h>
#include <lib/zx/channel.h>
#include <lib/zx/clock.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <limits>  // std::numeric_limits
#include <numeric>
#include <type_traits>
#include <unordered_map>
#include <vector>

#include <bind/fuchsia/sysmem/heap/cpp/bind.h>
#include <fbl/algorithm.h>
#include <fbl/ref_ptr.h>
#include <fbl/string.h>
#include <fbl/string_printf.h>
#include <safemath/safe_math.h>

#include "src/sysmem/server/buffer_collection.h"
#include "src/sysmem/server/buffer_collection_token.h"
#include "src/sysmem/server/buffer_collection_token_group.h"
#include "src/sysmem/server/koid_util.h"
#include "src/sysmem/server/logging.h"
#include "src/sysmem/server/macros.h"
#include "src/sysmem/server/node_properties.h"
#include "src/sysmem/server/orphaned_node.h"
#include "src/sysmem/server/pad_for_block_size.h"
#include "src/sysmem/server/sysmem.h"
#include "src/sysmem/server/usage_pixel_format_cost.h"

using safemath::CheckAdd;
using safemath::CheckDiv;
using safemath::CheckMul;
using safemath::CheckSub;

namespace sysmem_service {

namespace {

using Error = fuchsia_sysmem2::Error;

// Sysmem is creating the VMOs, so sysmem can have all the rights and just not
// mis-use any rights.  Remove ZX_RIGHT_EXECUTE though.
const uint32_t kSysmemVmoRights = ZX_DEFAULT_VMO_RIGHTS & ~ZX_RIGHT_EXECUTE;
// 1 GiB cap for now.
const uint64_t kMaxTotalSizeBytesPerCollection = 1ull * 1024 * 1024 * 1024;
// 256 MiB cap for now.
const uint64_t kMaxSizeBytesPerBuffer = 256ull * 1024 * 1024;
// Give up on attempting to aggregate constraints after exactly this many group
// child combinations have been attempted.  This prevents sysmem getting stuck
// trying too many combinations.
const uint32_t kMaxGroupChildCombinations = 64;

// Map of all supported color spaces to an unique semi arbitrary number. A higher number means
// that the color space is less desirable and a lower number means that a color space is more
// desirable.
const std::unordered_map<sysmem::FidlUnderlyingTypeOrType_t<fuchsia_images2::ColorSpace>, uint32_t>
    kColorSpaceRanking = {
        {sysmem::fidl_underlying_cast(fuchsia_images2::ColorSpace::kInvalid),
         std::numeric_limits<uint32_t>::max()},
        {sysmem::fidl_underlying_cast(fuchsia_images2::ColorSpace::kSrgb), 1},
        {sysmem::fidl_underlying_cast(fuchsia_images2::ColorSpace::kRec601Ntsc), 8},
        {sysmem::fidl_underlying_cast(fuchsia_images2::ColorSpace::kRec601NtscFullRange), 7},
        {sysmem::fidl_underlying_cast(fuchsia_images2::ColorSpace::kRec601Pal), 6},
        {sysmem::fidl_underlying_cast(fuchsia_images2::ColorSpace::kRec601PalFullRange), 5},
        {sysmem::fidl_underlying_cast(fuchsia_images2::ColorSpace::kRec709), 4},
        {sysmem::fidl_underlying_cast(fuchsia_images2::ColorSpace::kRec2020), 3},
        {sysmem::fidl_underlying_cast(fuchsia_images2::ColorSpace::kRec2100), 2},
        {sysmem::fidl_underlying_cast(fuchsia_images2::ColorSpace::kPassthrough), 9}};

template <typename T>
bool IsNonZeroPowerOf2(T value) {
  static_assert(std::is_integral_v<T>);
  if (!value) {
    return false;
  }
  if (value & (value - 1)) {
    return false;
  }
  return true;
}

// TODO(https://fxbug.dev/42127687): It'd be nice if this could be a function template over FIDL
// scalar field types.
#define FIELD_DEFAULT_1(table_ref_name, field_name)                                         \
  do {                                                                                      \
    auto& table_ref = (table_ref_name);                                                     \
    static_assert(IsNaturalFidlTable<std::remove_reference_t<decltype(table_ref)>>::value); \
    using FieldType = std::remove_reference_t<decltype((table_ref.field_name().value()))>;  \
    if (!table_ref.field_name().has_value()) {                                              \
      table_ref.field_name().emplace(safe_cast<FieldType>(1));                              \
      ZX_DEBUG_ASSERT(table_ref.field_name().value() == 1);                                 \
    }                                                                                       \
    ZX_DEBUG_ASSERT(table_ref.field_name().has_value());                                    \
  } while (false)

// TODO(https://fxbug.dev/42127687): It'd be nice if this could be a function template over FIDL
// scalar field types.
#define FIELD_DEFAULT_MAX(table_ref_name, field_name)                                           \
  do {                                                                                          \
    auto& table_ref = (table_ref_name);                                                         \
    static_assert(IsNaturalFidlTable<std::remove_reference_t<decltype(table_ref)>>::value);     \
    using FieldType = std::remove_reference_t<decltype((table_ref.field_name().value()))>;      \
    if (!table_ref.field_name().has_value()) {                                                  \
      table_ref.field_name().emplace(std::numeric_limits<FieldType>::max());                    \
      ZX_DEBUG_ASSERT(table_ref.field_name().value() == std::numeric_limits<FieldType>::max()); \
    }                                                                                           \
    ZX_DEBUG_ASSERT(table_ref.field_name().has_value());                                        \
  } while (false)

// TODO(https://fxbug.dev/42127687): It'd be nice if this could be a function template over FIDL
// scalar field types.
#define FIELD_DEFAULT_ZERO(table_ref_name, field_name)                                      \
  do {                                                                                      \
    auto& table_ref = (table_ref_name);                                                     \
    static_assert(IsNaturalFidlTable<std::remove_reference_t<decltype(table_ref)>>::value); \
    using FieldType = std::remove_reference_t<decltype((table_ref.field_name().value()))>;  \
    using UnderlyingType = sysmem::FidlUnderlyingTypeOrType_t<FieldType>;                   \
    if (!table_ref.field_name().has_value()) {                                              \
      table_ref.field_name().emplace(safe_cast<FieldType>(safe_cast<UnderlyingType>(0)));   \
      ZX_DEBUG_ASSERT(0 == safe_cast<UnderlyingType>(table_ref.field_name().value()));      \
    }                                                                                       \
    ZX_DEBUG_ASSERT(table_ref.field_name().has_value());                                    \
  } while (false)

// TODO(https://fxbug.dev/42127687): It'd be nice if this could be a function template over FIDL
// scalar field types.
#define FIELD_DEFAULT_ZERO_64_BIT(table_ref_name, field_name)                               \
  do {                                                                                      \
    auto& table_ref = (table_ref_name);                                                     \
    static_assert(IsNaturalFidlTable<std::remove_reference_t<decltype(table_ref)>>::value); \
    using FieldType = std::remove_reference_t<decltype((table_ref.field_name().value()))>;  \
    using UnderlyingType = sysmem::FidlUnderlyingTypeOrType_t<FieldType>;                   \
    if (!table_ref.field_name().has_value()) {                                              \
      table_ref.field_name().emplace(safe_cast<FieldType>(0));                              \
      ZX_DEBUG_ASSERT(0 == safe_cast<UnderlyingType>(table_ref.field_name().value()));      \
    }                                                                                       \
    ZX_DEBUG_ASSERT(table_ref.field_name().has_value());                                    \
  } while (false)

#define FIELD_DEFAULT_FALSE(table_ref_name, field_name)                                     \
  do {                                                                                      \
    auto& table_ref = (table_ref_name);                                                     \
    static_assert(IsNaturalFidlTable<std::remove_reference_t<decltype(table_ref)>>::value); \
    using FieldType = std::remove_reference_t<decltype((table_ref.field_name().value()))>;  \
    static_assert(std::is_same_v<FieldType, bool>);                                         \
    if (!table_ref.field_name().has_value()) {                                              \
      table_ref.field_name().emplace(false);                                                \
      ZX_DEBUG_ASSERT(!table_ref.field_name().value());                                     \
    }                                                                                       \
    ZX_DEBUG_ASSERT(table_ref.field_name().has_value());                                    \
  } while (false)

template <typename Type>
class IsStdVector : public std::false_type {};
template <typename Type>
class IsStdVector<std::vector<Type>> : public std::true_type {};
template <typename Type>
inline constexpr bool IsStdVector_v = IsStdVector<Type>::value;

static_assert(IsStdVector_v<std::vector<uint32_t>>);
static_assert(!IsStdVector_v<uint32_t>);

template <typename Type, typename enable = void>
class IsStdString : public std::false_type {};
template <typename Type>
class IsStdString<Type, std::enable_if_t<std::is_same_v<std::string, std::decay_t<Type>>>>
    : public std::true_type {};
template <typename Type>
inline constexpr bool IsStdString_v = IsStdString<Type>::value;

static_assert(IsStdString_v<std::string>);
static_assert(!IsStdString_v<uint32_t>);

#define FIELD_DEFAULT(table_ref_name, field_name, value_name)                               \
  do {                                                                                      \
    auto& table_ref = (table_ref_name);                                                     \
    static_assert(IsNaturalFidlTable<std::remove_reference_t<decltype(table_ref)>>::value); \
    using FieldType = std::remove_reference_t<decltype((table_ref.field_name().value()))>;  \
    static_assert(!fidl::IsFidlObject<FieldType>::value);                                   \
    static_assert(!IsStdVector_v<FieldType>);                                               \
    static_assert(!IsStdString_v<FieldType>);                                               \
    if (!table_ref.field_name().has_value()) {                                              \
      auto field_value = (value_name);                                                      \
      table_ref.field_name().emplace(field_value);                                          \
      ZX_DEBUG_ASSERT(table_ref.field_name().value() == field_value);                       \
    }                                                                                       \
    ZX_DEBUG_ASSERT(table_ref.field_name().has_value());                                    \
  } while (false)

#define FIELD_DEFAULT_SET(table_ref_name, field_name)                                       \
  do {                                                                                      \
    auto& table_ref = (table_ref_name);                                                     \
    static_assert(IsNaturalFidlTable<std::remove_reference_t<decltype(table_ref)>>::value); \
    using TableType = std::remove_reference_t<decltype((table_ref.field_name().value()))>;  \
    static_assert(IsNaturalFidlTable<TableType>::value);                                    \
    if (!table_ref.field_name().has_value()) {                                              \
      table_ref.field_name().emplace();                                                     \
    }                                                                                       \
    ZX_DEBUG_ASSERT(table_ref.field_name().has_value());                                    \
  } while (false)

// regardless of capacity, initial count is always 0
#define FIELD_DEFAULT_SET_VECTOR(table_ref_name, field_name, capacity_param)                     \
  do {                                                                                           \
    auto& table_ref = (table_ref_name);                                                          \
    static_assert(IsNaturalFidlTable<std::remove_reference_t<decltype(table_ref)>>::value);      \
    using VectorFieldType = std::remove_reference_t<decltype((table_ref.field_name().value()))>; \
    static_assert(IsStdVector_v<VectorFieldType>);                                               \
    if (!table_ref.field_name().has_value()) {                                                   \
      size_t capacity = (capacity_param);                                                        \
      table_ref.field_name().emplace(0);                                                         \
      table_ref.field_name().value().reserve(capacity);                                          \
      ZX_DEBUG_ASSERT(table_ref.field_name().value().capacity() >= capacity);                    \
    }                                                                                            \
    ZX_DEBUG_ASSERT(table_ref.field_name().has_value());                                         \
  } while (false)

bool IsPotentiallyIncludedInInitialAllocation(const NodeProperties& node) {
  bool potentially_included_in_initial_allocation = true;
  for (const NodeProperties* iter = &node; iter; iter = iter->parent()) {
    if (iter->error_propagation_mode() == ErrorPropagationMode::kDoNotPropagate) {
      potentially_included_in_initial_allocation = false;
    }
  }
  return potentially_included_in_initial_allocation;
}

fit::result<zx_status_t, bool> IsColorSpaceArrayDoNotCare(
    const std::vector<fuchsia_images2::ColorSpace>& x) {
  if (x.size() == 0) {
    return fit::error(ZX_ERR_INVALID_ARGS);
  }
  uint32_t do_not_care_count = 0;
  for (uint32_t i = 0; i < x.size(); ++i) {
    if (x[i] == fuchsia_images2::ColorSpace::kDoNotCare) {
      ++do_not_care_count;
    }
  }
  if (do_not_care_count >= 1 && x.size() != 1) {
    return fit::error(ZX_ERR_INVALID_ARGS);
  }
  ZX_DEBUG_ASSERT(do_not_care_count <= 1);
  return fit::ok(do_not_care_count != 0);
}

// true iff either field is DoNotCare
[[nodiscard]] bool IsPixelFormatAndModifierAtLeastPartlyDoNotCare(const PixelFormatAndModifier& a) {
  return a.pixel_format == fuchsia_images2::PixelFormat::kDoNotCare ||
         a.pixel_format_modifier == fuchsia_images2::PixelFormatModifier::kDoNotCare;
}

[[nodiscard]] PixelFormatAndModifier CombinePixelFormatAndModifier(
    const PixelFormatAndModifier& a, const PixelFormatAndModifier& b) {
  PixelFormatAndModifier result = a;
  if (result.pixel_format == fuchsia_images2::PixelFormat::kDoNotCare) {
    // Can be a specific value, or kDoNotCare.
    result.pixel_format = b.pixel_format;
  }
  if (result.pixel_format_modifier == fuchsia_images2::PixelFormatModifier::kDoNotCare) {
    // Can be a specific value, or kFormatModifierDoNotCare
    result.pixel_format_modifier = b.pixel_format_modifier;
  }
  return result;
}

[[nodiscard]] fit::result<std::monostate, uint32_t> LeastCommonMultiple(uint32_t a, uint32_t b) {
  ZX_DEBUG_ASSERT(a != 0);
  ZX_DEBUG_ASSERT(b != 0);
  // this computes the least common multiple without factoring the numbers; by doing this with
  // uint64_t, we avoid undefined behavior of std::lcm given that a and b are uint32_t, but then we
  // need to check whether the result fits in uint32_t
  uint64_t lcm = std::lcm<uint64_t, uint64_t>(a, b);
  auto result = safemath::CheckedNumeric<uint64_t>(lcm).Cast<uint32_t>();
  if (!result.IsValid()) {
    return fit::error(std::monostate{});
  }
  return fit::ok(result.ValueOrDie());
}

[[nodiscard]] fit::result<std::monostate, uint32_t> CombineBytesPerRowDivisor(uint32_t a,
                                                                              uint32_t b) {
  return LeastCommonMultiple(a, b);
}

[[nodiscard]] bool IsPixelFormatAndModifierCombineable(const PixelFormatAndModifier& a,
                                                       const PixelFormatAndModifier& b) {
  bool is_pixel_format_combineable = a.pixel_format == b.pixel_format ||
                                     a.pixel_format == fuchsia_images2::PixelFormat::kDoNotCare ||
                                     b.pixel_format == fuchsia_images2::PixelFormat::kDoNotCare;
  bool is_pixel_format_and_modifier_combineable =
      a.pixel_format_modifier == b.pixel_format_modifier ||
      a.pixel_format_modifier == fuchsia_images2::PixelFormatModifier::kDoNotCare ||
      b.pixel_format_modifier == fuchsia_images2::PixelFormatModifier::kDoNotCare;
  if (!is_pixel_format_combineable || !is_pixel_format_and_modifier_combineable) {
    return false;
  }
  auto provisional_combined = CombinePixelFormatAndModifier(a, b);
  if (!IsPixelFormatAndModifierAtLeastPartlyDoNotCare(provisional_combined) &&
      !ImageFormatIsSupported(provisional_combined)) {
    // To be combine-able, a resulting specific (not DoNotCare) format must be a supported format.
    //
    // This is in contrast to a client explicitly specifying an unsupported format, which causes
    // allocation failure.
    return false;
  }
  return true;
}

[[nodiscard]] bool ReplicateAnyPixelFormatAndModifierDoNotCares(
    const std::vector<fuchsia_sysmem2::ImageFormatConstraints>& to_match,
    std::vector<fuchsia_sysmem2::ImageFormatConstraints>& to_update) {
  // Given the number of distinct valid values for each field as of this comment, reaching this many
  // entries would require a caller to send duplicate entries (already checked and rejected
  // previously), or to send invalid values for pixel_format or pixel_format_modifier.
  constexpr size_t kMaxItemCount = 512;

  std::vector<fuchsia_sysmem2::ImageFormatConstraints> to_append;

  for (size_t ui = 0; ui < to_update.size();) {
    auto u_pixel_format_and_modifier = PixelFormatAndModifierFromConstraints(to_update[ui]);
    if (!IsPixelFormatAndModifierAtLeastPartlyDoNotCare(u_pixel_format_and_modifier)) {
      // this entry has no DoNotCare; leave in to_update
      ++ui;
      continue;
    }

    // If to_match also has DoNotCare (rather than a set of specific values), we'll basically move
    // the to_update DoNotCare entry from to_update to to_append and then back to to_update, which
    // doesn't change anything overall, but also doesn't seem performance-costly enough to justify
    // extra code here to avoid.
    //
    // If to_match has no entries combine-able with to_fan_out, this will result in to_fan_out being
    // removed and not replaced; this is fine since to_match has no entry that can work with
    // to_fan_out. However, note that the caller still needs its own filtering to remove items that
    // can't combine, since we're only processing items with DoNotCare here.

    auto to_fan_out = std::move(to_update[ui]);

    // to_fan_out's information is effectively being moved (along with fanning out) to to_append, so
    // move down the last item (if this isn't already the last) to keep to_update packed
    if (ui != to_update.size() - 1) {
      to_update[ui] = std::move(to_update[to_update.size() - 1]);
    }
    to_update.resize(to_update.size() - 1);

    for (size_t mi = 0; mi < to_match.size(); ++mi) {
      auto m_pixel_format_and_modifier = PixelFormatAndModifierFromConstraints(to_match[mi]);
      if (!IsPixelFormatAndModifierCombineable(m_pixel_format_and_modifier,
                                               u_pixel_format_and_modifier)) {
        continue;
      }

      // intentional copy/clone
      auto cloned_entry = to_fan_out;
      auto combined_pixel_format_and_modifier =
          CombinePixelFormatAndModifier(u_pixel_format_and_modifier, m_pixel_format_and_modifier);
      cloned_entry.pixel_format() = combined_pixel_format_and_modifier.pixel_format;
      cloned_entry.pixel_format_modifier() =
          combined_pixel_format_and_modifier.pixel_format_modifier;

      // Since combined_pixel_format_and_modifier is the result of DoNotCare fanout, we want to
      // filter out color spaces that aren't supported with the format, to avoid a fanout format
      // causing allocation failure when color spaces are checked later. If zero color spaces remain
      // here we can just filter out the cloned_entry here by not adding to to_append.
      auto is_color_space_do_not_care_result =
          IsColorSpaceArrayDoNotCare(*cloned_entry.color_spaces());
      // checked previously
      ZX_DEBUG_ASSERT(is_color_space_do_not_care_result.is_ok());
      bool is_color_space_do_not_care = is_color_space_do_not_care_result.value();
      if (!IsPixelFormatAndModifierAtLeastPartlyDoNotCare(combined_pixel_format_and_modifier) &&
          !is_color_space_do_not_care) {
        auto& color_spaces = *cloned_entry.color_spaces();
        for (uint32_t ci = 0; ci < color_spaces.size();) {
          auto color_space = color_spaces[ci];
          if (!ImageFormatIsSupportedColorSpaceForPixelFormat(color_space,
                                                              combined_pixel_format_and_modifier)) {
            // filter out color space
            if (ci != color_spaces.size() - 1) {
              color_spaces[ci] = color_spaces[color_spaces.size() - 1];
            }
            color_spaces.resize(color_spaces.size() - 1);
            continue;
          }
          ++ci;
        }
        if (color_spaces.empty()) {
          // If a client is triggering this, the client may want to consider using ColorSpace
          // kDoNotCare instead (if the client really doesn't care). But if a client really does
          // care about the specific ColorSpace(s), it's fine to keep leaning on this filtering.
          //
          // If this gets too noisy we can remove this log output.
          LOG(INFO,
              "omitting DoNotCare fanout format because zero remaining color spaces supported with format: %u 0x%" PRIx64,
              static_cast<uint32_t>(combined_pixel_format_and_modifier.pixel_format),
              fidl::ToUnderlying(combined_pixel_format_and_modifier.pixel_format_modifier));
          continue;
        }
      }

      if (to_append.size() + to_update.size() >= kMaxItemCount) {
        LOG(ERROR, "too many entries; caller sending invalid values?");
        return false;
      }

      to_append.emplace_back(std::move(cloned_entry));
    }

    // intentionally don't ++ui, since this entry has changed, so still needs to be processed (or
    // ui == size() in which case we're done)
  }

  for (auto& to_move : to_append) {
    to_update.emplace_back(std::move(to_move));
  }

  return true;
}

void ReplicateColorSpaceDoNotCare(const std::vector<fuchsia_images2::ColorSpace>& to_match,
                                  std::vector<fuchsia_images2::ColorSpace>& to_update) {
  // error result excluded by caller
  ZX_DEBUG_ASSERT(!IsColorSpaceArrayDoNotCare(to_match).value());
  // error result excluded by caller
  ZX_DEBUG_ASSERT(IsColorSpaceArrayDoNotCare(to_update).value());
  ZX_DEBUG_ASSERT(to_update.size() == 1);
  if (to_match.empty()) {
    to_update.resize(0);
    return;
  }
  ZX_DEBUG_ASSERT(!to_match.empty());
  to_update.resize(to_match.size());
  for (uint32_t i = 0; i < to_match.size(); ++i) {
    to_update[i] = to_match[i];
  }
  ZX_DEBUG_ASSERT(to_update.size() == to_match.size());
  ZX_DEBUG_ASSERT(!to_update.empty());
  ZX_DEBUG_ASSERT(!to_match.empty());
  ZX_DEBUG_ASSERT(to_update[0] == to_match[0]);
}

TokenServerEndCombinedV1AndV2 ConvertV1TokenRequestToCombinedTokenRequest(
    TokenServerEndV1 token_server_end_v1) {
  // This is the only place we convert from a V1 token request to a combined V1 and V2 internal
  // "request".
  return TokenServerEndCombinedV1AndV2(token_server_end_v1.TakeChannel());
}

TokenServerEndCombinedV1AndV2 ConvertV2TokenRequestToCombinedTokenRequest(
    TokenServerEndV2 token_server_end_v2) {
  // This is the only place we convert from a V2 token request to a combined V1 and V2 internal
  // "request".
  return TokenServerEndCombinedV1AndV2(token_server_end_v2.TakeChannel());
}

bool IsSecureRequired(const fuchsia_sysmem2::BufferCollectionConstraints& constraints) {
  if (!constraints.buffer_memory_constraints().has_value()) {
    return false;
  }
  auto& bmc = *constraints.buffer_memory_constraints();
  if (!bmc.secure_required().has_value()) {
    return false;
  }
  return *bmc.secure_required();
}

bool HasAlphaOrX(fuchsia_images2::PixelFormat pixel_format) {
  ZX_DEBUG_ASSERT(pixel_format != fuchsia_images2::PixelFormat::kDoNotCare);
  ZX_DEBUG_ASSERT(pixel_format != fuchsia_images2::PixelFormat::kInvalid);
  switch (pixel_format) {
    case fuchsia_images2::PixelFormat::kR8G8B8A8:
    case fuchsia_images2::PixelFormat::kR8G8B8X8:
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
    case fuchsia_images2::PixelFormat::kB8G8R8X8:
    case fuchsia_images2::PixelFormat::kR2G2B2X2:
    case fuchsia_images2::PixelFormat::kA2R10G10B10:
    case fuchsia_images2::PixelFormat::kA2B10G10R10:
      return true;
    case fuchsia_images2::PixelFormat::kI420:
    case fuchsia_images2::PixelFormat::kM420:
    case fuchsia_images2::PixelFormat::kNv12:
    case fuchsia_images2::PixelFormat::kYuy2:
    case fuchsia_images2::PixelFormat::kMjpeg:
    case fuchsia_images2::PixelFormat::kYv12:
    case fuchsia_images2::PixelFormat::kB8G8R8:
    case fuchsia_images2::PixelFormat::kR5G6B5:
    case fuchsia_images2::PixelFormat::kR3G3B2:
    case fuchsia_images2::PixelFormat::kL8:
    case fuchsia_images2::PixelFormat::kR8:
    case fuchsia_images2::PixelFormat::kR8G8:
    case fuchsia_images2::PixelFormat::kP010:
    case fuchsia_images2::PixelFormat::kR8G8B8:
      return false;
    default:
      LOG(ERROR, "HasAlphaOrX missing case for value: %u", fidl::ToUnderlying(pixel_format));
      return false;
  }
}

}  // namespace

// static
fbl::RefPtr<LogicalBufferCollection> LogicalBufferCollection::CommonCreate(
    Sysmem* parent_sysmem, const ClientDebugInfo* client_debug_info) {
  fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection =
      fbl::AdoptRef<LogicalBufferCollection>(new LogicalBufferCollection(parent_sysmem));
  // The existence of a channel-owned BufferCollectionToken adds a
  // fbl::RefPtr<> ref to LogicalBufferCollection.
  logical_buffer_collection->LogInfo(FROM_HERE, "LogicalBufferCollection::Create()");
  logical_buffer_collection->root_ =
      NodeProperties::NewRoot(logical_buffer_collection.get(), client_debug_info);
  return logical_buffer_collection;
}

// static
void LogicalBufferCollection::CreateV1(TokenServerEndV1 buffer_collection_token_request,
                                       Sysmem* parent_sysmem,
                                       const ClientDebugInfo* client_debug_info) {
  fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection =
      LogicalBufferCollection::CommonCreate(parent_sysmem, client_debug_info);
  logical_buffer_collection->CreateBufferCollectionTokenV1(
      logical_buffer_collection, logical_buffer_collection->root_.get(),
      std::move(buffer_collection_token_request));
}

// static
void LogicalBufferCollection::CreateV2(TokenServerEndV2 buffer_collection_token_request,
                                       Sysmem* parent_sysmem,
                                       const ClientDebugInfo* client_debug_info) {
  fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection =
      LogicalBufferCollection::CommonCreate(parent_sysmem, client_debug_info);
  logical_buffer_collection->CreateBufferCollectionTokenV2(
      logical_buffer_collection, logical_buffer_collection->root_.get(),
      std::move(buffer_collection_token_request));
}

// static
fit::result<zx_status_t, BufferCollectionToken*> LogicalBufferCollection::CommonConvertToken(
    Sysmem* parent_sysmem, zx::channel buffer_collection_token,
    const ClientDebugInfo* client_debug_info, const char* fidl_message_name) {
  ZX_DEBUG_ASSERT(buffer_collection_token);

  zx_koid_t token_client_koid;
  zx_koid_t token_server_koid;
  zx_status_t status = get_handle_koids(buffer_collection_token, &token_client_koid,
                                        &token_server_koid, ZX_OBJ_TYPE_CHANNEL);
  if (status != ZX_OK) {
    LogErrorStatic(FROM_HERE, client_debug_info, "Failed to get channel koids - %d", status);
    // ~buffer_collection_token
    return fit::error(ZX_ERR_INVALID_ARGS);
  }

  BufferCollectionToken* token = parent_sysmem->FindTokenByServerChannelKoid(token_server_koid);
  if (!token) {
    // The most likely scenario for why the token was not found is that Sync() was not called on
    // either the BufferCollectionToken or the BufferCollection.
    LogErrorStatic(FROM_HERE, client_debug_info,
                   "BindSharedCollection could not find token from server channel koid %ld; "
                   "perhaps BufferCollectionToken.Sync() was not called",
                   token_server_koid);
    // ~buffer_collection_token
    return fit::error(ZX_ERR_NOT_FOUND);
  }

  return fit::success(token);
}

fit::result<zx_status_t, std::optional<zx::vmo>> LogicalBufferCollection::CreateWeakVmo(
    uint32_t buffer_index, const ClientDebugInfo& client_debug_info) {
  auto buffers_iter = buffers_.find(buffer_index);
  if (buffers_iter == buffers_.end()) {
    // success, but no VMO
    return fit::ok(std::nullopt);
  }
  auto& buffer = buffers_iter->second;

  return buffer->CreateWeakVmo(client_debug_info);
}

fit::result<zx_status_t, std::optional<zx::eventpair>>
LogicalBufferCollection::DupCloseWeakAsapClientEnd(uint32_t buffer_index) {
  auto buffer_iter = buffers_.find(buffer_index);
  if (buffer_iter == buffers_.end()) {
    // Success but no eventpair needed since no weak VMO will be provided from CreateWeakVmo().
    return fit::ok(std::nullopt);
  }
  auto& buffer = buffer_iter->second;
  auto dup_result = buffer->DupCloseWeakAsapClientEnd();
  if (dup_result.is_error()) {
    LogError(FROM_HERE, "DupCloseWeakAsapClientEnd() failed: %d", dup_result.error_value());
    return dup_result.take_error();
  }
  return fit::ok(std::move(dup_result).value());
}

// static
//
// The buffer_collection_token is the client end of the BufferCollectionToken
// which the client is exchanging for the BufferCollection (which the client is
// passing the server end of in buffer_collection_request).
//
// However, before we convert the client's token into a BufferCollection and
// start processing the messages the client may have already sent toward the
// BufferCollection, we want to process all the messages the client may have
// already sent toward the BufferCollectionToken.  This comes up because the
// BufferCollectionToken and Allocator are separate channels.
//
// We know that fidl_server will process all messages before it processes the
// close - it intentionally delays noticing the close until no messages are
// available to read.
//
// So this method will close the buffer_collection_token and when it closes via
// normal FIDL processing path, the token will remember the
// buffer_collection_request to essentially convert itself into.
void LogicalBufferCollection::BindSharedCollection(Sysmem* parent_device,
                                                   zx::channel buffer_collection_token,
                                                   CollectionServerEnd buffer_collection_request,
                                                   const ClientDebugInfo* client_debug_info) {
  auto result = CommonConvertToken(parent_device, std::move(buffer_collection_token),
                                   client_debug_info, "BindSharedCollection");
  if (result.is_error()) {
    return;
  }

  BufferCollectionToken* token = result.value();

  // This will token->FailAsync() if the token has already got one, or if the
  // token already saw token->Close().
  token->SetBufferCollectionRequest(std::move(buffer_collection_request));

  if (client_debug_info) {
    // The info will be propagated into the logcial buffer collection when the token closes.
    //
    // Intentionally copying *client_debug_info not moving, since allocator may be used for more
    // BindSharedCollection(s).
    token->QueueAllocatorClientDebugInfo(*client_debug_info);
  }

  // At this point, the token will process the rest of its previously queued
  // messages (from client to server), and then will convert the token into
  // a BufferCollection (view).  That conversion happens async shortly in
  // BindSharedCollectionInternal() (unless the LogicalBufferCollection fails
  // before then, in which case everything just gets deleted).
  //
  // ~buffer_collection_token here closes the client end of the token, but we
  // still process the rest of the queued messages before we process the
  // close.
  //
  // ~buffer_collection_token
}

zx_status_t LogicalBufferCollection::ValidateBufferCollectionToken(Sysmem* parent_device,
                                                                   zx_koid_t token_server_koid) {
  BufferCollectionToken* token = parent_device->FindTokenByServerChannelKoid(token_server_koid);
  return token ? ZX_OK : ZX_ERR_NOT_FOUND;
}

void LogicalBufferCollection::HandleTokenFailure(BufferCollectionToken& token, zx_status_t status) {
  // Clean close from FIDL channel point of view is ZX_ERR_PEER_CLOSED, and ZX_OK is never passed to
  // the error handler.
  ZX_DEBUG_ASSERT(status != ZX_OK);

  // We know |this| is alive because the token is alive and the token has a
  // fbl::RefPtr<LogicalBufferCollection>.  The token is alive because the token is still under the
  // tree rooted at root_.
  //
  // Any other deletion of the token_ptr out of the tree at root_ (outside of this error handler)
  // doesn't run this error handler.
  ZX_DEBUG_ASSERT(root_);

  std::optional<CollectionServerEnd> buffer_collection_request =
      token.TakeBufferCollectionRequest();

  if (!(status == ZX_ERR_PEER_CLOSED &&
        (token.is_done() || buffer_collection_request.has_value()))) {
    // LogAndFailDownFrom() will also remove any no-longer-needed nodes from the tree.
    //
    // A token whose error handler sees anything other than clean close with is_done() implies
    // LogicalBufferCollection failure.  The ability to detect unexpected closure of a token is a
    // main reason we use a channel for BufferCollectionToken instead of an eventpair.
    //
    // If a participant for some reason finds itself with an extra token it doesn't need, the
    // participant should use Close() to avoid triggering this failure.
    NodeProperties* tree_to_fail = FindTreeToFail(&token.node_properties());
    token.node_properties().LogError(FROM_HERE, "token failure - status: %d", status);
    if (tree_to_fail == root_.get()) {
      LogAndFailDownFrom(
          FROM_HERE, tree_to_fail, Error::kUnspecified,
          "Token failure causing LogicalBufferCollection failure - status: %d client: %s:%" PRIu64,
          status, token.node_properties().client_debug_info().name.c_str(),
          token.node_properties().client_debug_info().id);
    } else {
      LogAndFailDownFrom(
          FROM_HERE, tree_to_fail, Error::kUnspecified,
          "Token failure causing AttachToken() sub-tree failure - status: %d client: %s:%" PRIu64,
          status, token.node_properties().client_debug_info().name.c_str(),
          token.node_properties().client_debug_info().id);
    }
    return;
  }

  // At this point we know the token channel was closed, and that the client either did a Close()
  // or the token was dispensable, or allocator::BindSharedCollection() was used.
  ZX_DEBUG_ASSERT(status == ZX_ERR_PEER_CLOSED &&
                  (token.is_done() || buffer_collection_request.has_value()));
  // BufferCollectionToken enforces that these never both become true; the BufferCollectionToken
  // will fail instead.
  ZX_DEBUG_ASSERT(!(token.is_done() && buffer_collection_request.has_value()));

  if (!buffer_collection_request.has_value()) {
    ZX_DEBUG_ASSERT(token.is_done());
    // This was a token::Close().  We want to stop tracking the token now that we've processed all
    // its previously-queued inbound messages.  This might be the last token, so we MaybeAllocate().
    // This path isn't a failure (unless there are also zero BufferCollection views in which case
    // MaybeAllocate() calls Fail()).
    //
    // Keep self alive via "self" in case this will drop connected_node_count_ to zero.
    auto self = token.shared_logical_buffer_collection();
    ZX_DEBUG_ASSERT(self.get() == this);
    // This token never had any constraints, and it was Close()ed, but we need to keep the
    // NodeProperties because it may have child NodeProperties under it, and it may have had
    // SetDispensable() called on it.
    //
    // This OrphanedNode has no BufferCollectionConstraints.
    ZX_DEBUG_ASSERT(!token.node_properties().buffers_logically_allocated());
    NodeProperties& node_properties = token.node_properties();
    // This replaces token with an OrphanedNode, and also de-refs token.  Not possible to send an
    // epitaph because ZX_ERR_PEER_CLOSED.
    OrphanedNode::EmplaceInTree(fbl::RefPtr(this), &node_properties);
    MaybeAllocate();
    // ~self may delete "this"
  } else {
    // At this point we know that this was a BindSharedCollection().  We need to convert the
    // BufferCollectionToken into a BufferCollection.  The NodeProperties remains, with its Node
    // set to the new BufferCollection instead of the old BufferCollectionToken.
    //
    // ~token during this call
    BindSharedCollectionInternal(&token, std::move(buffer_collection_request.value()));
  }
}

bool LogicalBufferCollection::CommonCreateBufferCollectionTokenStage1(
    fbl::RefPtr<LogicalBufferCollection> self, NodeProperties* new_node_properties,
    const TokenServerEnd& token_request, BufferCollectionToken** out_token) {
  auto& token = BufferCollectionToken::EmplaceInTree(self, new_node_properties, token_request);
  token.SetErrorHandler([this, &token](zx_status_t status) { HandleTokenFailure(token, status); });

  if (token.create_status() != ZX_OK) {
    LogAndFailNode(FROM_HERE, &token.node_properties(), Error::kUnspecified,
                   "token.status() failed - create_status: %d", token.create_status());
    return false;
  }

  if (token.was_unfound_node()) {
    // No failure triggered by this, but a helpful debug message on how to avoid a previous failure.
    LogClientError(FROM_HERE, new_node_properties,
                   "BufferCollectionToken.Duplicate() received for creating token with server koid"
                   "%ld after BindSharedCollection() previously received attempting to use same"
                   "token.  Client sequence should be Duplicate(), Sync(), BindSharedCollection()."
                   "Missing Sync()?",
                   token.server_koid());
  }

  *out_token = &token;
  return true;
}

void LogicalBufferCollection::CreateBufferCollectionTokenV1(
    fbl::RefPtr<LogicalBufferCollection> self, NodeProperties* new_node_properties,
    TokenServerEndV1 token_server_end_v1) {
  ZX_DEBUG_ASSERT(token_server_end_v1);
  TokenServerEndCombinedV1AndV2 server_end_combined =
      ConvertV1TokenRequestToCombinedTokenRequest(std::move(token_server_end_v1));
  TokenServerEnd token_request(std::move(server_end_combined));
  BufferCollectionToken* token;
  if (!CommonCreateBufferCollectionTokenStage1(self, new_node_properties, token_request, &token)) {
    return;
  }
  token->Bind(std::move(token_request));
}

void LogicalBufferCollection::CreateBufferCollectionTokenV2(
    fbl::RefPtr<LogicalBufferCollection> self, NodeProperties* new_node_properties,
    TokenServerEndV2 token_server_end_v2) {
  ZX_DEBUG_ASSERT(token_server_end_v2);
  TokenServerEndCombinedV1AndV2 server_end_combined =
      ConvertV2TokenRequestToCombinedTokenRequest(std::move(token_server_end_v2));
  TokenServerEnd token_request(std::move(server_end_combined));
  BufferCollectionToken* token;
  if (!CommonCreateBufferCollectionTokenStage1(self, new_node_properties, std::move(token_request),
                                               &token)) {
    return;
  }
  token->Bind(std::move(token_request));
}

bool LogicalBufferCollection::CommonCreateBufferCollectionTokenGroupStage1(
    fbl::RefPtr<LogicalBufferCollection> self, NodeProperties* new_node_properties,
    const GroupServerEnd& group_request, BufferCollectionTokenGroup** out_group) {
  auto& group = BufferCollectionTokenGroup::EmplaceInTree(self, new_node_properties, group_request);
  group.SetErrorHandler([this, &group](zx_status_t status) {
    // Clean close from FIDL channel point of view is ZX_ERR_PEER_CLOSED,
    // and ZX_OK is never passed to the error handler.
    ZX_DEBUG_ASSERT(status != ZX_OK);

    // We know |this| is alive because the group is alive and the group has
    // a fbl::RefPtr<LogicalBufferCollection>.  The group is alive because
    // the group is still under the tree rooted at root_.
    //
    // Any other deletion of the group out of the tree at root_ (outside of
    // this error handler) doesn't run this error handler.
    ZX_DEBUG_ASSERT(root_);

    // If not clean Close()
    if (!(status == ZX_ERR_PEER_CLOSED && group.is_done())) {
      // LogAndFailDownFrom() will also remove any no-longer-needed nodes from the tree.
      //
      // A group whose error handler sees anything other than clean Close() (is_done()) implies
      // failure domain failure (possibly LogicalBufferCollection failure if not separated from
      // root_ by AttachToken() or SetDispensable()).
      //
      // If a participant needs/wants to close its BufferCollectionTokenGroup channel without
      // triggering failure domain failure, the participant should use Close() to avoid triggering
      // this failure.
      NodeProperties* tree_to_fail = FindTreeToFail(&group.node_properties());
      if (tree_to_fail == root_.get()) {
        LogAndFailDownFrom(
            FROM_HERE, tree_to_fail, Error::kUnspecified,
            "Group failure causing LogicalBufferCollection failure - status: %d client: %s:%" PRIu64,
            status, group.node_properties().client_debug_info().name.c_str(),
            group.node_properties().client_debug_info().id);
      } else {
        LogAndFailDownFrom(
            FROM_HERE, tree_to_fail, Error::kUnspecified,
            "Group failure causing failure domain sub-tree failure - status: %d client: %s:%" PRIu64,
            status, group.node_properties().client_debug_info().name.c_str(),
            group.node_properties().client_debug_info().id);
      }
      return;
    }

    // At this point we know the group channel was Close()ed.
    ZX_DEBUG_ASSERT(status == ZX_ERR_PEER_CLOSED && group.is_done());

    // Just like a Close()ed token or collection channel, we replace a Close()ed
    // BufferCollectionTokenGroup with an OrphanedNode.  Since the 1 of N semantic of a group during
    // constraints aggregation is handled by NodeProperties, we don't need the
    // BufferCollectionTokenGroup object for that aspect.  The BufferCollectionTokenGroup is only
    // for the protocol-serving aspect and driving state changes related to protocol-serving.  Now
    // that protocol-serving is done, the BufferCollectionTokenGroup can go away.
    //
    // Unlike a token, since a group has no constraints of its own, the Close() doesn't implicitly
    // set has_constraints false constraints (unconstrained constraints), so the only reason we're
    // calling MaybeAllocate() in this path is in case there are zero remaining open channels to
    // any node, in which case the MaybeAllocate() will call Fail().
    //
    // We want to stop tracking the group now that we've processed all its previously-queued inbound
    // messages.
    //
    // Keep self alive fia "self" in case this will drop connected_node_count_ to zero.
    auto self = group.shared_logical_buffer_collection();
    ZX_DEBUG_ASSERT(self.get() == this);
    // This OrphanedNode will have no BufferCollectionConstraints of its own (no group ever does),
    // and the NodeConstraints is already configured as a group.  A group may be Close()ed before
    // or after buffers are logically allocated (in contrast to a token which can only be Close()ed
    // before buffers are logically allocated.
    NodeProperties& node_properties = group.node_properties();
    // This replaces group with an OrphanedNode, and also de-refs group.  It's not possible to send
    // an epitaph because ZX_ERR_PEER_CLOSED.
    OrphanedNode::EmplaceInTree(fbl::RefPtr(this), &node_properties);
    // This will never actually allocate, since group Close() is never a state change that will
    // enable allocation, but MaybeAllocate() may call Fail() if there are zero client connections
    // to any node remaining.
    MaybeAllocate();
    // ~self may delete "this"
  });

  if (group.create_status() != ZX_OK) {
    LogAndFailNode(FROM_HERE, new_node_properties, Error::kUnspecified,
                   "get_handle_koids() failed - status: %d", group.create_status());
    return false;
  }

  LogInfo(FROM_HERE, "CreateBufferCollectionTokenGroup() - server_koid: %lu", group.server_koid());

  *out_group = &group;
  return true;
}

void LogicalBufferCollection::CreateBufferCollectionTokenGroupV1(
    fbl::RefPtr<LogicalBufferCollection> self, NodeProperties* new_node_properties,
    GroupServerEndV1 group_request_v1) {
  ZX_DEBUG_ASSERT(group_request_v1);
  GroupServerEnd group_request(std::move(group_request_v1));
  BufferCollectionTokenGroup* group;
  if (!CommonCreateBufferCollectionTokenGroupStage1(self, new_node_properties, group_request,
                                                    &group)) {
    return;
  }
  group->Bind(std::move(group_request));
}

void LogicalBufferCollection::CreateBufferCollectionTokenGroupV2(
    fbl::RefPtr<LogicalBufferCollection> self, NodeProperties* new_node_properties,
    GroupServerEndV2 group_request_v2) {
  ZX_DEBUG_ASSERT(group_request_v2);
  GroupServerEnd group_request(std::move(group_request_v2));
  BufferCollectionTokenGroup* group;
  if (!CommonCreateBufferCollectionTokenGroupStage1(self, new_node_properties, group_request,
                                                    &group)) {
    return;
  }
  group->Bind(std::move(group_request));
}

void LogicalBufferCollection::AttachLifetimeTracking(zx::eventpair server_end,
                                                     uint32_t buffers_remaining) {
  lifetime_tracking_.emplace(buffers_remaining, std::move(server_end));
  SweepLifetimeTracking();
}

void LogicalBufferCollection::SweepLifetimeTracking() {
  while (true) {
    if (lifetime_tracking_.empty()) {
      return;
    }
    auto last_iter = lifetime_tracking_.end();
    last_iter--;
    uint32_t buffers_remaining = last_iter->first;
    if (buffers_remaining < buffers_.size()) {
      return;
    }
    ZX_DEBUG_ASSERT(buffers_remaining >= buffers_.size());
    // This does ~server_end, which signals ZX_EVENTPAIR_PEER_CLOSED to the client_end which is
    // typically held by the client.
    lifetime_tracking_.erase(last_iter);
  }
}

void LogicalBufferCollection::OnDependencyReady() {
  // MaybeAllocate() requires the caller to keep "this" alive.
  auto self = fbl::RefPtr(this);
  MaybeAllocate();
  return;
}

void LogicalBufferCollection::SetName(uint32_t priority, std::string name) {
  if (!name_.has_value() || (priority > name_->priority)) {
    name_ = CollectionName{priority, name};
    name_property_ = inspect_node_.CreateString("name", name);
  }
}

void LogicalBufferCollection::SetDebugTimeoutLogDeadline(int64_t deadline) {
  if (has_allocation_result_) {
    // too late to complain about allocation taking too long; already done
    return;
  }
  if (creation_timer_run_count_ != 0) {
    // SetDebugTimeoutLogDeadline is too late to set the initial first timer deadline; the second
    // deadline is a fixed 30 additional seconds beyond the first time the timer runs.
    return;
  }
  creation_timer_.Cancel();
  zx_status_t status =
      creation_timer_.PostForTime(parent_sysmem_->loop_dispatcher(), zx::time(deadline));
  ZX_ASSERT(status == ZX_OK);
}

void LogicalBufferCollection::SetVerboseLogging() {
  is_verbose_logging_ = true;
  LogInfo(FROM_HERE, "SetVerboseLogging()");
}

uint64_t LogicalBufferCollection::CreateDispensableOrdinal() { return next_dispensable_ordinal_++; }

AllocationResult LogicalBufferCollection::allocation_result() {
  ZX_DEBUG_ASSERT(has_allocation_result_ ||
                  (!maybe_allocation_error_.has_value() && !allocation_result_info_.has_value()));
  // If this assert fails, it mean we've already done ::Fail().  This should be impossible since
  // Fail() clears all BufferCollection views so they shouldn't be able to call
  // ::allocation_result().
  ZX_DEBUG_ASSERT(!(has_allocation_result_ && !maybe_allocation_error_.has_value() &&
                    !allocation_result_info_.has_value()));
  return {
      .buffer_collection_info =
          allocation_result_info_.has_value() ? &allocation_result_info_.value() : nullptr,
      .maybe_error = maybe_allocation_error_,
  };
}

LogicalBufferCollection::LogicalBufferCollection(Sysmem* parent_device)
    : parent_sysmem_(parent_device), create_time_monotonic_(zx::clock::get_monotonic()) {
  TRACE_DURATION("gfx", "LogicalBufferCollection::LogicalBufferCollection", "this", this);
  LogInfo(FROM_HERE, "LogicalBufferCollection::LogicalBufferCollection()");

  // For now, we create+get a koid that's unique to this LogicalBufferCollection, to be absolutely
  // certain that the buffer_collection_id_ will be unique per boot. We're using an event here only
  // because it's a fairly cheap object to create/delete.
  zx::event dummy_event;
  zx_status_t status = zx::event::create(0, &dummy_event);
  ZX_ASSERT(status == ZX_OK);
  zx_koid_t koid = get_koid(dummy_event);
  ZX_ASSERT(koid != ZX_KOID_INVALID);
  dummy_event.reset();
  buffer_collection_id_ = koid;
  ZX_DEBUG_ASSERT(buffer_collection_id_ != ZX_KOID_INVALID);
  ZX_DEBUG_ASSERT(buffer_collection_id_ != ZX_KOID_KERNEL);

  parent_sysmem_->AddLogicalBufferCollection(this);
  inspect_node_ =
      parent_sysmem_->collections_node().CreateChild(CreateUniqueName("logical-collection-"));

  status = creation_timer_.PostDelayed(parent_sysmem_->loop_dispatcher(), zx::sec(5));
  ZX_ASSERT(status == ZX_OK);
  // nothing else to do here
}

void LogicalBufferCollection::Fail() {
  // This is allowed to (but not required to) delete "this" synchronously.
  LogAndFailRootNode(FROM_HERE, fuchsia_sysmem2::Error::kUnspecified,
                     "force-failing LogicalBufferCollection");
}

LogicalBufferCollection::~LogicalBufferCollection() {
  TRACE_DURATION("gfx", "LogicalBufferCollection::~LogicalBufferCollection", "this", this);
  LogInfo(FROM_HERE, "~LogicalBufferCollection");

  // This isn't strictly necessary, but to avoid any confusion or brittle-ness, cancel explicitly
  // before member destructors start running.
  creation_timer_.Cancel();

  // Cancel all TrackedParentVmo waits to avoid a use-after-free of |this|.
  ClearBuffers();

  if (memory_allocator_) {
    memory_allocator_->RemoveDestroyCallback(reinterpret_cast<intptr_t>(this));
  }

  parent_sysmem_->RemoveLogicalBufferCollection(this);
}

void LogicalBufferCollection::LogAndFailRootNode(Location location, Error error, const char* format,
                                                 ...) {
  ZX_DEBUG_ASSERT(format);
  va_list args;
  va_start(args, format);
  vLogBuffered(fuchsia_logging::LogSeverity::Warn, location.file(), location.line(),
               "LogicalBufferCollection", format, args);
  va_end(args);
  FailRootNode(error);
}

void LogicalBufferCollection::FailRootNode(Error error) {
  if (!root_) {
    // This can happen for example when we're failing due to zero strong VMOs remaining, but all
    // Node(s) happen to already be gone so the root node was already removed.
    return;
  }
  FailDownFrom(root_.get(), error);
}

void LogicalBufferCollection::LogAndFailDownFrom(Location location, NodeProperties* tree_to_fail,
                                                 Error error, const char* format, ...) {
  ZX_DEBUG_ASSERT(format);
  va_list args;
  va_start(args, format);
  bool is_root = (tree_to_fail == root_.get());
  vLogBuffered(
      fuchsia_logging::LogSeverity::Info, location.file(), location.line(),
      is_root ? "LogicalBufferCollection root fail" : "LogicalBufferCollection sub-tree fail",
      format, args);
  va_end(args);
  FailDownFrom(tree_to_fail, error);
}

void LogicalBufferCollection::FailDownFrom(NodeProperties* tree_to_fail, Error error) {
  // Keep self alive until this method is done.
  auto self = fbl::RefPtr(this);
  bool is_root = (tree_to_fail == root_.get());
  std::vector<NodeProperties*> breadth_first_order = tree_to_fail->BreadthFirstOrder();
  while (!breadth_first_order.empty()) {
    NodeProperties* child_most = breadth_first_order.back();
    breadth_first_order.pop_back();
    ZX_DEBUG_ASSERT(child_most->child_count() == 0);
    child_most->node()->Fail(error);
    child_most->RemoveFromTreeAndDelete();
  }
  if (is_root) {
    // At this point there is no further way for any participant on this LogicalBufferCollection to
    // perform a BufferCollectionToken::Duplicate(), BufferCollection::AttachToken(), or
    // BindSharedCollection(), because all the BufferCollectionToken(s) and BufferCollection(s) are
    // gone.  Any further pending requests were dropped when the channels closed just above.
    //
    // Since all the token views and collection views are gone, there is no way for any client to be
    // sent the strong VMOs again, so we can close the handles to the strong VMOs here.  This is
    // necessary in order to get ZX_VMO_ZERO_CHILDREN to happen in TrackedParentVmo(s) of
    // parent_vmos_, but not sufficient alone (clients must also close their VMO(s) including both
    // strong and weak VMOs).
    //
    // Clear out the result info since we won't be sending it again. This also clears out any
    // children of strong_parent_vmo_ that LogicalBufferCollection may still be holding.
    allocation_result_info_.reset();
  }
  // ~self, which will delete "this" if there are no more references to "this".
}

void LogicalBufferCollection::LogAndFailNode(Location location, NodeProperties* member_node,
                                             Error error, const char* format, ...) {
  ZX_DEBUG_ASSERT(format);
  auto tree_to_fail = FindTreeToFail(member_node);
  va_list args;
  va_start(args, format);
  bool is_root = (tree_to_fail == root_.get());
  vLogBuffered(
      fuchsia_logging::LogSeverity::Info, location.file(), location.line(),
      is_root ? "LogicalBufferCollection root fail" : "LogicalBufferCollection sub-tree fail",
      format, args);
  va_end(args);
  FailDownFrom(tree_to_fail, error);
}

void LogicalBufferCollection::FailNode(NodeProperties* member_node, Error error) {
  auto tree_to_fail = FindTreeToFail(member_node);
  FailDownFrom(tree_to_fail, error);
}

void LogicalBufferCollection::LogBuffered(fuchsia_logging::LogSeverity log_severity,
                                          Location location, const char* format, ...) const {
  va_list args;
  va_start(args, format);

  vLogBuffered(log_severity, location.file(), location.line(), nullptr, format, args);

  va_end(args);
}

void LogicalBufferCollection::vLogBuffered(fuchsia_logging::LogSeverity severity, const char* file,
                                           int line, const char* prefix, const char* format,
                                           va_list args) const {
  vLogToCallback(severity, file, line, prefix, format, args, maybe_buffer_log_);
}

void LogicalBufferCollection::LogInfo(Location location, const char* format, ...) const {
  va_list args;
  va_start(args, format);

  fuchsia_logging::LogSeverity severity;
  if (is_verbose_logging()) {
    severity = fuchsia_logging::LogSeverity::Info;
  } else {
    severity = fuchsia_logging::LogSeverity::Debug;
  }

  vLogBuffered(severity, location.file(), location.line(), nullptr, format, args);

  va_end(args);
}

void LogicalBufferCollection::LogErrorStatic(Location location,
                                             const ClientDebugInfo* client_debug_info,
                                             const char* format, ...) {
  va_list args;
  va_start(args, format);
  fbl::String formatted = fbl::StringVPrintf(format, args);
  if (client_debug_info && !client_debug_info->name.empty()) {
    fbl::String client_name = fbl::StringPrintf(
        " - client \"%s\" id %ld", client_debug_info->name.c_str(), client_debug_info->id);

    formatted = fbl::String::Concat({formatted, client_name});
  }
  ::sysmem_service::Log(fuchsia_logging::LogSeverity::Error, location.file(), location.line(),
                        nullptr, "%s", formatted.c_str());
  va_end(args);
}

fuchsia_logging::LogSeverity LogicalBufferCollection::LogSeverityToFuchsiaLogSeverity(
    LogicalBufferCollection::LogSeverity log_severity) const {
  // This intentionally lacks a default case so we'll get a compiler warning if another value is
  // added to LogicalBufferCollection::LogSeverity.
  switch (log_severity) {
    case LogSeverity::Trace:
      return fuchsia_logging::LogSeverity::Trace;
    case LogSeverity::Debug:
      return fuchsia_logging::LogSeverity::Debug;
    case LogSeverity::InfoOrDebug:
      if (is_verbose_logging()) {
        return fuchsia_logging::LogSeverity::Info;
      } else {
        return fuchsia_logging::LogSeverity::Debug;
      }
    case LogSeverity::Info:
      return fuchsia_logging::LogSeverity::Info;
    case LogSeverity::Warn:
      return fuchsia_logging::LogSeverity::Warn;
    case LogSeverity::Error:
      return fuchsia_logging::LogSeverity::Error;
    case LogSeverity::Fatal:
      return fuchsia_logging::LogSeverity::Fatal;
  }
}

void LogicalBufferCollection::VLogClient(LogicalBufferCollection::LogSeverity log_severity,
                                         Location location, const NodeProperties* node_properties,
                                         const char* format, va_list args) const {
  const char* collection_name = name_.has_value() ? name_->name.c_str() : "Unknown";
  fbl::String formatted = fbl::StringVPrintf(format, args);
  if (node_properties && !node_properties->client_debug_info().name.empty()) {
    fbl::String client_name = fbl::StringPrintf(
        " - collection \"%s\" - client \"%s\" id %ld", collection_name,
        node_properties->client_debug_info().name.c_str(), node_properties->client_debug_info().id);

    formatted = fbl::String::Concat({formatted, client_name});
  } else {
    fbl::String client_name = fbl::StringPrintf(" - collection \"%s\"", collection_name);

    formatted = fbl::String::Concat({formatted, client_name});
  }

  auto fuchsia_log_severity = LogSeverityToFuchsiaLogSeverity(log_severity);

  LogBuffered(fuchsia_log_severity, location, "%s", formatted.c_str());
}

void LogicalBufferCollection::LogClientInfo(Location location,
                                            const NodeProperties* node_properties,
                                            const char* format, ...) const {
  va_list args;
  va_start(args, format);
  VLogClientInfo(location, node_properties, format, args);
  va_end(args);
}

void LogicalBufferCollection::LogClientWarn(Location location,
                                            const NodeProperties* node_properties,
                                            const char* format, ...) const {
  va_list args;
  va_start(args, format);
  VLogClientWarn(location, node_properties, format, args);
  va_end(args);
}

void LogicalBufferCollection::LogClientError(Location location,
                                             const NodeProperties* node_properties,
                                             const char* format, ...) const {
  va_list args;
  va_start(args, format);
  VLogClientError(location, node_properties, format, args);
  va_end(args);
}

void LogicalBufferCollection::VLogClientInfo(Location location,
                                             const NodeProperties* node_properties,
                                             const char* format, va_list args) const {
  VLogClient(LogSeverity::InfoOrDebug, location, node_properties, format, args);
}

void LogicalBufferCollection::VLogClientWarn(Location location,
                                             const NodeProperties* node_properties,
                                             const char* format, va_list args) const {
  VLogClient(LogSeverity::Warn, location, node_properties, format, args);
}

void LogicalBufferCollection::VLogClientError(Location location,
                                              const NodeProperties* node_properties,
                                              const char* format, va_list args) const {
  VLogClient(LogSeverity::Error, location, node_properties, format, args);
}

void LogicalBufferCollection::LogWarn(Location location, const char* format, ...) const {
  va_list args;
  va_start(args, format);
  VLogWarn(location, format, args);
  va_end(args);
}

void LogicalBufferCollection::LogError(Location location, const char* format, ...) const {
  va_list args;
  va_start(args, format);
  VLogError(location, format, args);
  va_end(args);
}

void LogicalBufferCollection::VLogWarn(Location location, const char* format, va_list args) const {
  VLogClientWarn(location, current_node_properties_, format, args);
}

void LogicalBufferCollection::VLogError(Location location, const char* format, va_list args) const {
  VLogClientError(location, current_node_properties_, format, args);
}

void LogicalBufferCollection::LogBufferEnable() {
  ZX_DEBUG_ASSERT(!log_buffer_.has_value());
  log_buffer_.emplace();
}

void LogicalBufferCollection::LogBufferFlushAndDisable() {
  ZX_DEBUG_ASSERT(log_buffer_.has_value());
  for (auto& log_entry : *log_buffer_) {
    GetDefaultLogCallback()(log_entry.severity(), log_entry.file(), log_entry.line(),
                            log_entry.formatted_str().c_str());
  }
  log_buffer_.reset();
}

void LogicalBufferCollection::LogBufferDiscardAndDisable() {
  ZX_DEBUG_ASSERT(log_buffer_.has_value());
  if (is_verbose_logging()) {
    for (auto& log_entry : *log_buffer_) {
      GetDefaultLogCallback()(fuchsia_logging::LogSeverity::Info, log_entry.file(),
                              log_entry.line(), log_entry.formatted_str().c_str());
    }
  }
  log_buffer_.reset();
}

void LogicalBufferCollection::InitializeConstraintSnapshots(
    const ConstraintsList& constraints_list) {
  ZX_DEBUG_ASSERT(!constraints_list.empty());
  constraints_at_allocation_.clear();
  for (auto& constraints : constraints_list) {
    ConstraintInfoSnapshot snapshot;
    snapshot.inspect_node =
        inspect_node().CreateChild(CreateUniqueName("collection-at-allocation-"));
    if (constraints.constraints().min_buffer_count_for_camping().has_value()) {
      snapshot.inspect_node.RecordUint(
          "min_buffer_count_for_camping",
          constraints.constraints().min_buffer_count_for_camping().value());
    }
    if (constraints.constraints().min_buffer_count_for_shared_slack().has_value()) {
      snapshot.inspect_node.RecordUint(
          "min_buffer_count_for_shared_slack",
          constraints.constraints().min_buffer_count_for_shared_slack().value());
    }
    if (constraints.constraints().min_buffer_count_for_dedicated_slack().has_value()) {
      snapshot.inspect_node.RecordUint(
          "min_buffer_count_for_dedicated_slack",
          constraints.constraints().min_buffer_count_for_dedicated_slack().value());
    }
    if (constraints.constraints().min_buffer_count().has_value()) {
      snapshot.inspect_node.RecordUint("min_buffer_count",
                                       constraints.constraints().min_buffer_count().value());
    }
    snapshot.inspect_node.RecordUint("debug_id", constraints.client_debug_info().id);
    snapshot.inspect_node.RecordString("debug_name", constraints.client_debug_info().name);
    constraints_at_allocation_.push_back(std::move(snapshot));
  }
}

void LogicalBufferCollection::ResetGroupChildSelection(
    std::vector<NodeProperties*>& groups_by_priority) {
  // We intentionally set this to false in both ResetGroupChildSelection() and
  // InitGroupChildSelection(), because both put the child selections into a non-"done" state.
  done_with_group_child_selection_ = false;
  for (auto& node_properties : groups_by_priority) {
    node_properties->ResetWhichChild();
  }
}

void LogicalBufferCollection::InitGroupChildSelection(
    std::vector<NodeProperties*>& groups_by_priority) {
  // We intentionally set this to false in both ResetGroupChildSelection() and
  // InitGroupChildSelection(), because both put the child selections into a non-"done" state.
  done_with_group_child_selection_ = false;
  for (auto& node_properties : groups_by_priority) {
    node_properties->SetWhichChild(0);
  }
}

void LogicalBufferCollection::NextGroupChildSelection(
    std::vector<NodeProperties*> groups_by_priority) {
  ZX_DEBUG_ASSERT(groups_by_priority.empty() || groups_by_priority[0]->visible());
  std::vector<NodeProperties*>::reverse_iterator iter;
  if (groups_by_priority.empty()) {
    done_with_group_child_selection_ = true;
    ZX_DEBUG_ASSERT(DoneWithGroupChildSelections(groups_by_priority));
    return;
  }
  for (iter = groups_by_priority.rbegin(); iter != groups_by_priority.rend(); ++iter) {
    NodeProperties& np = **iter;
    if (!np.visible()) {
      // In this case we know there's another group before np in groups_by_priority, so in addition
      // to not needing to update which_child() of np, we don't need to handle being out of child
      // selections to consider in this path since we can handle that when we get to the first
      // group, which is also always visible.
      continue;
    }
    // If we're using NextGroupChildSelection(), we know that all groups have which_child() set.
    ZX_DEBUG_ASSERT(np.which_child().has_value());
    ZX_DEBUG_ASSERT(*np.which_child() < np.child_count());
    auto which_child = *np.which_child();
    if (which_child == np.child_count() - 1) {
      // "carry"; we'll keep looking for a parent which can increment without running out of
      // "digits".
      np.SetWhichChild(0);
      continue;
    }
    ZX_DEBUG_ASSERT(which_child + 1 < np.child_count());
    np.SetWhichChild(which_child + 1);
    // Successfully moved to next group child selection, and not done with child selections.
    ZX_DEBUG_ASSERT(!DoneWithGroupChildSelections(groups_by_priority));
    return;
  }
  // We tried to carry off the top (roughly speaking), so DoneWithGroupChildSelections() should now
  // return true.
  done_with_group_child_selection_ = true;
  ZX_DEBUG_ASSERT(DoneWithGroupChildSelections(groups_by_priority));
}

bool LogicalBufferCollection::DoneWithGroupChildSelections(
    const std::vector<NodeProperties*> groups_by_priority) {
  return done_with_group_child_selection_;
}

void LogicalBufferCollection::MaybeAllocate() {
  bool did_something;
  do {
    did_something = false;

    // It is possible for this to be running just after DecStrongNodeTally() posted an async root
    // fail, with that still pending as the last Node becomes ready for allocation and ends up here.
    // In that case, the allocation can happen and then the buffers will shortly get deleted again,
    // but there's no point in noticing strong_node_count_ == 0 in code here since the
    // allocation-first vs failure-before-allocation race can happen anyway due to message delivery
    // order at the server.

    // If a previous iteration of the loop failed the root_ of the LogicalBufferCollection, we'll
    // return below when we noticed that root_.connected_client_count() == 0.
    //
    // MaybeAllocate() is called after a connection drops.  If that connection is the last
    // connection to a failure domain, we'll fail the failure domain via this check.  We don't blame
    // the specific node that closed here, as it's just the last one, and could just as easily not
    // have been the last one.  We blame the root of the failure domain since it's fairly likely to
    // have useful debug name and debug ID.
    //
    // When it comes to connected_client_count() 0, we care about failure domains as defined by
    // FailurePropagationMode != kPropagate.  In other words, we'll fail a sub-tree with any degree
    // of failure isolation if its connected_client_count() == 0.  Whether we also fail the parent
    // tree depends on ErrorPropagationMode::kPropagateBeforeAllocation (and is_allocate_attempted_)
    // vs. ErrorPropagationMode::kDoNotPropagate.
    //
    // There's no "safe" iteration order that isn't subject to NodeProperties getting deleted out
    // from under the iteration, so we re-enumerate failure domains each time we fail a node + any
    // relevant other nodes.  The cost of enumeration could be reduced, but it should be good enough
    // given the expected participant counts.
    while (true) {
      auto failure_domains = FailureDomainSubtrees();
      // To get more detailed log output, we fail smaller trees first.
      int32_t i;
      for (i = safe_cast<int32_t>(failure_domains.size()) - 1; i >= 0; --i) {
        auto node_properties = failure_domains[i];
        if (node_properties->connected_client_count() == 0) {
          bool is_root = (node_properties == root_.get());
          if (is_root) {
            if (is_allocate_attempted_) {
              // Only log as info, because this is a normal way to destroy the buffer collection.
              LogClientInfo(FROM_HERE, node_properties, "Zero clients remain (after allocation).");
            } else {
              LogClientError(FROM_HERE, node_properties,
                             "Zero clients remain (before allocation).");
            }
          } else {
            LogClientError(FROM_HERE, node_properties,
                           "Sub-tree has zero clients remaining - failure_propagation_mode(): %u "
                           "is_allocate_attempted_: %u",
                           static_cast<unsigned int>(node_properties->error_propagation_mode()),
                           is_allocate_attempted_);
          }
          // This may fail the parent failure domain, possibly including the root, depending on
          // error_propagation_mode() and possibly is_allocate_attempted_.  If that happens,
          // FailNode() will log INFO saying so (FindTreeToFail() will log INFO).
          FailNode(node_properties, Error::kUnspecified);
          if (is_root) {
            return;
          }
          ZX_DEBUG_ASSERT(i >= 0);
          break;
        }
      }
      if (i < 0) {
        // Processed all failure domains and found zero that needed to fail due to zero
        // connected_client_count().
        break;
      }
    }

    // We may have failed the root.  The caller is keeping "this" alive, so we can still check
    // root_.
    if (!root_) {
      LogInfo(FROM_HERE, "Root node was failed due to sub-tree having zero clients remaining. (1)");
      return;
    }

    auto eligible_subtrees = PrunedSubtreesEligibleForLogicalAllocation();
    if (eligible_subtrees.empty()) {
      // nothing to do
      return;
    }

    for (auto eligible_subtree : eligible_subtrees) {
      // It's possible to fail a sub-tree mid-way through processing sub-trees; in that case we're
      // fine to continue with the next sub-tree since failure of one sub-tree in the list doesn't
      // impact any other sub-tree in the list.  However, if the root_ is failed, there are no
      // longer any sub-trees.
      if (!root_) {
        LogError(FROM_HERE,
                 "Root node was failed due to sub-tree having zero clients remaining. (2)");
        return;
      }

      // By default, aggregation failure, unless we get a more immediate failure or success.
      std::optional<Error> maybe_subtree_error = Error::kConstraintsIntersectionEmpty;

      // Buffer log output from LogInfo, LogWarn, LogError into log_buffer_. We'll discard
      // log_buffer_ if allocation is successful in the end. If allocation is not successful, we'll
      // FlushAndClearProvisional which will output log_buffer_ to the log. This way, if a group
      // child selection doesn't work ("all pixel_format(s) eliminated" or similar), but a later
      // group child selection does work, we won't log anything for the overall successful
      // allocation.
      //
      // This is mainly for cases like trying to include display's constraints along with gpu, but
      // falling back to gpu without display. This case can come up often, so we need this mechanism
      // to avoid log spam from the initial failure to aggregate constraints when display is
      // included.
      //
      // When is_verbose_logging(), instead of discarding log_buffer_ on success, we instead
      // attenuate the log severity to INFO if the overall allocation succeeds. The use of
      // SetVerboseLogging is limited to local debugging, and when SetVerboseLogging is used, we do
      // want to see all the log output, though it can be INFO if allocation succeeds overall.
      LogBufferEnable();
      // Unless this is cancelled, we'll flush the log output, in order, with relevant severity.
      // In the success case, we call LogBufferDiscardAndDisable.
      auto cleanup_log_buffer = fit::defer([this, &maybe_subtree_error] {
        if (maybe_subtree_error.has_value()) {
          LogBufferFlushAndDisable();
          return;
        }
        LogBufferDiscardAndDisable();
      });

      // Group 0 is highest priority (most important), with decreasing priority after that.
      std::vector<NodeProperties*> groups_by_priority =
          PrioritizedGroupsOfPrunedSubtreeEligibleForLogicalAllocation(*eligible_subtree);
      // All nodes of logical allocation (each group set to "all").
      ResetGroupChildSelection(groups_by_priority);
      auto all_subtree_nodes = NodesOfPrunedSubtreeEligibleForLogicalAllocation(*eligible_subtree);
      if (is_verbose_logging()) {
        // Just the NodeProperties* and whether it has constraints, in tree layout, accounting for
        // which_child == all which shows all children.
        //
        // We also log this again with which_child != all below, per group child selection.
        LogInfo(FROM_HERE, "pruned subtree (including all group children):");
        LogPrunedSubTree(eligible_subtree);
      }
      ZX_DEBUG_ASSERT(all_subtree_nodes.front() == eligible_subtree);
      bool found_not_ready_node = false;
      for (auto node_properties : all_subtree_nodes) {
        if (!node_properties->node()->ReadyForAllocation()) {
          found_not_ready_node = true;
          break;
        }
      }
      if (found_not_ready_node) {
        // next sub-tree
        continue;
      }

      if (is_verbose_logging()) {
        // All constraints, including NodeProperties* to ID the node.  Logged only once per subtree.
        LogInfo(FROM_HERE, "pruned subtree - node constraints:");
        LogNodeConstraints(all_subtree_nodes);
        // Log after constraints also, mainly to make the tree view easier to reference in the log
        // since the constraints log info can be a bit long.
        LogInfo(FROM_HERE, "pruned subtree (including all group children):");
        LogPrunedSubTree(eligible_subtree);
      }

      ZX_DEBUG_ASSERT((!is_allocate_attempted_) == (eligible_subtree == root_.get()));
      ZX_DEBUG_ASSERT(is_allocate_attempted_ || eligible_subtrees.size() == 1);

      bool was_allocate_attempted = is_allocate_attempted_;
      uint32_t combination_ordinal = 0;
      bool done_with_subtree;
      for (done_with_subtree = false, InitGroupChildSelection(groups_by_priority);
           !done_with_subtree && !DoneWithGroupChildSelections(groups_by_priority);
           ++combination_ordinal,
          ignore_result(done_with_subtree ||
                        (NextGroupChildSelection(groups_by_priority), false))) {
        if (combination_ordinal == kMaxGroupChildCombinations) {
          LogInfo(FROM_HERE,
                  "hit kMaxGroupChildCombinations before successful constraint aggregation");
          maybe_subtree_error = Error::kTooManyGroupChildCombinations;
          done_with_subtree = true;
          break;
        }
        auto nodes = NodesOfPrunedSubtreeEligibleForLogicalAllocation(*eligible_subtree);
        ZX_DEBUG_ASSERT(nodes.front() == eligible_subtree);

        if (is_verbose_logging()) {
          // Just the NodeProperties* and its type, in tree view, accounting for which_child, to
          // show only the children that'll be included in the aggregation.
          LogInfo(FROM_HERE, "pruned subtree (including only selected group children):");
          LogPrunedSubTree(eligible_subtree);
        }

        if (is_allocate_attempted_) {
          // Allocate was already previously attempted.
          std::optional<Error> maybe_error = TryLateLogicalAllocation(nodes);
          if (maybe_error.has_value()) {
            switch (*maybe_error) {
              case Error::kConstraintsIntersectionEmpty:
                // next child selections (next iteration of the enclosing loop)
                ZX_DEBUG_ASSERT(maybe_subtree_error.has_value() &&
                                *maybe_subtree_error == Error::kConstraintsIntersectionEmpty);
                break;
              default:
                maybe_subtree_error = *maybe_error;
                done_with_subtree = true;
                break;
            }
            // next child selections or done_with_subtree
            continue;
          }
          maybe_subtree_error.reset();
          done_with_subtree = true;
          // Succeed the nodes of the subtree that aren't currently hidden by which_child()
          // selections, and fail the rest of the subtree as if ZX_ERR_NOT_SUPPORTED (like
          // aggregation failure).
          SetSucceededLateLogicalAllocationResult(std::move(nodes), std::move(all_subtree_nodes));
          // done_with_subtree true means loop will be done
          ZX_DEBUG_ASSERT(done_with_subtree);
          continue;
        }

        // Initial allocation can only have one eligible subtree.
        ZX_DEBUG_ASSERT(eligible_subtrees.size() == 1);
        ZX_DEBUG_ASSERT(eligible_subtrees[0] == root_.get());

        // All the views have seen SetConstraints(), and there are no tokens left.
        // Regardless of whether allocation succeeds or fails, we remember we've
        // started an attempt to allocate so we don't attempt again.
        auto result = TryAllocate(nodes);
        if (!result.is_ok()) {
          switch (result.error()) {
            case Error::kConstraintsIntersectionEmpty:
              ZX_DEBUG_ASSERT(maybe_subtree_error.has_value() &&
                              *maybe_subtree_error == Error::kConstraintsIntersectionEmpty);
              // next child selections
              break;
            case Error::kPending:
              // OnDependencyReady will call MaybeAllocate again later after all secure allocators
              // are ready
              maybe_subtree_error = Error::kPending;
              done_with_subtree = true;
              break;
            default:
              maybe_subtree_error = result.error();
              done_with_subtree = true;
              break;
          }
          // next child selections or done_with_subtree
          continue;
        }
        maybe_subtree_error.reset();
        done_with_subtree = true;
        is_allocate_attempted_ = true;

        // Succeed portion of pruned subtree indicated by current group child selections; fail
        // rest of pruned subtree nodes with ZX_ERR_NOT_SUPPORTED as if they failed aggregation.
        //
        // For nodes outside the portion of the pruned subtree indicated by current group child
        // selections, it's not relevant whether the node(s) were part of any attempted
        // aggregations so far which happened to fail aggregation (with diffuse blame), or
        // whether the node(s) just didn't need to be used/tried.  We just fail them with
        // ZX_ERR_NOT_SUPPORTED as a sanitized/converged error so the relevant collection
        // channels indicate failure and the corresponding Node(s) can be cleaned up.
        SetAllocationResult(std::move(nodes), result.take_value(), std::move(all_subtree_nodes));
        // The outermost loop will try again, in case there were ready AttachToken()(s) and/or
        // dispensable views queued up behind the initial allocation.  In the next iteration if
        // there's nothing to do we'll return.
        //
        // done_with_subtree true means loop will be done
        ZX_DEBUG_ASSERT(done_with_subtree);
      }
      // The subtree_status can still be ZX_ERR_NOT_SUPPORTED if we never got any more immediate
      // failure and never got success, or this can be some other more immediate failure (still
      // needs to be handled/propagated here), or this can be ZX_OK if we already handled success,
      // or can be Error::kPending if not all secure allocators are ready yet.
      did_something = did_something ||
                      (!maybe_subtree_error.has_value() || *maybe_subtree_error != Error::kPending);
      if (maybe_subtree_error.has_value()) {
        if (*maybe_subtree_error == Error::kPending) {
          // next sub-tree
          //
          // OnDependencyReady will call MaybeAllocate again later after all secure allocators are
          // ready
          continue;
        }
        if (was_allocate_attempted) {
          // fail entire logical allocation, including all pruned subtree nodes, regardless of
          // group child selections
          SetFailedLateLogicalAllocationResult(all_subtree_nodes[0], *maybe_subtree_error);
        } else {
          // fail the initial allocation from root_ down
          LogInfo(FROM_HERE, "fail the initial allocation from root_ down");
          SetFailedAllocationResult(*maybe_subtree_error);
        }
      }
    }
  } while (did_something);
}

fpromise::result<fuchsia_sysmem2::BufferCollectionInfo, Error> LogicalBufferCollection::TryAllocate(
    std::vector<NodeProperties*> nodes) {
  TRACE_DURATION("gfx", "LogicalBufferCollection::TryAllocate", "this", this);

  // If we're here it means we have connected clients.
  ZX_DEBUG_ASSERT(root_->connected_client_count() != 0);
  ZX_DEBUG_ASSERT(!root_->buffers_logically_allocated());
  ZX_DEBUG_ASSERT(!nodes.empty());

  // Since this is the initial allocation, it's impossible for anything to be eligible other than
  // the root, since parents logically allocate before children, or together with children,
  // depending on use of AttachToken() or not.
  //
  // The root will be nodes[0].
  ZX_DEBUG_ASSERT(nodes[0] == root_.get());

  // Build a list of current constraints.  We clone/copy the constraints instead of moving any
  // portion of constraints, since we potentially will need the original constraints again after an
  // AttachToken().
  ConstraintsList constraints_list;
  for (auto node_properties : nodes) {
    ZX_DEBUG_ASSERT(node_properties->node()->ReadyForAllocation());
    if (node_properties->buffer_collection_constraints()) {
      // first parameter is cloned/copied via generated code
      constraints_list.emplace_back(*node_properties->buffer_collection_constraints(),
                                    node_properties->must_match_settings(), *node_properties);
    }
  }

  if (!waiting_for_secure_allocators_ready_) {
    InitializeConstraintSnapshots(constraints_list);
  }

  bool is_secure_required = false;
  for (auto& constraints : constraints_list) {
    if (IsSecureRequired(constraints.constraints())) {
      is_secure_required = true;
      break;
    }
  }
  if (is_secure_required && !parent_sysmem_->is_secure_mem_ready()) {
    if (!parent_sysmem_->was_secure_mem_ready()) {
      // parent_sysmem_ will call OnDependencyReady when all secure heaps/allocators are ready
      LogInfo(FROM_HERE, "secure_required && !is_secure_mem_ready");
      waiting_for_secure_allocators_ready_ = true;
      return fpromise::error(Error::kPending);
    } else {
      // In normal operation this won't happen, but if the SecureMem driver has cleanly disconnected
      // from sysmem, we'll have was_secure_mem_ready true, which means we don't necessarily expect
      // the securemem to be ready again any time soon (if ever). So in this case, don't try again
      // later and just fail now instead.
      LogError(FROM_HERE, "secure_required && !is_secure_mem_ready && was_secure_mem_ready");
      return fpromise::error(Error::kUnspecified);
    }
  }

  // Must out-last elements in constraints_list (and does), but doesn't need to out-last
  // constraints_list itself. See assert below re. constraints_list.empty().
  std::vector<std::unique_ptr<NodeProperties>> tmp_nodes;
  // This handles any must_match_vmo by adding an entry to constraints_list based on
  // SingleBufferSettings of the must_match_vmo's collection, or failing if must_match_vmo isn't a
  // sysmem-provided VMO. All the current entries in the constraints_list remain unmodified, but
  // must_match_vmo is ignored after this call, aside from potential logging if this collection has
  // verbose logging enabled.
  if (!AugmentConstraintsList(constraints_list, tmp_nodes)) {
    LogError(FROM_HERE, "AugmentConstraintsList failed");
    // the only current reason for this is client providing a must_match_vmo that's not a
    // sysmem-provided VMO
    return fpromise::error(Error::kProtocolDeviation);
  }

  // Notes on memory pressure:
  //
  // See fuchsia.sysmem2/BufferCollection.AttachLifetimeTracking for a way for a client to wait for
  // old/failed collections' VMOs to finish deleting (see also the caveats in the doc comments on
  // that message). This can be used to avoid overlap between an old collection lifetime and a new
  // collection lifetime when both collections are known to the same client.
  //
  // Currently, we don't detect (here) and wait (async) for any failed collections to be done
  // deleting their buffers before attempting allocation of this new collection. If we were to do so
  // here by returning Error::kPending and waiting async, we'd need to use a short deadline because
  // client threads commonly call WaitForAllBuffersAllocated synchronously, which can prevent that
  // thread from closing an old VMO handle until after this new allocation succeeds. Such a wait
  // without a short deadline could create a deadlock. In contrast, with AttachLifetimeTracking, a
  // client can basically promise (per doc comment warnings) that it won't end up creating such a
  // deadlock, and/or can set its own deadline to mitigate the possibility.
  //
  // Currently we also don't pay attention to fuchsia.memorypressure/Level here. As/when needed, we
  // potentially could coordinate with zircon to pre-roll a change in fuchsia.memorypressure/Level
  // before actually attempting to allocate the memory (presumably with a short async wait started
  // roughly here if the pressure level is increasing). So far, we don't want to intentionally fail
  // the new allocation here when pressure is WARNING or CRITICAL, since the fuchsia memory pressure
  // strategy (at least so far) is to use fuchsia.memorypressure signalling to clean up old stuff
  // ASAP rather than fail new stuff (which this allocation is).
  //
  // Old failed sysmem collections that haven't finished cleaning up aren't the only reason that a
  // new allocation may fail due to insufficient memory, even when the new allocation is allocating
  // physically-contiguous buffers (or protected buffers), as physically-contiguous pages can be
  // loaned to zircon when not in use using VMO range decommit, and success of re-commit isn't
  // guaranteed.

  // This doesn't necessarily mean that any of the clients have
  // set non-empty constraints though.  We do require that at least one
  // participant (probably the initiator) retains an open channel to its
  // BufferCollection until allocation is done, else allocation won't be
  // attempted.
  ZX_DEBUG_ASSERT(root_->connected_client_count() != 0);
  auto combine_result = CombineConstraints(&constraints_list);
  if (!combine_result.is_ok()) {
    // It's impossible to combine the constraints due to incompatible
    // constraints, or all participants set null constraints.
    LogInfo(FROM_HERE, "CombineConstraints() failed");
    return fpromise::error(Error::kConstraintsIntersectionEmpty);
  }

  ZX_DEBUG_ASSERT(combine_result.is_ok());

  ZX_DEBUG_ASSERT(constraints_list.empty());
  // tmp_nodes is still alive here after constraints_list.empty() assert just above, so tmp_nodes
  // outlasted elements of constraints_list

  auto combined_constraints = combine_result.take_value();

  auto generate_result = GenerateUnpopulatedBufferCollectionInfo(combined_constraints);
  if (!generate_result.is_ok()) {
    ZX_DEBUG_ASSERT(generate_result.error() != ZX_OK);
    if (generate_result.error() != ZX_ERR_NOT_SUPPORTED) {
      LogError(FROM_HERE, "GenerateUnpopulatedBufferCollectionInfo() failed");
    }
    // This error code allows a BufferCollectionTokenGroup (if any) to try its next child.
    return fpromise::error(Error::kConstraintsIntersectionEmpty);
  }
  ZX_DEBUG_ASSERT(generate_result.is_ok());
  auto buffer_collection_info = generate_result.take_value();

  // Above here, a BufferCollectionTokenGroup will move on to its next child. Below here, the group
  // will not move on to its next child and the overall allocation will fail.

  // Save BufferCollectionInfo prior to populating with VMOs, for later comparison with analogous
  // BufferCollectionInfo generated after AttachToken().
  //
  // Save both non-linearized and linearized versions of pre-populated BufferCollectionInfo.  The
  // linearized copy is for checking whether an attachtoken sequence should succeed, and the
  // non-linearized copy is for easier logging of diffs if an AttachToken() sequence fails due to
  // mismatched BufferCollectionInfo.
  ZX_DEBUG_ASSERT(!buffer_collection_info_before_population_.has_value());
  auto clone_result = sysmem::V2CloneBufferCollectionInfo(buffer_collection_info, 0);
  if (!clone_result.is_ok()) {
    ZX_DEBUG_ASSERT(clone_result.error() != ZX_OK);
    ZX_DEBUG_ASSERT(clone_result.error() != ZX_ERR_NOT_SUPPORTED);
    LogError(FROM_HERE, "V2CloneBufferCollectionInfo() failed");
    return fpromise::error(Error::kUnspecified);
  }
  buffer_collection_info_before_population_.emplace(clone_result.take_value());

  fpromise::result<fuchsia_sysmem2::BufferCollectionInfo, Error> result =
      Allocate(combined_constraints, &buffer_collection_info);
  if (!result.is_ok()) {
    ZX_DEBUG_ASSERT(result.error() != Error::kInvalid);
    ZX_DEBUG_ASSERT(result.error() != Error::kConstraintsIntersectionEmpty);
    return result;
  }

  if (is_verbose_logging()) {
    const auto& bci = result.value();
    IndentTracker indent_tracker(0);
    auto indent = indent_tracker.Current();
    LogInfo(FROM_HERE, "%*sBufferCollectionInfo:", indent.num_spaces(), "");
    {  // mirror indent level
      LogBufferCollectionInfo(indent_tracker, bci);
    }
  }

  ZX_DEBUG_ASSERT(result.is_ok());
  return result;
}

// This requires that nodes have the sub-tree's root-most node at nodes[0].
std::optional<Error> LogicalBufferCollection::TryLateLogicalAllocation(
    std::vector<NodeProperties*> nodes) {
  TRACE_DURATION("gfx", "LogicalBufferCollection::TryLateLogicalAllocation", "this", this);

  // The initial allocation was attempted, or we wouldn't be here.
  ZX_DEBUG_ASSERT(is_allocate_attempted_);

  // The initial allocation succeeded, or we wouldn't be here.  If the initial allocation fails, it
  // responds to any already-pending late allocation attempts also, and clears out all allocation
  // attempts including late allocation attempts.
  ZX_DEBUG_ASSERT(!allocation_result().maybe_error.has_value() &&
                  allocation_result().buffer_collection_info != nullptr);

  // If we're here it means we still have connected clients.
  ZX_DEBUG_ASSERT(root_->connected_client_count() != 0);

  // Build a list of current constraints.  We clone/copy instead of moving since we potentially will
  // need the original constraints again after another AttachToken() later.
  ConstraintsList constraints_list;

  // The constraints_list will include all already-logically-allocated node constraints, as well as
  // all node constraints from the "nodes" list which is all the nodes attempting to logically
  // allocate in this attempt.  There's also a synthetic entry to make sure the total # of buffers
  // is at least as large as the number already allocated.

  // Constraints of already-logically-allocated nodes.  This can include some OrphanedNode(s) in
  // addition to still-connected BufferCollection nodes.  There are no BufferCollectionToken nodes
  // in this category.
  auto logically_allocated_nodes =
      root_->BreadthFirstOrder([](const NodeProperties& node_properties) {
        bool keep_and_iterate_children = node_properties.buffers_logically_allocated();
        return NodeFilterResult{.keep_node = keep_and_iterate_children,
                                .iterate_children = keep_and_iterate_children};
      });
  for (auto logically_allocated_node : logically_allocated_nodes) {
    if (logically_allocated_node->buffer_collection_constraints()) {
      // first parameter cloned/copied via generated code
      constraints_list.emplace_back(*logically_allocated_node->buffer_collection_constraints(),
                                    logically_allocated_node->must_match_settings(),
                                    *logically_allocated_node);
    }
  }

  // Constraints of nodes trying to logically allocate now.  These can include BufferCollection(s)
  // and OrphanedNode(s).
  for (auto additional_node : nodes) {
    ZX_DEBUG_ASSERT(additional_node->node()->ReadyForAllocation());
    if (additional_node->buffer_collection_constraints()) {
      // first parameter cloned/copied via generated code
      constraints_list.emplace_back(*additional_node->buffer_collection_constraints(),
                                    additional_node->must_match_settings(), *additional_node);
    }
  }

  // Synthetic constraints entry to make sure the total # of buffers is at least as large as the
  // number already allocated.  Also, to try to use the same PixelFormat as we've already allocated,
  // else we'll fail to CombineConstraints().  Also, if what we've already allocated has any
  // optional characteristics, we require those so that we'll choose to enable those characteristics
  // again if we can, else we'll fail to CombineConstraints().
  const auto& existing = *buffer_collection_info_before_population_;
  fuchsia_sysmem2::BufferCollectionConstraints existing_constraints = GetExistingConstraints(
      existing.settings().value(), safe_cast<uint32_t>(existing.buffers()->size()),
      safe_cast<uint32_t>(existing.buffers()->size()));

  if (is_verbose_logging()) {
    LogInfo(FROM_HERE, "constraints from initial allocation:");
    LogConstraints(FROM_HERE, nullptr, existing_constraints);
  }

  // We could make this temp NodeProperties entirely stack-based, but we'd rather enforce that
  // NodeProperties is always tracked with std::unique_ptr<NodeProperties>.
  auto tmp_node = NodeProperties::NewTemporary(this, std::move(existing_constraints),
                                               "sysmem-internals-no-fewer");
  // first parameter cloned/copied via generated code
  constraints_list.emplace_back(*tmp_node->buffer_collection_constraints(),
                                tmp_node->must_match_settings(), *tmp_node);

  // Must out-last elements in constraints_list (and does), but doesn't need to out-last
  // constraints_list itself. See assert below re. constraints_list.empty().
  std::vector<std::unique_ptr<NodeProperties>> tmp_nodes;
  // This handles any must_match_vmo by adding an entry to constraints_list based on
  // SingleBufferSettings of the must_match_vmo's collection, or failing if must_match_vmo isn't a
  // sysmem-provided VMO. All the current entries in the constraints_list remain unmodified, but
  // must_match_vmo is ignored after this call, aside from potential logging if this collection has
  // verbose logging enabled.
  if (!AugmentConstraintsList(constraints_list, tmp_nodes)) {
    LogError(FROM_HERE, "AugmentConstraintsList failed");
    // the only current reason for this is client providing a must_match_vmo that's not a
    // sysmem-provided VMO
    return Error::kProtocolDeviation;
  }

  // This doesn't necessarily mean that any of the clients have
  // set non-empty constraints though.  We do require that at least one
  // participant (probably the initiator) retains an open channel to its
  // BufferCollection until allocation is done, else allocation won't be
  // attempted.
  ZX_DEBUG_ASSERT(root_->connected_client_count() != 0);
  auto combine_result = CombineConstraints(&constraints_list);
  if (!combine_result.is_ok()) {
    // It's impossible to combine the constraints due to incompatible constraints, or all
    // participants set null constraints.
    //
    // While nodes are from the pruned tree, if a parent can't allocate, then its child can't
    // allocate either, so this fails the whole sub-tree.
    return Error::kConstraintsIntersectionEmpty;
  }

  ZX_DEBUG_ASSERT(combine_result.is_ok());

  ZX_DEBUG_ASSERT(constraints_list.empty());
  // tmp_nodes is still alive here after constraints_list.empty() assert just above, so tmp_nodes
  // outlasted elements of constraints_list

  auto combined_constraints = combine_result.take_value();

  auto generate_result = GenerateUnpopulatedBufferCollectionInfo(combined_constraints);
  if (!generate_result.is_ok()) {
    ZX_DEBUG_ASSERT(generate_result.error() != ZX_OK);
    if (generate_result.error() != ZX_ERR_NOT_SUPPORTED) {
      LogError(FROM_HERE,
               "GenerateUnpopulatedBufferCollectionInfo() failed -> "
               "AttachToken() sequence failed - status: %d",
               generate_result.error());
    }
    // This error code allows a BufferCollectionTokenGroup (if any) to try its next child.
    return Error::kConstraintsIntersectionEmpty;
  }
  ZX_DEBUG_ASSERT(generate_result.is_ok());
  fuchsia_sysmem2::BufferCollectionInfo unpopulated_buffer_collection_info =
      generate_result.take_value();

  zx::result comparison = CompareBufferCollectionInfo(*buffer_collection_info_before_population_,
                                                      unpopulated_buffer_collection_info);
  if (comparison.is_error()) {
    LogInfo(FROM_HERE, "Failed to compare buffer collection info, %s", comparison.status_string());
    return Error::kUnspecified;
  }
  if (!comparison.value()) {
    LogInfo(FROM_HERE,
            "buffer_collection_info_before_population_ is not the same as "
            "unpopulated_buffer_collection_info");
    LogDiffsBufferCollectionInfo(*buffer_collection_info_before_population_,
                                 unpopulated_buffer_collection_info);
    return Error::kConstraintsIntersectionEmpty;
  }

  // Now that we know the new participants can be added without changing the BufferCollectionInfo,
  // we can inform the new participants that their logical allocation succeeded.
  //
  // This will set success for nodes of the pruned sub-tree, not any AttachToken() children; those
  // attempt logical allocation later assuming all goes well.  The success only applies to the
  // current which_child() selections; nodes of the pruned sub-tree outside the current
  // which_child() selections will still be handled as if they are ZX_ERR_NOT_SUPPORTED aggregation
  // failure (despite the possibility that perhaps a lower-priority list of selections could have
  // succeeded if the current list of selections hadn't).
  //
  // no error == success
  return std::nullopt;
}

fuchsia_sysmem2::BufferCollectionConstraints LogicalBufferCollection::GetExistingConstraints(
    const fuchsia_sysmem2::SingleBufferSettings& settings, std::optional<uint32_t> min_buffer_count,
    std::optional<uint32_t> max_buffer_count) {
  fuchsia_sysmem2::BufferCollectionConstraints existing_constraints;
  fuchsia_sysmem2::BufferUsage usage;
  usage.none().emplace(fuchsia_sysmem2::kNoneUsage);
  existing_constraints.usage().emplace(std::move(usage));
  ZX_DEBUG_ASSERT(!existing_constraints.min_buffer_count_for_camping().has_value());
  ZX_DEBUG_ASSERT(!existing_constraints.min_buffer_count_for_dedicated_slack().has_value());
  ZX_DEBUG_ASSERT(!existing_constraints.min_buffer_count_for_shared_slack().has_value());
  if (min_buffer_count.has_value()) {
    existing_constraints.min_buffer_count() = *min_buffer_count;
  }
  if (max_buffer_count.has_value()) {
    existing_constraints.max_buffer_count() = *max_buffer_count;
  }
  existing_constraints.buffer_memory_constraints().emplace();
  auto& buffer_memory_constraints = existing_constraints.buffer_memory_constraints().value();
  buffer_memory_constraints.min_size_bytes().emplace(
      settings.buffer_settings()->size_bytes().value());
  buffer_memory_constraints.max_size_bytes().emplace(
      settings.buffer_settings()->size_bytes().value());
  if (settings.buffer_settings()->is_physically_contiguous().value()) {
    buffer_memory_constraints.physically_contiguous_required().emplace(true);
  }
  ZX_DEBUG_ASSERT(settings.buffer_settings()->is_secure().value() ==
                  IsSecureHeap(settings.buffer_settings()->heap().value()));
  if (settings.buffer_settings()->is_secure().value()) {
    buffer_memory_constraints.secure_required().emplace(true);
  }
  switch (settings.buffer_settings()->coherency_domain().value()) {
    case fuchsia_sysmem2::CoherencyDomain::kCpu:
      // We don't want defaults chosen based on usage, so explicitly specify each of these fields.
      buffer_memory_constraints.cpu_domain_supported().emplace(true);
      buffer_memory_constraints.ram_domain_supported().emplace(false);
      buffer_memory_constraints.inaccessible_domain_supported().emplace(false);
      break;
    case fuchsia_sysmem2::CoherencyDomain::kRam:
      // We don't want defaults chosen based on usage, so explicitly specify each of these fields.
      buffer_memory_constraints.cpu_domain_supported().emplace(false);
      buffer_memory_constraints.ram_domain_supported().emplace(true);
      buffer_memory_constraints.inaccessible_domain_supported().emplace(false);
      break;
    case fuchsia_sysmem2::CoherencyDomain::kInaccessible:
      // We don't want defaults chosen based on usage, so explicitly specify each of these fields.
      buffer_memory_constraints.cpu_domain_supported().emplace(false);
      buffer_memory_constraints.ram_domain_supported().emplace(false);
      buffer_memory_constraints.inaccessible_domain_supported().emplace(true);
      break;
    default:
      ZX_PANIC("not yet implemented (new enum value?)");
  }
  buffer_memory_constraints.permitted_heaps().emplace(1);
  // intentional copy/clone
  buffer_memory_constraints.permitted_heaps()->at(0) = settings.buffer_settings()->heap().value();
  if (settings.image_format_constraints().has_value()) {
    // We can't loosen the constraints after initial allocation, nor can we tighten them.  We also
    // want to chose the same PixelFormat as we already have allocated.
    existing_constraints.image_format_constraints().emplace(1);
    // clone/copy via generated code
    existing_constraints.image_format_constraints()->at(0) =
        settings.image_format_constraints().value();
  }

  return existing_constraints;
}

bool LogicalBufferCollection::AugmentConstraintsList(
    ConstraintsList& constraints_list, std::vector<std::unique_ptr<NodeProperties>>& tmp_nodes) {
  size_t starting_size = constraints_list.size();
  auto constraints_list_iter = constraints_list.begin();
  // std::list iterators remain valid when adding an item to end of list
  static_assert(std::is_same_v<ConstraintsList, std::list<Constraints>>);
  for (size_t i = 0; i < starting_size; ++i, ++constraints_list_iter) {
    auto& maybe_must_match_settings = constraints_list_iter->must_match_settings();
    if (!maybe_must_match_settings.has_value()) {
      continue;
    }
    auto& must_match_settings = *maybe_must_match_settings;
    auto must_match_constraints =
        GetExistingConstraints(must_match_settings, std::nullopt, std::nullopt);

    // This tmp_node doesn't need BufferCollectionConstraints; we add must_match_constraints to
    // constraints_list directly below. We may remove the BufferCollectionConstraints parameter from
    // NewTemporary before long, or make the NodeProperties of Constraints optional.
    auto tmp_node = NodeProperties::NewTemporary(
        this, fuchsia_sysmem2::BufferCollectionConstraints{}, "AugmentConstraintsList");

    // first parameter cloned/copied by generated code; second parameter kept alive by tmp_nodes
    //
    // We won't be iterating over this added element in the current loop because we stop at
    // starting_size minus 1.
    constraints_list.emplace_back(must_match_constraints, tmp_node->must_match_settings(),
                                  *tmp_node);
    tmp_nodes.emplace_back(std::move(tmp_node));
  }
  return true;
}

zx::result<bool> LogicalBufferCollection::CompareBufferCollectionInfo(
    fuchsia_sysmem2::BufferCollectionInfo& lhs, fuchsia_sysmem2::BufferCollectionInfo& rhs) {
  // Clone both.
  auto clone = [this](fuchsia_sysmem2::BufferCollectionInfo& v)
      -> zx::result<fuchsia_sysmem2::BufferCollectionInfo> {
    auto clone_result = sysmem::V2CloneBufferCollectionInfo(v, 0);
    if (!clone_result.is_ok()) {
      ZX_DEBUG_ASSERT(clone_result.error() != ZX_OK);
      ZX_DEBUG_ASSERT(clone_result.error() != ZX_ERR_NOT_SUPPORTED);
      LogError(FROM_HERE, "V2CloneBufferCollectionInfo() failed: %d", clone_result.error());
      return zx::error(clone_result.error());
    }
    return zx::ok(clone_result.take_value());
  };
  auto clone_lhs = clone(lhs);
  if (clone_lhs.is_error()) {
    return clone_lhs.take_error();
  }
  auto clone_rhs = clone(rhs);
  if (clone_rhs.is_error()) {
    return clone_rhs.take_error();
  }

  // Encode both.
  auto encoded_lhs = fidl::StandaloneEncode(std::move(clone_lhs.value()));
  if (!encoded_lhs.message().ok()) {
    LogError(FROM_HERE, "lhs encode error: %s", encoded_lhs.message().FormatDescription().c_str());
    return zx::error(encoded_lhs.message().error().status());
  }
  ZX_DEBUG_ASSERT(encoded_lhs.message().handle_actual() == 0);
  auto encoded_rhs = fidl::StandaloneEncode(std::move(clone_rhs.value()));
  if (!encoded_rhs.message().ok()) {
    LogError(FROM_HERE, "rhs encode error: %s", encoded_rhs.message().FormatDescription().c_str());
    return zx::error(encoded_rhs.message().error().status());
  }
  ZX_DEBUG_ASSERT(encoded_rhs.message().handle_actual() == 0);

  // Compare.
  return zx::ok(encoded_lhs.message().BytesMatch(encoded_rhs.message()));
}

void LogicalBufferCollection::SetFailedAllocationResult(Error error) {
  ZX_DEBUG_ASSERT(error != Error::kInvalid);

  // Only set result once.
  ZX_DEBUG_ASSERT(!has_allocation_result_);
  // maybe_allocation_error_ is initialized to std::nullopt, so should still be set that way.
  ZX_DEBUG_ASSERT(!maybe_allocation_error_.has_value());

  creation_timer_.Cancel();
  maybe_allocation_error_ = error;
  // Was initialized to nullptr.
  ZX_DEBUG_ASSERT(!allocation_result_info_.has_value());
  has_allocation_result_ = true;
  if (creation_timer_run_count_ != 0) {
    LogInfo(FROM_HERE, "allocation eventually failed after %" PRId64 " ms",
            (zx::clock::get_monotonic() - create_time_monotonic_).to_msecs());
  }
  SendAllocationResult(root_->BreadthFirstOrder());
  return;
}

void LogicalBufferCollection::SetAllocationResult(
    std::vector<NodeProperties*> visible_pruned_sub_tree,
    fuchsia_sysmem2::BufferCollectionInfo info,
    std::vector<NodeProperties*> whole_pruned_sub_tree) {
  // Setting empty constraints as the success case isn't allowed.  That's considered a failure.  At
  // least one participant must specify non-empty constraints.
  ZX_DEBUG_ASSERT(!info.IsEmpty());

  // Only set result once.
  ZX_DEBUG_ASSERT(!has_allocation_result_);

  // maybe_allocation_error_ is initialized to std::nullopt, so should still be set that way.
  ZX_ASSERT(!maybe_allocation_error_.has_value());

  creation_timer_.Cancel();
  allocation_result_info_.emplace(std::move(info));
  has_allocation_result_ = true;
  if (creation_timer_run_count_ != 0) {
    LogInfo(FROM_HERE, "allocation eventually succeeded after %" PRId64 " ms",
            (zx::clock::get_monotonic() - create_time_monotonic_).to_msecs());
  }
  SendAllocationResult(std::move(visible_pruned_sub_tree));

  std::vector<NodeProperties*>::reverse_iterator next;
  for (auto iter = whole_pruned_sub_tree.rbegin(); iter != whole_pruned_sub_tree.rend();
       iter = next) {
    next = iter + 1;
    auto& np = **iter;
    if (np.buffers_logically_allocated()) {
      // np is part of visible_pruned_sub_tree (or was, before the move above), so we're not failing
      // this item
      continue;
    }
    np.error_propagation_mode() = ErrorPropagationMode::kDoNotPropagate;
    // Using this error code helps make a failure-domain subtree under a group look as much like a
    // normal (failed in this case) allocation as possible from a client's point of view. This seems
    // better than a special error code for "different group child chosen", since that would
    // explicitly allow a client to tell whether its token was under a group, which could lead to
    // inconsistent client behavior when under a group or not.
    FailDownFrom(&np, Error::kConstraintsIntersectionEmpty);
  }
}

void LogicalBufferCollection::SendAllocationResult(std::vector<NodeProperties*> nodes) {
  ZX_DEBUG_ASSERT(has_allocation_result_);
  ZX_DEBUG_ASSERT(root_->buffer_collection_count() != 0);
  ZX_DEBUG_ASSERT(nodes[0] == root_.get());

  for (auto node_properties : nodes) {
    ZX_DEBUG_ASSERT(!node_properties->buffers_logically_allocated());
    node_properties->node()->OnBuffersAllocated(allocation_result());
    ZX_DEBUG_ASSERT(node_properties->buffers_logically_allocated());
  }

  if (maybe_allocation_error_.has_value()) {
    LogAndFailRootNode(FROM_HERE, *maybe_allocation_error_,
                       "LogicalBufferCollection::SendAllocationResult() done sending allocation "
                       "failure - now auto-failing self.");
    return;
  }
}

void LogicalBufferCollection::SetFailedLateLogicalAllocationResult(NodeProperties* tree,
                                                                   Error error) {
  ZX_DEBUG_ASSERT(error != Error::kInvalid);
  AllocationResult logical_allocation_result{
      .buffer_collection_info = nullptr,
      .maybe_error = error,
  };
  auto nodes_to_notify_and_fail = tree->BreadthFirstOrder();
  for (auto node_properties : nodes_to_notify_and_fail) {
    ZX_DEBUG_ASSERT(!node_properties->buffers_logically_allocated());
    node_properties->node()->OnBuffersAllocated(logical_allocation_result);
    ZX_DEBUG_ASSERT(node_properties->buffers_logically_allocated());
  }
  LogAndFailDownFrom(FROM_HERE, tree, error,
                     "AttachToken() sequence failed logical allocation - error: %u",
                     static_cast<uint32_t>(error));
}

void LogicalBufferCollection::SetSucceededLateLogicalAllocationResult(
    std::vector<NodeProperties*> visible_pruned_sub_tree,
    std::vector<NodeProperties*> whole_pruned_sub_tree) {
  ZX_DEBUG_ASSERT(!allocation_result().maybe_error.has_value());
  for (auto node_properties : visible_pruned_sub_tree) {
    ZX_DEBUG_ASSERT(!node_properties->buffers_logically_allocated());
    node_properties->node()->OnBuffersAllocated(allocation_result());
    ZX_DEBUG_ASSERT(node_properties->buffers_logically_allocated());
  }
  std::vector<NodeProperties*>::reverse_iterator next;
  for (auto iter = whole_pruned_sub_tree.rbegin(); iter != whole_pruned_sub_tree.rend();
       iter = next) {
    next = iter + 1;
    auto& np = **iter;
    if (np.buffers_logically_allocated()) {
      // np is part of visible_pruned_sub_tree, so we're not failing this item
      continue;
    }
    np.error_propagation_mode() = ErrorPropagationMode::kDoNotPropagate;
    // Using this error code makes a token under a group behave as much as possible like a token not
    // under a group.
    FailDownFrom(&np, Error::kConstraintsIntersectionEmpty);
  }
}

void LogicalBufferCollection::BindSharedCollectionInternal(
    BufferCollectionToken* token, CollectionServerEnd buffer_collection_request) {
  auto self = fbl::RefPtr(this);
  // This links the new collection into the tree under root_ in the same place as the token was, and
  // deletes the token.
  //
  // ~BufferCollectionToken calls UntrackTokenKoid().
  auto& collection = BufferCollection::EmplaceInTree(self, token, buffer_collection_request);
  token = nullptr;
  collection.SetErrorHandler([this, &collection](zx_status_t status) {
    // status passed to an error handler is never ZX_OK.  Clean close is
    // ZX_ERR_PEER_CLOSED.
    ZX_DEBUG_ASSERT(status != ZX_OK);

    // We know collection is still alive because collection is still under root_.  We know "this"
    // is still alive because collection has a fbl::RefPtr<> to "this".
    //
    // If collection isn't under root_, this check isn't going to be robust, but it's better than
    // nothing.  We could iterate the tree to verify it contains collection, but that'd be a lot for
    // just an assert.
    ZX_DEBUG_ASSERT(collection.node_properties().parent() ||
                    &collection.node_properties() == root_.get());

    // The BufferCollection may have had Close() called on it, in which case closure of the
    // BufferCollection doesn't cause LogicalBufferCollection failure.  Or, Close() wasn't called
    // and the BufferCollection node needs to fail, along with its failure domain and any child
    // failure domains.

    if (!(status == ZX_ERR_PEER_CLOSED && collection.is_done())) {
      // LogAndFailDownFrom() will also remove any no-longer-needed Node(s) from the tree.
      //
      // A collection whose error handler sees anything other than clean Close() (is_done() true)
      // implies LogicalBufferCollection failure.  The ability to detect unexpected closure is a
      // main reason we use a channel for BufferCollection instead of an eventpair.
      //
      // If a participant for some reason finds itself with an extra BufferCollection it doesn't
      // need, the participant should use Close() to avoid triggering this failure.
      NodeProperties* tree_to_fail = FindTreeToFail(&collection.node_properties());
      if (tree_to_fail == root_.get()) {
        // A LogicalBufferCollection intentionally treats any error (other than errors explicitly
        // ignored using SetDispensable() or AttachToken()) that might be triggered by a client
        // failure as a LogicalBufferCollection failure, because a LogicalBufferCollection can use a
        // lot of RAM and can tend to block creating a replacement LogicalBufferCollection.
        //
        // In rare cases, an initiator might choose to use Release()/Close() to avoid this failure,
        // but more typically initiators will just close their BufferCollection view without
        // Release()/Close() first, and this failure results.  This is considered acceptable partly
        // because it helps exercise code in participants that may see BufferCollection channel
        // closure before closure of related channels, and it helps get the VMO handles closed ASAP
        // to avoid letting those continue to use space of a MemoryAllocator's pool of pre-reserved
        // space (for example).
        //
        // We don't complain when a non-initiator participant closes first, since even if we prefer
        // that the initiator close first, the channels are separate so we could see some
        // reordering.
        FailDownFrom(tree_to_fail, Error::kUnspecified);
      } else {
        // This also removes the sub-tree, which can reduce SUM(min_buffer_count_for_camping) (or
        // similar for other constraints) to make room for a replacement sub-tree.  The replacement
        // sub-tree can be created with AttachToken().  The initial sub-tree may have been placed
        // in a separate failure domain by using SetDispensable() or AttachToken().
        //
        // Hopefully this won't be too noisy.
        LogAndFailDownFrom(FROM_HERE, tree_to_fail, Error::kUnspecified,
                           "BufferCollection failure causing sub-tree failure (SetDispensable() or "
                           "AttachToken() was used) - status: %d client: %s:%" PRIu64,
                           status, collection.node_properties().client_debug_info().name.c_str(),
                           collection.node_properties().client_debug_info().id);
      }
      return;
    }

    // At this point we know the collection is cleanly done (Close() was sent from client).  We keep
    // the NodeProperties for now though, in case there are children, and in case the
    // BufferCollection had SetDispensable() called on the token that led to this BufferCollection.
    //
    // We keep the collection's constraints (if any), as those are still relevant; this lets a
    // participant do SetConstraints() followed by Close() followed by closing the participant's
    // BufferCollection channel, which is convenient for some participants.
    //
    // If this causes zero remaining BufferCollectionToken(s) and zero remaining BufferCollection(s)
    // then LogicalBufferCollection can be deleted below.
    ZX_DEBUG_ASSERT(collection.is_done());
    auto self = fbl::RefPtr(this);
    ZX_DEBUG_ASSERT(self.get() == this);
    ZX_DEBUG_ASSERT(collection.shared_logical_buffer_collection().get() == this);
    // This also de-refs collection.
    (void)OrphanedNode::EmplaceInTree(self, &collection.node_properties());
    MaybeAllocate();
    // ~self may delete "this"
    return;
  });

  if (collection.create_status() != ZX_OK) {
    LogAndFailNode(FROM_HERE, &collection.node_properties(), Error::kUnspecified,
                   "token.status() failed - create_status: %d", collection.create_status());
    return;
  }

  collection.Bind(std::move(buffer_collection_request));
  // ~self
}

bool LogicalBufferCollection::IsMinBufferSizeSpecifiedByAnyParticipant(
    const ConstraintsList& constraints_list) {
  ZX_DEBUG_ASSERT(!constraints_list.empty());
  for (auto& entry : constraints_list) {
    auto& constraints = entry.constraints();
    if (constraints.buffer_memory_constraints().has_value() &&
        constraints.buffer_memory_constraints()->min_size_bytes().has_value() &&
        constraints.buffer_memory_constraints()->min_size_bytes().value() > 0) {
      return true;
    }
    if (constraints.image_format_constraints().has_value()) {
      for (auto& image_format_constraints : *constraints.image_format_constraints()) {
        if (image_format_constraints.min_size().has_value() &&
            image_format_constraints.min_size()->width() > 0 &&
            image_format_constraints.min_size()->height() > 0) {
          return true;
        }
        if (image_format_constraints.required_max_size().has_value() &&
            image_format_constraints.required_max_size()->width() > 0 &&
            image_format_constraints.required_max_size()->height() > 0) {
          return true;
        }
        if (image_format_constraints.required_max_size_list().has_value()) {
          for (auto& size : *image_format_constraints.required_max_size_list()) {
            if (size.width() > 0 && size.height() > 0) {
              return true;
            }
          }
        }
      }
    }
  }
  return false;
}

std::vector<NodeProperties*> LogicalBufferCollection::FailureDomainSubtrees() {
  if (!root_) {
    return std::vector<NodeProperties*>();
  }
  return root_->BreadthFirstOrder([](const NodeProperties& node_properties) {
    if (!node_properties.parent()) {
      return NodeFilterResult{.keep_node = true, .iterate_children = true};
    }
    if (node_properties.error_propagation_mode() != ErrorPropagationMode::kPropagate) {
      return NodeFilterResult{.keep_node = true, .iterate_children = true};
    }
    return NodeFilterResult{.keep_node = false, .iterate_children = true};
  });
}

// This could be more efficient, but should be fast enough as-is.
std::vector<NodeProperties*> LogicalBufferCollection::PrunedSubtreesEligibleForLogicalAllocation() {
  if (!root_) {
    return std::vector<NodeProperties*>();
  }
  return root_->BreadthFirstOrder([](const NodeProperties& node_properties) {
    if (node_properties.buffers_logically_allocated()) {
      return NodeFilterResult{.keep_node = false, .iterate_children = true};
    }
    if (node_properties.parent() && !node_properties.parent()->buffers_logically_allocated()) {
      return NodeFilterResult{.keep_node = false, .iterate_children = false};
    }
    ZX_DEBUG_ASSERT(!node_properties.parent() || node_properties.error_propagation_mode() ==
                                                     ErrorPropagationMode::kDoNotPropagate);
    return NodeFilterResult{.keep_node = true, .iterate_children = false};
  });
}

std::vector<NodeProperties*>
LogicalBufferCollection::NodesOfPrunedSubtreeEligibleForLogicalAllocation(NodeProperties& subtree) {
  ZX_DEBUG_ASSERT(!subtree.buffers_logically_allocated());
  ZX_DEBUG_ASSERT((&subtree == root_.get()) || subtree.parent()->buffers_logically_allocated());
  // Don't do anything during visit of each pruned subtree node, just keep all the pruned subtree
  // nodes in the returned.
  return subtree.BreadthFirstOrder(
      PrunedSubtreeFilter(subtree, [](const NodeProperties& node_properties) { return true; }));
}

fit::function<NodeFilterResult(const NodeProperties&)> LogicalBufferCollection::PrunedSubtreeFilter(
    NodeProperties& subtree, fit::function<bool(const NodeProperties&)> visit_keep) const {
  return [&subtree, visit_keep = std::move(visit_keep)](const NodeProperties& node_properties) {
    bool in_pruned_subtree = false;
    bool iterate_children = true;
    if (&node_properties != &subtree &&
        ((node_properties.error_propagation_mode() == ErrorPropagationMode::kDoNotPropagate) ||
         (node_properties.parent() && node_properties.parent()->which_child().has_value() &&
          &node_properties.parent()->child(*node_properties.parent()->which_child()) !=
              &node_properties))) {
      ZX_DEBUG_ASSERT(!in_pruned_subtree);
      iterate_children = false;
    } else {
      // We know we won't encounter any of the conditions checked above on the way from
      // node_properties to subtree (before reaching subtree) since parents are iterated before
      // children and we don't iterate any child of a node with any of the conditions checked above.
      in_pruned_subtree = true;
    }
    bool keep_node = false;
    if (in_pruned_subtree) {
      keep_node = visit_keep(node_properties);
    }
    return NodeFilterResult{.keep_node = keep_node, .iterate_children = iterate_children};
  };
}

std::vector<NodeProperties*>
LogicalBufferCollection::PrioritizedGroupsOfPrunedSubtreeEligibleForLogicalAllocation(
    NodeProperties& subtree) {
  ZX_DEBUG_ASSERT(!subtree.buffers_logically_allocated());
  ZX_DEBUG_ASSERT((&subtree == root_.get()) || subtree.parent()->buffers_logically_allocated());
  return subtree.DepthFirstPreOrder([&subtree](const NodeProperties& node_properties) {
    // This is only needed for a ZX_DEBUG_ASSERT() below.
    bool in_pruned_subtree = false;
    // By default we iterate children, but if we encounter kDoNotPropagate, we
    // don't iterate children since node_properties is no longer
    // in_pruned_subtree, and no children under node_properties are either.
    bool iterate_children = true;
    if (&node_properties != &subtree &&
        node_properties.error_propagation_mode() == ErrorPropagationMode::kDoNotPropagate) {
      // Groups can't have kDoNotPropagate, so we know iter is not a group.  We also know that
      // node_properties is not in_pruned_subtree since we encountered kDoNotPropagate between
      // node_properties and subtree (before reaching subtree).
      ZX_DEBUG_ASSERT(!node_properties.node()->buffer_collection_token_group());
      ZX_DEBUG_ASSERT(!in_pruned_subtree);
      iterate_children = false;
    } else {
      // We won't encounter any kDoNotPropagate from node_properties up to subtree (before reaching
      // subtree) because we only take this else path if the present node_properties is not
      // kDoNotPropagate, and because if a parent node of this node_properties other than subtree
      // were kDoNotPropagate, we wouldn't be iterating this child node of that parent node.
      in_pruned_subtree = true;
    }

    // The node_properties can be presently associated with an OrphanedNode; this is whether the
    // logical node is a group, not about whether the client is still connected.
    bool is_group = node_properties.is_token_group();
    // We can assert that all groups that we actually iterate to are in the
    // pruned subtree, since we iterate only from "subtree" down, and because
    // the nodes that we do potentially iterate which are not in the pruned
    // subtree must be non-group nodes because groups can't have kDoNoPropagate.
    // Since we don't iterate to any children of a kDoNotPropagate node, there's
    // no way for the DepthFirstPreOrder() to get called with node_properties
    // a group that is not in the pruned subtree.  If instead we did iterate
    // children of a kDoNotPropagate node, we could encounter some groups that
    // are not in_pruned_subtree, so we'd need to set
    // keep_node = in_pruned_subtree && is_group.  But since we don't iterate
    // children of a kDoNotPropagate node, we can just assert that it's
    // impossible to be iterating a group that is not in the pruned subtree, and
    // we don't need to include in_pruned_subtree in the keep_node expression
    // since we know that if is_group is true, in_pruned_subtree must also be
    // true, and if in_pruned_subtree is false, is_group is also false.
    ZX_DEBUG_ASSERT(!is_group || in_pruned_subtree);
    // The keep_node expressions with or without in_pruned_subtree will always
    // have the same value (in this method), so we can just use "in_group" (in
    // this method) since it's a simpler expression that accomplishes the same
    // thing (in this method, not necessarily in other methods that still need
    // in_pruned_subtree).
    ZX_DEBUG_ASSERT((in_pruned_subtree && is_group) == is_group);
    return NodeFilterResult{.keep_node = is_group, .iterate_children = iterate_children};
  });
}

fpromise::result<fuchsia_sysmem2::BufferCollectionConstraints, void>
LogicalBufferCollection::CombineConstraints(ConstraintsList* constraints_list) {
  ZX_DEBUG_ASSERT(!constraints_list->empty());

  // At least one participant must specify min buffer size (in terms of non-zero min buffer size or
  // non-zero min image size or non-zero potential max image size).
  //
  // This also enforces that at least one participant must specify non-empty constraints.
  if (!IsMinBufferSizeSpecifiedByAnyParticipant(*constraints_list)) {
    // Too unconstrained...  We refuse to allocate buffers without any min size
    // bounds from any participant.  At least one participant must provide
    // some form of size bounds (in terms of buffer size bounds or in terms
    // of image size bounds).
    LogError(FROM_HERE,
             "At least one participant must specify buffer_memory_constraints or "
             "image_format_constraints that implies non-zero min buffer size.");
    return fpromise::error();
  }

  // Start with empty constraints / unconstrained.
  fuchsia_sysmem2::BufferCollectionConstraints acc;
  // Sanitize initial accumulation target to keep accumulation simpler.  This is guaranteed to
  // succeed; the input is always the same.
  bool result = CheckSanitizeBufferCollectionConstraints(CheckSanitizeStage::kInitial, acc);
  ZX_DEBUG_ASSERT(result);
  // Accumulate each participant's constraints.
  while (!constraints_list->empty()) {
    Constraints constraints_entry = std::move(constraints_list->front());
    constraints_list->pop_front();
    current_node_properties_ = &constraints_entry.node_properties();
    auto defer_reset = fit::defer([this] { current_node_properties_ = nullptr; });
    if (!CheckSanitizeBufferCollectionConstraints(CheckSanitizeStage::kNotAggregated,
                                                  constraints_entry.mutate_constraints())) {
      return fpromise::error();
    }
    if (!AccumulateConstraintBufferCollection(&acc,
                                              std::move(constraints_entry.mutate_constraints()))) {
      // This is a failure.  The space of permitted settings contains no
      // points.
      return fpromise::error();
    }
  }

  if (acc.image_format_constraints().has_value()) {
    // This loop does not and must not eliminate any *acc.image_format_constraints() entries; that
    // happens above under AccumulateConstraintBufferCollection.
    for (auto& image_constraints : *acc.image_format_constraints()) {
      // list is present but may be empty
      ZX_DEBUG_ASSERT(image_constraints.required_max_size_list().has_value());
      // field is present but may be {0, 0}
      ZX_DEBUG_ASSERT(image_constraints.required_max_size().has_value());
      auto& required_max_size = *image_constraints.required_max_size();
      // an entry with 0 width or 0 height would be redundant
      if (required_max_size.width() != 0 && required_max_size.height() != 0) {
        // Until required_max_size field is removed (replaced by required_max_size_list), we copy
        // the aggregated required_max_size (max of widths and max of heights) to an additional
        // entry in required_max_size_list. This of course can go away when we remove
        // required_max_size.
        //
        // intentional copy; the required_max_size retains it's meaning and is sent by the server
        // for backwards compatibility until required_max_size is removed
        image_constraints.required_max_size_list()->emplace_back(required_max_size);
      }

      // remove redundant entries from required_max_size_list; this is done mainly to decrease the
      // chance of exceeding MAX_COUNT_IMAGE_FORMAT_CONSTRAINTS_REQUIRED_MAX_SIZE_LIST in the
      // response from the server
      {
        // this isn't the most optimized, but should be easy to read and understand
        std::vector<fuchsia_math::SizeU> new_list;
        std::vector<fuchsia_math::SizeU>& old_list = *image_constraints.required_max_size_list();
        auto redundant_with_list = [](const std::vector<fuchsia_math::SizeU>& list_to_check,
                                      const fuchsia_math::SizeU& candidate) -> bool {
          for (auto& other : list_to_check) {
            if (other.width() >= candidate.width() && other.height() >= candidate.height()) {
              // candidate is redundant with this entry in list_to_check
              return true;
            }
          }
          // candidate is not redundant with any entry in list_to_check
          return false;
        };
        // the number of redundant entries isn't large enough to make this reserve problematic
        new_list.reserve(old_list.size());
        while (!old_list.empty()) {
          auto candidate = old_list.back();
          old_list.pop_back();
          // we don't want to add a candidate to new_list if there's another old_list entry that's
          // at least as good (which we'll get to shortly), and we don't want to add candidate to
          // new_list if new_list already has an entry that's at least as good
          bool is_candidate_redundant =
              redundant_with_list(old_list, candidate) || redundant_with_list(new_list, candidate);
          if (is_candidate_redundant) {
            continue;
          }
          new_list.push_back(candidate);
        }
        image_constraints.required_max_size_list() = new_list;
      }

      if (image_constraints.required_max_size_list()->size() >
          fuchsia_sysmem2::kMaxCountImageFormatConstraintsRequiredMaxSizeList) {
        // This failure case is unlikely given this threshold and expected usage. If we fail this
        // way in a real situation we could increase the threshold if we don't see a way to create a
        // more suitable (less verbose) constraint for that situation.
        //
        // We intentionally don't treat this as elimination of an entry in
        // acc.image_format_constraints(), as this isn't about aggregation failure - this is just a
        // limitation of the fidl field's ability to communicate an aggregation result. We also
        // intentionally don't determine which entry of acc.image_format_constraints() we're going
        // to select for allocation before checking this, because that would mess with overall
        // ordering too much, and this error should be too unlikely to justify this driving an
        // overall sequencing change.
        //
        // Also, even if we're not going to pick this entry of acc.image_format_constraints(), this
        // occurring is as likely as not to be a client under development doing something
        // unintentional, so failing here is probably best anyway.
        LogError(FROM_HERE, "required_max_size_list from server has too many entries for fidl: %zu",
                 image_constraints.required_max_size_list()->size());
        return fpromise::error();
      }
    }
  }

  ZX_DEBUG_ASSERT_MSG(
      !acc.buffer_memory_constraints().value().secure_required().value() ||
          parent_sysmem_->is_secure_mem_ready(),
      "CombineConstraints called too soon before parent_sysmem_->is_secure_mem_ready()");

  if (!CheckSanitizeBufferCollectionConstraints(CheckSanitizeStage::kAggregated, acc)) {
    return fpromise::error();
  }

  if (is_verbose_logging()) {
    LogInfo(FROM_HERE, "After combining constraints:");
    LogConstraints(FROM_HERE, nullptr, acc);
  }

  return fpromise::ok(std::move(acc));
}

// TODO(dustingreen): Consider rejecting secure_required + any non-secure heaps, including the
// potentially-implicit SYSTEM_RAM heap.
//
// TODO(dustingreen): From a particular participant, CPU usage without
// IsCpuAccessibleHeapPermitted() should fail.
//
// TODO(dustingreen): From a particular participant, be more picky about which domains are supported
// vs. which heaps are supported.
static bool IsPermittedHeap(const fuchsia_sysmem2::BufferMemoryConstraints& constraints,
                            const fuchsia_sysmem2::Heap& heap) {
  if (constraints.permitted_heaps()->size()) {
    for (auto iter = constraints.permitted_heaps().value().begin();
         iter != constraints.permitted_heaps().value().end(); ++iter) {
      if (heap == *iter) {
        return true;
      }
    }
    return false;
  }
  // Zero heaps in heap_permitted() means any heap is ok.
  return true;
}

static bool IsCpuAccessSupported(const fuchsia_sysmem2::BufferMemoryConstraints& constraints) {
  return constraints.cpu_domain_supported().value() || constraints.ram_domain_supported().value();
}

bool LogicalBufferCollection::CheckSanitizeBufferUsage(CheckSanitizeStage stage,
                                                       fuchsia_sysmem2::BufferUsage& buffer_usage) {
  FIELD_DEFAULT_ZERO(buffer_usage, none);
  FIELD_DEFAULT_ZERO(buffer_usage, cpu);
  FIELD_DEFAULT_ZERO(buffer_usage, vulkan);
  FIELD_DEFAULT_ZERO(buffer_usage, display);
  FIELD_DEFAULT_ZERO(buffer_usage, video);
  switch (stage) {
    case CheckSanitizeStage::kInitial:
      // empty usage is allowed for kInitial
      break;
    case CheckSanitizeStage::kNotAggregated:
      // At least one usage bit must be specified by any participant that
      // specifies constraints.  The "none" usage bit can be set by a participant
      // that doesn't directly use the buffers, so we know that the participant
      // didn't forget to set usage.
      if (*buffer_usage.none() == 0 && *buffer_usage.cpu() == 0 && *buffer_usage.vulkan() == 0 &&
          *buffer_usage.display() == 0 && *buffer_usage.video() == 0) {
        LogError(FROM_HERE, "At least one usage bit must be set by a participant.");
        return false;
      }
      if (*buffer_usage.none() != 0) {
        if (*buffer_usage.cpu() != 0 || *buffer_usage.vulkan() != 0 ||
            *buffer_usage.display() != 0 || *buffer_usage.video() != 0) {
          LogError(FROM_HERE,
                   "A participant indicating 'none' usage can't specify any other usage.");
          return false;
        }
        if ((*buffer_usage.none() & fuchsia_sysmem2::kNoneUsagePermitAllocation) &&
            !(*buffer_usage.none() & fuchsia_sysmem2::kNoneUsage)) {
          LogError(FROM_HERE, "NONE_USAGE_PERMIT_ALLOCATION requires NONE_USAGE");
          return false;
        }
      }
      break;
    case CheckSanitizeStage::kAggregated:
      if (!IsNoneUsagePermitAllocation(buffer_usage) && *buffer_usage.cpu() == 0 &&
          *buffer_usage.vulkan() == 0 && *buffer_usage.display() == 0 &&
          *buffer_usage.video() == 0) {
        LogError(FROM_HERE,
                 "At least one non-'none' usage bit must be set across all participants.");
        return false;
      }
      break;
  }
  return true;
}

size_t LogicalBufferCollection::InitialCapacityOrZero(CheckSanitizeStage stage,
                                                      size_t initial_capacity) {
  return (stage == CheckSanitizeStage::kInitial) ? initial_capacity : 0;
}

// Nearly all constraint checks must go under here or under ::Allocate() (not in the Accumulate*
// methods), else we could fail to notice a single participant providing unsatisfiable constraints,
// where no Accumulate* happens.  The constraint checks that are present under Accumulate* are
// commented explaining why it's ok for them to be there.
bool LogicalBufferCollection::CheckSanitizeBufferCollectionConstraints(
    CheckSanitizeStage stage, fuchsia_sysmem2::BufferCollectionConstraints& constraints) {
  bool was_empty = constraints.IsEmpty();
  FIELD_DEFAULT_SET(constraints, usage);
  if (was_empty) {
    // Completely empty constraints are permitted, so convert to NONE_USAGE to avoid triggering the
    // check applied to non-empty constraints where at least one usage bit must be set (NONE_USAGE
    // counts for that check, and doesn't constrain anything).
    FIELD_DEFAULT(constraints.usage().value(), none, fuchsia_sysmem2::kNoneUsage);
  }
  FIELD_DEFAULT_ZERO(constraints, min_buffer_count_for_camping);
  FIELD_DEFAULT_ZERO(constraints, min_buffer_count_for_dedicated_slack);
  FIELD_DEFAULT_ZERO(constraints, min_buffer_count_for_shared_slack);
  FIELD_DEFAULT_ZERO(constraints, min_buffer_count);
  FIELD_DEFAULT_MAX(constraints, max_buffer_count);
  ZX_DEBUG_ASSERT(constraints.buffer_memory_constraints().has_value() ||
                  stage != CheckSanitizeStage::kAggregated);
  FIELD_DEFAULT_SET(constraints, buffer_memory_constraints);
  ZX_DEBUG_ASSERT(constraints.buffer_memory_constraints().has_value());

  // The upside of this default is treating an empty vector from a client the same way as un-set
  // from the client. The downside is we have to be careful to notice when the size() moves from 1
  // to 0, which is an error, while size() 0 from the start is not an error.
  FIELD_DEFAULT_SET_VECTOR(constraints, image_format_constraints, InitialCapacityOrZero(stage, 64));

  if (!CheckSanitizeBufferUsage(stage, constraints.usage().value())) {
    LogError(FROM_HERE, "CheckSanitizeBufferUsage() failed");
    return false;
  }
  if (*constraints.max_buffer_count() == 0) {
    LogError(FROM_HERE, "max_buffer_count == 0");
    return false;
  }
  if (*constraints.min_buffer_count() > *constraints.max_buffer_count()) {
    LogError(FROM_HERE, "min_buffer_count > max_buffer_count");
    return false;
  }
  if (!CheckSanitizeBufferMemoryConstraints(stage, constraints.usage().value(),
                                            constraints.buffer_memory_constraints().value())) {
    return false;
  }
  if (stage != CheckSanitizeStage::kAggregated) {
    if (IsCpuUsage(constraints.usage().value())) {
      if (!IsCpuAccessSupported(constraints.buffer_memory_constraints().value())) {
        LogError(FROM_HERE, "IsCpuUsage() && !IsCpuAccessSupported()");
        return false;
      }
      // From a single participant, reject secure_required in combination with CPU usage, since CPU
      // usage isn't possible given secure memory.
      if (constraints.buffer_memory_constraints()->secure_required().value()) {
        LogError(FROM_HERE, "IsCpuUsage() && secure_required");
        return false;
      }
      // It's fine if a participant sets CPU usage but also permits inaccessible domain and possibly
      // IsSecurePermitted().  In that case the participant is expected to pay attention to the
      // coherency domain and is_secure and realize that it shouldn't attempt to read/write the
      // VMOs.
    }
    if (constraints.buffer_memory_constraints()->secure_required().value() &&
        IsCpuAccessSupported(constraints.buffer_memory_constraints().value())) {
      // This is a little picky, but easier to be less picky later than more picky later.
      LogError(FROM_HERE, "secure_required && IsCpuAccessSupported()");
      return false;
    }
  }

  if (stage == CheckSanitizeStage::kNotAggregated) {
    // At least for now, always flatten pixel_formats and pixel_format_modifiers. While it may be
    // tempting to make aggregation aware of `pixel_formats` and `pixel_format_modifiers` to avoid
    // this pre-flattening, that would likely increase the complexity of constraints aggregation
    // quite a bit. Another reasonable-seeming possibility would be some pre-pruning before
    // flattening.
    if (!FlattenPixelFormatAndModifiers(*constraints.usage(), constraints)) {
      // FlattenPixelFormatAndModifiers already logged.
      return false;
    }
  }

  ZX_DEBUG_ASSERT(constraints.image_format_constraints().has_value());
  auto& image_format_constraints = *constraints.image_format_constraints();

  for (uint32_t i = 0; i < image_format_constraints.size(); ++i) {
    ZX_DEBUG_ASSERT(constraints.usage().has_value());
    if (!CheckSanitizeImageFormatConstraints(stage, *constraints.usage(),
                                             image_format_constraints[i])) {
      return false;
    }
  }

  if (stage == CheckSanitizeStage::kAggregated) {
    // filter out any remaining PixelFormatAndModifier DoNotCare(s)
    for (uint32_t i = 0; i < image_format_constraints.size(); ++i) {
      auto& ifc = image_format_constraints[i];
      auto format = PixelFormatAndModifierFromConstraints(ifc);
      if (!IsPixelFormatAndModifierAtLeastPartlyDoNotCare(format)) {
        continue;
      }
      LogInfo(
          FROM_HERE,
          "per-PixelFormatAndModifier, at least one participant must specify specific value (not DoNotCare) for each field - removing format: %u 0x%" PRIx64,
          format.pixel_format, format.pixel_format_modifier);

      if (i != image_format_constraints.size() - 1) {
        image_format_constraints[i] =
            std::move(image_format_constraints[image_format_constraints.size() - 1]);
      }
      image_format_constraints.resize(image_format_constraints.size() - 1);
      --i;

      // 1 -> 0 is an error (in contrast to initially 0 which is not an error)
      if (image_format_constraints.empty()) {
        // By design, sysmem does not arbitrarily select a colorspace from among all color spaces
        // without any participant-specified pixel format constraints, as doing so would be likely
        // to lead to unexpected changes to the resulting pixel format when additional pixel formats
        // are added to PixelFormat.
        LogError(
            FROM_HERE,
            "after removing format(s) with remaining DoNotCare(s), zero PixelFormatAndModifier(s) remaining");
        return false;
      }
    }

    for (uint32_t i = 0; i < image_format_constraints.size(); ++i) {
      auto& ifc = image_format_constraints[i];
      auto is_color_space_do_not_care_result = IsColorSpaceArrayDoNotCare(*ifc.color_spaces());
      // maintained during accumulation
      ZX_DEBUG_ASSERT(is_color_space_do_not_care_result.is_ok());
      bool is_color_space_do_not_care = is_color_space_do_not_care_result.value();
      if (is_color_space_do_not_care) {
        // Both producers and consumers ("active" participants) are required to specify specific
        // color_spaces by design, with the only exception being kPassThrough.
        //
        // Only more passive participants should ever specify ColorSpaceType kDoNotCare.  If an
        // active participant really does not care, it can instead list all the color spaces.  In
        // a few scenarios it may be fine for participants to all specify kPassThrough, if there's
        // no reason to add a particular highly-custom and/or not-actually-color-as-such space to
        // the ColorSpaceType enum, or if all participants are all truly intending to just pass
        // through the color space no matter what it is with no need for sysmem to select a color
        // space and the special scenario would otherwise involve (by intent of design) _all_ the
        // participants wanting to set kDoNotCare (which would lead to this error).
        LogInfo(FROM_HERE,
                "per-PixelFormat, at least one participant must specify ColorSpaceType != "
                "kDoNotCare - removing: pixel_format: %u pixel_format_modifier: 0x%" PRIx64,
                *ifc.pixel_format(),
                ifc.pixel_format_modifier().has_value()
                    ? fidl::ToUnderlying(*ifc.pixel_format_modifier())
                    : 0ull);

        // Remove by moving down last PixelFormat to this index and processing this index again,
        // if this isn't already the last PixelFormat.
        if (i != image_format_constraints.size() - 1) {
          image_format_constraints[i] =
              std::move(image_format_constraints[image_format_constraints.size() - 1]);
        }
        image_format_constraints.resize(image_format_constraints.size() - 1);
        --i;

        // 1 -> 0 is an error (in contrast to initially 0 which is not an error)
        if (image_format_constraints.empty()) {
          // The preferred fix most of the time is specifying a specific color space in at least
          // one participant.  Much less commonly, and only if actually necessary, kPassThrough
          // can be used by all participants instead (see previous paragraph).
          LogError(
              FROM_HERE,
              "after removing format(s) that remained ColorSpace kDoNotCare, zero formats remaining");
          return false;
        }
      }
    }

    // Given the image constriant's pixel format, select the best color space
    for (auto& image_constraint : constraints.image_format_constraints().value()) {
      // We are guaranteed that color spaces are valid for the current pixel format
      if (image_constraint.color_spaces().has_value()) {
        auto best_color_space = fuchsia_images2::ColorSpace::kInvalid;

        for (auto& color_space : image_constraint.color_spaces().value()) {
          auto best_ranking = kColorSpaceRanking.at(sysmem::fidl_underlying_cast(best_color_space));
          auto current_ranking = kColorSpaceRanking.at(sysmem::fidl_underlying_cast(color_space));

          if (best_ranking > current_ranking) {
            best_color_space = color_space;
          }
        }

        // Set the best color space
        image_constraint.color_spaces() = {best_color_space};
      }
    }
  }

  if (stage == CheckSanitizeStage::kNotAggregated) {
    // As an optimization, only check the unaggregated inputs.
    for (uint32_t i = 0; i < constraints.image_format_constraints()->size(); ++i) {
      for (uint32_t j = i + 1; j < constraints.image_format_constraints()->size(); ++j) {
        auto i_pixel_format_and_modifier =
            PixelFormatAndModifierFromConstraints(constraints.image_format_constraints()->at(i));
        auto j_pixel_format_and_modifier =
            PixelFormatAndModifierFromConstraints(constraints.image_format_constraints()->at(j));
        if (ImageFormatIsPixelFormatEqual(i_pixel_format_and_modifier,
                                          j_pixel_format_and_modifier)) {
          // This can happen if a client is specifying two entries with same pixel_format and same
          // pixel_format_modifier.
          //
          // This can also happen if such a duplicate is implied after default field handling, such
          // as replacing "none" BufferUsage + un-set pixel_format_modifier with
          // pixel_format_modifier kDoNotCare, along with another entry from the client with same
          // pixel_format and explicit pixel_format_modifier kDoNotCare.
          //
          // Printing all four values is a bit redundant but perhaps more convincing.
          LogError(FROM_HERE, "image formats are identical: %u 0x%" PRIx64 " and %u 0x%" PRIx64,
                   static_cast<uint32_t>(i_pixel_format_and_modifier.pixel_format),
                   fidl::ToUnderlying(i_pixel_format_and_modifier.pixel_format_modifier),
                   static_cast<uint32_t>(j_pixel_format_and_modifier.pixel_format),
                   fidl::ToUnderlying(j_pixel_format_and_modifier.pixel_format_modifier));
          return false;
        }

        if (IsPixelFormatAndModifierCombineable(i_pixel_format_and_modifier,
                                                j_pixel_format_and_modifier)) {
          // Given the ImageFormatIsPixelFormatEqual check above, this can happen if a client is
          // specifying two entries where one entry is strictly less picky, so "covering" the other
          // entry. In this case the client should only send the less picky entry. Sysmem doesn't
          // have any policy of trying more picky entries then falling back to less picky entries,
          // short of using a BufferCollectionTokenGroup (please only use that if it's really
          // needed; prefer to just remove the more-picky entry if that'll work fine for the
          // client).
          //
          // This check also ensures that a (PixelFormat.DO_NOT_CARE,
          // PixelFormatModifier.DO_NOT_CARE) is the only entry, since that's combine-able with any
          // other format (including itself).
          //
          // This can also happen in case of (foo, do not care) and (do not care, bar), which is
          // fine to combine from separate participants (despite no single participant having nailed
          // down both fields at once), but messes with the "pick one" semantics of format entries
          // from a single participant, so is disallowed within the format entries of a single
          // participant.
          //
          // We intentionally disallow any combine-able formats from a single participant here. It's
          // easy enough for a participant that really needs/wants to pick one from set A and one
          // from set B to duplicate a token and send set A and set B via separate "participants"
          // driven by the one client.
          LogError(
              FROM_HERE,
              "not permitted for two formats from the same participant to be combine-able: %u 0x%" PRIx64
              " and %u 0x%" PRIx64,
              static_cast<uint32_t>(i_pixel_format_and_modifier.pixel_format),
              fidl::ToUnderlying(i_pixel_format_and_modifier.pixel_format_modifier),
              static_cast<uint32_t>(j_pixel_format_and_modifier.pixel_format),
              fidl::ToUnderlying(j_pixel_format_and_modifier.pixel_format_modifier));
          return false;
        }
      }
    }
  }

  return true;
}

bool LogicalBufferCollection::CheckSanitizeHeap(CheckSanitizeStage stage,
                                                fuchsia_sysmem2::Heap& heap) {
  if (!heap.heap_type().has_value()) {
    LogError(FROM_HERE, "heap_type field must be set");
    return false;
  }
  FIELD_DEFAULT_ZERO_64_BIT(heap, id);
  return true;
}

bool LogicalBufferCollection::CheckSanitizeBufferMemoryConstraints(
    CheckSanitizeStage stage, const fuchsia_sysmem2::BufferUsage& buffer_usage,
    fuchsia_sysmem2::BufferMemoryConstraints& constraints) {
  if (stage != CheckSanitizeStage::kAggregated) {
    if (constraints.min_physical_base_alignment().has_value()) {
      if (!constraints.physically_contiguous_required().has_value() ||
          !*constraints.physically_contiguous_required()) {
        LogError(
            FROM_HERE,
            "constraints.min_physical_base_alignment field set when physically_contiguous_required un-set or false");
        return false;
      }
      uint64_t alignment = *constraints.min_physical_base_alignment();
      if (alignment == 0) {
        LogError(FROM_HERE, "constraints.min_physical_base_alignment field must not be set to 0");
        return false;
      }
      if ((alignment & (alignment - 1)) != 0) {
        LogError(FROM_HERE, "constraints.min_physical_base_alignment must be a power of 2 if set");
        return false;
      }
    }
  }
  FIELD_DEFAULT_ZERO(constraints, min_size_bytes);
  FIELD_DEFAULT_MAX(constraints, max_size_bytes);
  FIELD_DEFAULT_FALSE(constraints, physically_contiguous_required);
  if (*constraints.physically_contiguous_required()) {
    FIELD_DEFAULT(constraints, min_physical_base_alignment, 1u);
  }
  FIELD_DEFAULT_FALSE(constraints, secure_required);
  // The CPU domain is supported by default, unless secure_required.
  FIELD_DEFAULT(constraints, cpu_domain_supported, !*constraints.secure_required());
  // If !usage.cpu, then participant doesn't care what domain, so indicate support for RAM and
  // inaccessible domains in that case, unless secure_required. This only takes effect if the
  // participant didn't explicitly specify a value for these fields.
  FIELD_DEFAULT(constraints, ram_domain_supported,
                !*constraints.secure_required() && !*buffer_usage.cpu());
  FIELD_DEFAULT(constraints, inaccessible_domain_supported, !*buffer_usage.cpu());
  if (stage != CheckSanitizeStage::kAggregated) {
    if (constraints.permitted_heaps().has_value() && constraints.permitted_heaps()->empty()) {
      LogError(FROM_HERE,
               "constraints.has_heap_permitted() && constraints.heap_permitted().empty()");
      return false;
    }
  }
  // TODO(dustingreen): When 0 heaps specified, constrain heap list based on other constraints.
  // For now 0 heaps means any heap.
  FIELD_DEFAULT_SET_VECTOR(constraints, permitted_heaps, 0);
  ZX_DEBUG_ASSERT(stage != CheckSanitizeStage::kInitial || constraints.permitted_heaps()->empty());
  for (auto& heap : constraints.permitted_heaps().value()) {
    if (!CheckSanitizeHeap(stage, heap)) {
      // CheckSanitizeHeap already logged
      return false;
    }
  }

  if (*constraints.min_size_bytes() > *constraints.max_size_bytes()) {
    LogError(FROM_HERE, "min_size_bytes > max_size_bytes");
    return false;
  }
  // When/if secure_required() && !is_secure_mem_ready(), we'll check this again later after
  // is_secure_mem_ready() becomes true (as called during CombineConstraints which only gets called
  // with !secure_required() || is_secure_mem_ready()).
  ZX_DEBUG_ASSERT(stage != CheckSanitizeStage::kAggregated ||
                  (!*constraints.secure_required() || parent_sysmem_->is_secure_mem_ready()));
  if (*constraints.secure_required() && parent_sysmem_->is_secure_mem_ready() &&
      !IsSecurePermitted(constraints)) {
    LogError(FROM_HERE, "secure memory required but not permitted");
    return false;
  }

  return true;
}

bool LogicalBufferCollection::CheckSanitizeImageFormatConstraints(
    CheckSanitizeStage stage, const fuchsia_sysmem2::BufferUsage& buffer_usage,
    fuchsia_sysmem2::ImageFormatConstraints& constraints) {
  // We never CheckSanitizeImageFormatConstraints() on empty (aka initial) constraints.
  ZX_DEBUG_ASSERT(stage != CheckSanitizeStage::kInitial);

  // Defaults for these fields are set in FlattenPixelFormatAndModifiers().
  ZX_DEBUG_ASSERT(constraints.pixel_format().has_value());
  ZX_DEBUG_ASSERT(constraints.pixel_format_modifier().has_value());

  if (*constraints.pixel_format() == fuchsia_images2::PixelFormat::kInvalid) {
    // kInvalid not allowed; see kDoNotCare if that is the intent.
    LogError(FROM_HERE, "PixelFormat INVALID not allowed");
    return false;
  }

  if (*constraints.pixel_format_modifier() == fuchsia_images2::PixelFormatModifier::kInvalid) {
    // kFormatModifierInvalid not allowed; see kFormatModifierDoNotCare if that is the intent.
    LogError(FROM_HERE, "pixel_format_modifier kFormatModifierInvalid not allowed");
    return false;
  }

  FIELD_DEFAULT_SET_VECTOR(constraints, color_spaces, 0);

  if (!constraints.min_size().has_value()) {
    constraints.min_size() = {0, 0};
  }

  if (!constraints.max_size().has_value()) {
    constraints.max_size() = {std::numeric_limits<uint32_t>::max(),
                              std::numeric_limits<uint32_t>::max()};
  }

  FIELD_DEFAULT_ZERO(constraints, min_bytes_per_row);
  FIELD_DEFAULT_MAX(constraints, max_bytes_per_row);
  FIELD_DEFAULT_MAX(constraints, max_width_times_height);

  if (!constraints.size_alignment().has_value()) {
    constraints.size_alignment() = {1, 1};
  }
  if (!IsNonZeroPowerOf2(constraints.size_alignment()->width())) {
    LogError(FROM_HERE, "non-power-of-2 size_alignment.width not supported");
    return false;
  }
  if (!IsNonZeroPowerOf2(constraints.size_alignment()->height())) {
    LogError(FROM_HERE, "non-power-of-2 size_alignment.height not supported");
    return false;
  }
  constraints.min_size()->width() =
      std::max(constraints.min_size()->width(), constraints.size_alignment()->width());
  constraints.min_size()->height() =
      std::max(constraints.min_size()->height(), constraints.size_alignment()->height());

  if (!constraints.display_rect_alignment().has_value()) {
    constraints.display_rect_alignment() = {1, 1};
  }

  if (!constraints.required_min_size().has_value()) {
    constraints.required_min_size() = {std::numeric_limits<uint32_t>::max(),
                                       std::numeric_limits<uint32_t>::max()};
  }

  // The required_max_size is deprecated but we retain the behavior until the field is removed.
  if (!constraints.required_max_size().has_value()) {
    constraints.required_max_size() = {0, 0};
  }

  // the kAggregated stage always has required_max_size_list.has_value()
  ZX_DEBUG_ASSERT(stage != CheckSanitizeStage::kAggregated ||
                  constraints.required_max_size_list().has_value());
  if (stage != CheckSanitizeStage::kAggregated) {
    if (!constraints.required_max_size_list().has_value()) {
      // default empty list
      constraints.required_max_size_list().emplace();
    }
  }
  ZX_DEBUG_ASSERT(constraints.required_max_size_list().has_value());
  // this won't fail kInitial stage since it has no entries
  for (auto& required_max_size : *constraints.required_max_size_list()) {
    if (required_max_size.width() == 0 || required_max_size.height() == 0) {
      LogError(FROM_HERE,
               "both required_max_size.width and height must not be 0 - from client: {%u, %u}",
               required_max_size.width(), required_max_size.height());
      return false;
    }
  }

  // "Packed" by default (for example, kBgr24 3 bytes per pixel is allowed to have zero padding per
  // row of pixels and just continue with the next row of pixels in the very next byte even if that
  // next row of pixels' offset from start of buffer and memory address of the start of the next row
  // is odd).
  FIELD_DEFAULT_1(constraints, bytes_per_row_divisor);
  FIELD_DEFAULT_FALSE(constraints, require_bytes_per_row_at_pixel_boundary);

  FIELD_DEFAULT_1(constraints, start_offset_divisor);

  if (!constraints.pad_for_block_size().has_value()) {
    constraints.pad_for_block_size() = {1, 1};
  }
  FIELD_DEFAULT_ZERO(constraints, pad_beyond_image_size_bytes);

  auto pixel_format_and_modifier = PixelFormatAndModifierFromConstraints(constraints);
  auto is_format_do_not_care =
      IsPixelFormatAndModifierAtLeastPartlyDoNotCare(pixel_format_and_modifier);

  if (!is_format_do_not_care) {
    if (!ImageFormatIsSupported(pixel_format_and_modifier)) {
      LogError(FROM_HERE,
               "Unsupported pixel format - pixel_format: %u pixel_format_modifier: 0x%" PRIx64,
               static_cast<uint32_t>(pixel_format_and_modifier.pixel_format),
               fidl::ToUnderlying(pixel_format_and_modifier.pixel_format_modifier));
      return false;
    }
    uint32_t stride_bytes_per_width_pixel =
        ImageFormatStrideBytesPerWidthPixel(pixel_format_and_modifier);
    uint32_t sample_alignment = ImageFormatSampleAlignment(pixel_format_and_modifier);

    constraints.size_alignment()->width() =
        std::max(constraints.size_alignment()->width(),
                 ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier));
    constraints.size_alignment()->height() =
        std::max(constraints.size_alignment()->height(),
                 ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier));

    auto combine_bytes_per_row_divisor_result =
        CombineBytesPerRowDivisor(*constraints.bytes_per_row_divisor(), sample_alignment);
    if (!combine_bytes_per_row_divisor_result.is_ok()) {
      LogError(FROM_HERE, "!combine_bytes_per_row_divisor_result.is_ok()");
      return false;
    }
    constraints.bytes_per_row_divisor() = *combine_bytes_per_row_divisor_result;

    if (*constraints.require_bytes_per_row_at_pixel_boundary()) {
      auto combine_bytes_per_row_divisor_result = CombineBytesPerRowDivisor(
          *constraints.bytes_per_row_divisor(), stride_bytes_per_width_pixel);
      if (!combine_bytes_per_row_divisor_result.is_ok()) {
        LogError(FROM_HERE, "!combine_bytes_per_row_divisor_result.is_ok()");
        return false;
      }
      constraints.bytes_per_row_divisor() = *combine_bytes_per_row_divisor_result;
    }

    constraints.start_offset_divisor() =
        std::max(*constraints.start_offset_divisor(), sample_alignment);

    constraints.min_bytes_per_row() =
        std::max(*constraints.min_bytes_per_row(), *constraints.bytes_per_row_divisor());
    ZX_DEBUG_ASSERT(constraints.min_size()->width() > 0);
    uint32_t minimum_row_bytes = 0;
    // If this call fails, it means we'll be failing the collection soon anyway, but don't fail
    // the collection here since this error would be less specific in the log.
    if (ImageFormatMinimumRowBytes(constraints, constraints.min_size()->width(),
                                   &minimum_row_bytes)) {
      constraints.min_bytes_per_row() =
          std::max(constraints.min_bytes_per_row().value(), minimum_row_bytes);
    }
  }

  // Avoid requiring clients to round up if the client is just using a min as-is. This must be done
  // after adjustments to the divisor(s) and alignment(s) (above) and before min > max is checked
  // for (below).
  constraints.min_size()->width() =
      fbl::round_up(constraints.min_size()->width(), constraints.size_alignment()->width());
  constraints.min_size()->height() =
      fbl::round_up(constraints.min_size()->height(), constraints.size_alignment()->height());
  constraints.min_bytes_per_row() =
      fbl::round_up(*constraints.min_bytes_per_row(), *constraints.bytes_per_row_divisor());

  // Avoid claiming a max is supported if that max isn't conformant with divisor(s) and
  // alignment(s). The value 0xFFFFFFFF is left alone, despite it being odd, since that value is
  // documented as the default if there's no explicit limit and some clients may check for and/or
  // expect 0xFFFFFFFF when no participant specifies a max.
  if (constraints.max_size()->width() != std::numeric_limits<uint32_t>::max()) {
    constraints.max_size()->width() =
        fbl::round_down(constraints.max_size()->width(), constraints.size_alignment()->width());
  }
  if (constraints.max_size()->height() != std::numeric_limits<uint32_t>::max()) {
    constraints.max_size()->height() =
        fbl::round_down(constraints.max_size()->height(), constraints.size_alignment()->height());
  }
  if (constraints.max_bytes_per_row() != std::numeric_limits<uint32_t>::max()) {
    constraints.max_bytes_per_row() =
        fbl::round_down(*constraints.max_bytes_per_row(), *constraints.bytes_per_row_divisor());
  }

  if (!constraints.color_spaces().has_value()) {
    LogError(FROM_HERE, "!color_spaces.has_value() not allowed");
    return false;
  }
  if (constraints.color_spaces()->empty()) {
    LogError(FROM_HERE, "color_spaces.empty() not allowed");
    return false;
  }

  if (constraints.min_size()->width() > constraints.max_size()->width()) {
    LogError(FROM_HERE, "min_size.width > max_size.width");
    return false;
  }
  if (constraints.min_size()->height() > constraints.max_size()->height()) {
    LogError(FROM_HERE, "min_size.height > max_size.height");
    return false;
  }

  if (constraints.min_bytes_per_row().value() > constraints.max_bytes_per_row().value()) {
    LogError(FROM_HERE, "min_bytes_per_row > max_bytes_per_row");
    return false;
  }

  uint32_t min_width_times_min_height;
  if (!CheckMul(constraints.min_size()->width(), constraints.min_size()->height())
           .AssignIfValid(&min_width_times_min_height)) {
    LogError(FROM_HERE, "min_size.width * min_size.height failed");
    return false;
  }
  if (min_width_times_min_height > constraints.max_width_times_height().value()) {
    LogError(FROM_HERE, "min_width_times_min_height > max_width_times_height");
    return false;
  }

  if (!IsNonZeroPowerOf2(constraints.display_rect_alignment()->width())) {
    LogError(FROM_HERE, "non-power-of-2 display_rect_alignment.width not supported");
    return false;
  }
  if (!IsNonZeroPowerOf2(constraints.display_rect_alignment()->height())) {
    LogError(FROM_HERE, "non-power-of-2 display_rect_alignment.height not supported");
    return false;
  }

  if (*constraints.bytes_per_row_divisor() == 0) {
    // instead, leave field un-set or set to a value >= 1
    LogError(FROM_HERE, "bytes_per_row_divisor set to 0 not permitted");
    return false;
  }

  if (!IsNonZeroPowerOf2(*constraints.start_offset_divisor())) {
    LogError(FROM_HERE, "non-power-of-2 start_offset_divisor not supported");
    return false;
  }
  if (*constraints.start_offset_divisor() > zx_system_get_page_size()) {
    LogError(FROM_HERE,
             "support for start_offset_divisor > zx_system_get_page_size() not yet implemented");
    return false;
  }

  auto is_color_space_do_not_care_result = IsColorSpaceArrayDoNotCare(*constraints.color_spaces());
  if (!is_color_space_do_not_care_result.is_ok()) {
    LogError(FROM_HERE, "malformed color_spaces re. DO_NOT_CARE");
    return false;
  }
  bool is_color_space_do_not_care = is_color_space_do_not_care_result.value();

  if (!is_format_do_not_care && !is_color_space_do_not_care) {
    for (uint32_t i = 0; i < constraints.color_spaces()->size(); ++i) {
      if (!ImageFormatIsSupportedColorSpaceForPixelFormat(constraints.color_spaces()->at(i),
                                                          pixel_format_and_modifier)) {
        auto color_space = constraints.color_spaces()->at(i);
        LogError(FROM_HERE,
                 "!ImageFormatIsSupportedColorSpaceForPixelFormat() "
                 "color_space: %u pixel_format: %u pixel_format_modifier: 0x%" PRIx64,
                 sysmem::fidl_underlying_cast(color_space),
                 sysmem::fidl_underlying_cast(*constraints.pixel_format()),
                 fidl::ToUnderlying(*constraints.pixel_format_modifier()));
        return false;
      }
    }
  }

  if (constraints.required_min_size()->width() == 0) {
    LogError(FROM_HERE, "required_min_size.width == 0");
    return false;
  }
  ZX_DEBUG_ASSERT(constraints.required_min_size()->width() != 0);
  if (constraints.required_min_size()->width() < constraints.min_size()->width()) {
    LogError(FROM_HERE, "required_min_size.width < min_size.width");
    return false;
  }
  if (constraints.required_max_size()->width() > constraints.max_size()->width()) {
    LogError(FROM_HERE, "required_max_size.width > max_size.width");
    return false;
  }
  for (auto& required_max_size : *constraints.required_max_size_list()) {
    if (required_max_size.width() > constraints.max_size()->width()) {
      LogError(FROM_HERE, "required_max_size_list[n].width > max_size.width");
      return false;
    }
  }
  if (constraints.required_min_size()->height() == 0) {
    LogError(FROM_HERE, "required_min_size.height == 0");
    return false;
  }
  ZX_DEBUG_ASSERT(constraints.required_min_size()->height() != 0);
  if (constraints.required_min_size()->height() < constraints.min_size()->height()) {
    LogError(FROM_HERE, "required_min_size.height < min_size.height");
    return false;
  }
  if (constraints.required_max_size()->height() > constraints.max_size()->height()) {
    LogError(FROM_HERE, "required_max_size.height > max_size.height");
    return false;
  }
  for (auto& required_max_size : *constraints.required_max_size_list()) {
    if (required_max_size.height() > constraints.max_size()->height()) {
      LogError(FROM_HERE, "required_max_size_list[n].height > max_size.height");
      return false;
    }
  }

  if (!IsNonZeroPowerOf2(constraints.pad_for_block_size()->width())) {
    LogError(FROM_HERE, "pad_for_block_size.width must be power of 2 - not %u",
             constraints.pad_for_block_size()->width());
    return false;
  }
  if (!IsNonZeroPowerOf2(constraints.pad_for_block_size()->height())) {
    LogError(FROM_HERE, "pad_for_block_size.height must be power of 2 - not %u",
             constraints.pad_for_block_size()->height());
    return false;
  }
  if (constraints.pad_for_block_size()->width() > 1 ||
      constraints.pad_for_block_size()->width() > 1) {
    if (*constraints.pixel_format() == fuchsia_images2::PixelFormat::kDoNotCare) {
      LogError(FROM_HERE,
               "pad_for_block_size not currently supported with PixelFormat::kDoNotCare");
      return false;
    }
    if (*constraints.pixel_format_modifier() == fuchsia_images2::PixelFormatModifier::kDoNotCare) {
      LogError(FROM_HERE,
               "pad_for_block_size not currently supported with PixelFormatModifier::kDoNotCare");
      return false;
    }
    if (*constraints.pixel_format_modifier() != fuchsia_images2::PixelFormatModifier::kLinear) {
      LogError(FROM_HERE,
               "pad_for_block_size currently only supported for PixelFormatModifier.Linear");
      return false;
    }
    if (!ImageFormatIsNonTiledSinglePlane(PixelFormatAndModifierFromConstraints(constraints))) {
      LogError(FROM_HERE,
               "pad_for_block_size currently requires ImageFormatIsNonTiledSinglePlane()");
      return false;
    }
    if (constraints.pad_for_block_size()->height() > 1) {
      if (constraints.max_size()->width() == std::numeric_limits<uint32_t>::max()) {
        // Please set something sane and reasonable. Something like 2x-4x of the expected actual
        // expected usage max can be a sane and reasonable value in a lot of cases. Usage of blocks
        // can incur space overhead up to a factor of pad_for_block_size.height without this, so
        // this is why the participant using blocks must also set a max width.
        //
        // Setting max_size.width to 0xFFFFFFFE can be justified in rare cases, such as these:
        //   * If/when there is another participant (possibly one from a set of possible
        //   participants)
        //     that is guaranteed to exist as a participant in all non-test situations, and is
        //     guaranteed to set a lower max width, and the current participant wants to use that
        //     other participant's value for max_size.width instead of a-priori knowing what the min
        //     of the max(es) of all the max_size.width(s) among those participants happens to be.
        //     If a participant sets 0xFFFFFFFE for this reason, please comment near the 0xFFFFFFFE
        //     which other participant or set of participants will be guaranteed to set a lower
        //     value. When pad_for_block_size.height > 1, sysmem will complain to the log about an
        //     aggregated max width >= 0x80000000, and will fail (here when stage == kAggregated) if
        //     aggregated max width is 0xFFFFFFFF.
        LogError(
            FROM_HERE,
            "setting pad_for_block_size.height requires also setting max_size.width (2x - 4x of the expected actual usage max is an ok rule of thumb)");
        return false;
      }
      if (stage == CheckSanitizeStage::kAggregated &&
          constraints.max_size()->width() >= 0x80000000) {
        // See comment above and the error text here for why sysmem complains in this case.
        //
        // Setting max_size.width to 0x7FFFFFFF can be justified in rare cases, such as these:
        //   * The participant knows that the result BufferMemorySettings.size_bytes will be quite
        //     small and that an overhead of pad_for_block_size.height minus 1 extra rows won't be
        //     a problem. Please comment the 0x7FFFFFFF in this case.
        LogWarn(
            FROM_HERE,
            "setting pad_for_block_size.height requires also setting max_size.width, which it is, but max_size.width is still (very) excessive; high space overhead may occur");
        return false;
      }
    }
  }

  return true;
}

bool LogicalBufferCollection::AccumulateConstraintsBufferUsage(fuchsia_sysmem2::BufferUsage* acc,
                                                               fuchsia_sysmem2::BufferUsage c) {
  // We accumulate "none" usage just like other usages, to make aggregation and CheckSanitize
  // consistent/uniform.
  *acc->none() |= *c.none();
  *acc->cpu() |= *c.cpu();
  *acc->vulkan() |= *c.vulkan();
  *acc->display() |= *c.display();
  *acc->video() |= *c.video();
  return true;
}

// |acc| accumulated constraints so far
//
// |c| additional constraint to aggregate into acc
bool LogicalBufferCollection::AccumulateConstraintBufferCollection(
    fuchsia_sysmem2::BufferCollectionConstraints* acc,
    fuchsia_sysmem2::BufferCollectionConstraints c) {
  if (!AccumulateConstraintsBufferUsage(&acc->usage().value(), std::move(c.usage().value()))) {
    return false;
  }

  acc->min_buffer_count_for_camping().value() += c.min_buffer_count_for_camping().value();
  acc->min_buffer_count_for_dedicated_slack().value() +=
      c.min_buffer_count_for_dedicated_slack().value();
  acc->min_buffer_count_for_shared_slack().emplace(
      std::max(acc->min_buffer_count_for_shared_slack().value(),
               c.min_buffer_count_for_shared_slack().value()));
  acc->min_buffer_count().emplace(
      std::max(acc->min_buffer_count().value(), c.min_buffer_count().value()));

  // 0 is replaced with 0xFFFFFFFF in
  // CheckSanitizeBufferCollectionConstraints.
  ZX_DEBUG_ASSERT(acc->max_buffer_count().value() != 0);
  ZX_DEBUG_ASSERT(c.max_buffer_count().value() != 0);
  acc->max_buffer_count().emplace(
      std::min(acc->max_buffer_count().value(), c.max_buffer_count().value()));

  // CheckSanitizeBufferCollectionConstraints() takes care of setting a default
  // buffer_collection_constraints, so we can assert that both acc and c "has_" one.
  ZX_DEBUG_ASSERT(acc->buffer_memory_constraints().has_value());
  ZX_DEBUG_ASSERT(c.buffer_memory_constraints().has_value());
  if (!AccumulateConstraintBufferMemory(&acc->buffer_memory_constraints().value(),
                                        std::move(c.buffer_memory_constraints().value()))) {
    return false;
  }

  if (acc->image_format_constraints()->empty()) {
    // Take the whole VectorView<>, as the count() can only go down later, so the capacity of
    // c.image_format_constraints() is fine.
    acc->image_format_constraints().emplace(std::move(c.image_format_constraints().value()));
  } else {
    ZX_DEBUG_ASSERT(!acc->image_format_constraints()->empty());
    if (!c.image_format_constraints()->empty()) {
      if (!AccumulateConstraintImageFormats(&acc->image_format_constraints().value(),
                                            std::move(c.image_format_constraints().value()))) {
        // We return false if we've seen non-zero
        // image_format_constraint_count from at least one participant
        // but among non-zero image_format_constraint_count participants
        // since then the overlap has dropped to empty set.
        //
        // This path is taken when there are completely non-overlapping
        // PixelFormats and also when PixelFormat(s) overlap but none
        // of those have any non-empty settings space remaining.  In
        // that case we've removed the PixelFormat from consideration
        // despite it being common among participants (so far).
        return false;
      }
      ZX_DEBUG_ASSERT(!acc->image_format_constraints()->empty());
    }
  }

  // acc->image_format_constraints().count() == 0 is allowed here, when all
  // participants had image_format_constraints().count() == 0.
  return true;
}

bool LogicalBufferCollection::AccumulateConstraintPermittedHeaps(
    std::vector<fuchsia_sysmem2::Heap>* acc, std::vector<fuchsia_sysmem2::Heap> c) {
  // Remove any heap in acc that's not in c.  If zero heaps
  // remain in acc, return false.
  ZX_DEBUG_ASSERT(acc->size() > 0);

  for (uint32_t ai = 0; ai < acc->size(); ++ai) {
    uint32_t ci;
    for (ci = 0; ci < c.size(); ++ci) {
      if ((*acc)[ai] == c[ci]) {
        // We found heap in c.  Break so we can move on to
        // the next heap.
        break;
      }
    }
    if (ci == c.size()) {
      // Remove from acc because not found in c.
      //
      // Move formerly last item on top of the item being removed, if not the same item.
      if (ai != acc->size() - 1) {
        (*acc)[ai] = std::move((*acc)[acc->size() - 1]);
      }
      // remove last item
      acc->resize(acc->size() - 1);
      // adjust ai to force current index to be processed again as it's
      // now a different item
      --ai;
    }
  }

  if (acc->empty()) {
    LogError(FROM_HERE, "Zero permitted heap overlap");
    return false;
  }

  return true;
}

bool LogicalBufferCollection::AccumulateConstraintBufferMemory(
    fuchsia_sysmem2::BufferMemoryConstraints* acc, fuchsia_sysmem2::BufferMemoryConstraints c) {
  acc->min_size_bytes() = std::max(*acc->min_size_bytes(), *c.min_size_bytes());

  // Don't permit 0 as the overall min_size_bytes; that would be nonsense.  No
  // particular initiator should feel that it has to specify 1 in this field;
  // that's just built into sysmem instead.  While a VMO will have a minimum
  // actual size of page size, we do permit treating buffers as if they're 1
  // byte, mainly for testing reasons, and to avoid any unnecessary dependence
  // or assumptions re. page size.
  acc->min_size_bytes() = std::max(*acc->min_size_bytes(), 1ul);
  acc->max_size_bytes() = std::min(*acc->max_size_bytes(), *c.max_size_bytes());

  acc->physically_contiguous_required() =
      *acc->physically_contiguous_required() || *c.physically_contiguous_required();
  if (*acc->physically_contiguous_required()) {
    uint64_t acc_alignment =
        acc->min_physical_base_alignment().has_value() ? *acc->min_physical_base_alignment() : 1u;
    uint64_t c_alignment =
        c.min_physical_base_alignment().has_value() ? *c.min_physical_base_alignment() : 1u;
    acc->min_physical_base_alignment() = std::max(acc_alignment, c_alignment);
  }

  acc->secure_required() = *acc->secure_required() || *c.secure_required();

  acc->ram_domain_supported() = *acc->ram_domain_supported() && *c.ram_domain_supported();
  acc->cpu_domain_supported() = *acc->cpu_domain_supported() && *c.cpu_domain_supported();
  acc->inaccessible_domain_supported() =
      *acc->inaccessible_domain_supported() && *c.inaccessible_domain_supported();

  if (acc->permitted_heaps()->empty()) {
    acc->permitted_heaps().emplace(std::move(*c.permitted_heaps()));
  } else {
    if (!c.permitted_heaps()->empty()) {
      if (!AccumulateConstraintPermittedHeaps(&*acc->permitted_heaps(),
                                              std::move(*c.permitted_heaps()))) {
        return false;
      }
    }
  }
  return true;
}

bool LogicalBufferCollection::AccumulateConstraintImageFormats(
    std::vector<fuchsia_sysmem2::ImageFormatConstraints>* acc,
    std::vector<fuchsia_sysmem2::ImageFormatConstraints> c) {
  // Remove any pixel_format in acc that's not in c.  Process any format
  // that's in both.  If processing the format results in empty set for that
  // format, pretend as if the format wasn't in c and remove that format from
  // acc.  If acc ends up with zero formats, return false.

  // This method doesn't get called unless there's at least one format in
  // acc.
  ZX_DEBUG_ASSERT(!acc->empty());

  // Fan out any entries with DoNotCare in either field of PixelFormatAndModifier, so that the loop
  // below can separately aggregate exactly-matching PixelFormatAndModifier(s).
  //
  // After this, accumulation can proceed as normal, with kDoNotCare (if still present) treated as
  // any other normal PixelFormatType.  At the end of overall accumulation, we filter out any
  // remaining DoNotCare entries (elsewhere).
  if (!ReplicateAnyPixelFormatAndModifierDoNotCares(c, *acc) ||
      !ReplicateAnyPixelFormatAndModifierDoNotCares(*acc, c)) {
    LogError(FROM_HERE, "ReplicateAnyPixelFormatAndModifierDoNotCares failed");
    return false;
  }

  for (size_t ai = 0; ai < acc->size(); ++ai) {
    bool is_found_in_c = false;
    for (size_t ci = 0; ci < c.size(); ++ci) {
      auto acc_pixel_format_and_modifier = PixelFormatAndModifierFromConstraints((*acc)[ai]);
      auto c_pixel_format_and_modifier = PixelFormatAndModifierFromConstraints(c[ci]);
      if (ImageFormatIsPixelFormatEqual(acc_pixel_format_and_modifier,
                                        c_pixel_format_and_modifier)) {
        // Move last entry into the entry we're consuming, since LLCPP FIDL tables don't have any
        // way to detect that they've been moved out of, so we need to keep c tightly packed with
        // not-moved-out-of entries.  We don't need to adjust ci to stay at the same entry for the
        // next iteration of the loop because by this point we know we're done scanning c in this
        // iteration of the ai loop.
        fuchsia_sysmem2::ImageFormatConstraints old_c_ci = std::move(c[ci]);
        if (ci != c.size() - 1) {
          c[ci] = std::move(c[c.size() - 1]);
        }
        c.resize(c.size() - 1);
        if (!AccumulateConstraintImageFormat(&(*acc)[ai], std::move(old_c_ci))) {
          // Pretend like the format wasn't in c to begin with, so
          // this format gets removed from acc.  Only if this results
          // in zero formats in acc do we end up returning false.
          ZX_DEBUG_ASSERT(!is_found_in_c);
          break;
        }
        // We found the format in c and processed the format without
        // that resulting in empty set; break so we can move on to the
        // next format.
        is_found_in_c = true;
        break;
      }
    }
    if (!is_found_in_c) {
      // Remove from acc because not found in c.
      //
      // Move last item on top of the item being removed, if not the same item.
      if (ai != acc->size() - 1) {
        (*acc)[ai] = std::move((*acc)[acc->size() - 1]);
      } else {
        // Stuff under this item would get deleted later anyway, but delete now to avoid keeping
        // cruft we don't need.
        (*acc)[ai] = fuchsia_sysmem2::ImageFormatConstraints();
      }
      // remove last item
      acc->resize(acc->size() - 1);
      // adjust ai to force current index to be processed again as it's
      // now a different item
      --ai;
    }
  }

  if (acc->empty()) {
    // It's ok for this check to be under Accumulate* because it's permitted
    // for a given participant to have zero image_format_constraints_count.
    // It's only when the count becomes non-zero then drops back to zero
    // (checked here), or if we end up with no image format constraints and
    // no buffer constraints (checked in ::Allocate()), that we care.
    LogWarn(FROM_HERE, "all pixel_format(s) eliminated");
    return false;
  }

  return true;
}

bool LogicalBufferCollection::AccumulateConstraintImageFormat(
    fuchsia_sysmem2::ImageFormatConstraints* acc, fuchsia_sysmem2::ImageFormatConstraints c) {
  ZX_DEBUG_ASSERT(ImageFormatIsPixelFormatEqual(PixelFormatAndModifierFromConstraints(*acc),
                                                PixelFormatAndModifierFromConstraints(c)));
  // Checked previously.
  ZX_DEBUG_ASSERT(acc->color_spaces().has_value());
  ZX_DEBUG_ASSERT(!acc->color_spaces()->empty());
  // Checked previously.
  ZX_DEBUG_ASSERT(c.color_spaces().has_value());
  ZX_DEBUG_ASSERT(!c.color_spaces()->empty());

  if (!AccumulateConstraintColorSpaces(&*acc->color_spaces(), std::move(*c.color_spaces()))) {
    return false;
  }
  // Else AccumulateConstraintColorSpaces() would have returned false.
  ZX_DEBUG_ASSERT(!acc->color_spaces()->empty());

  ZX_DEBUG_ASSERT(acc->min_size().has_value());
  ZX_DEBUG_ASSERT(c.min_size().has_value());
  acc->min_size()->width() = std::max(acc->min_size()->width(), c.min_size()->width());
  acc->min_size()->height() = std::max(acc->min_size()->height(), c.min_size()->height());

  ZX_DEBUG_ASSERT(acc->max_size().has_value());
  ZX_DEBUG_ASSERT(c.max_size().has_value());
  acc->max_size()->width() = std::min(acc->max_size()->width(), c.max_size()->width());
  acc->max_size()->height() = std::min(acc->max_size()->height(), c.max_size()->height());

  acc->min_bytes_per_row() = std::max(*acc->min_bytes_per_row(), *c.min_bytes_per_row());
  acc->max_bytes_per_row() = std::min(*acc->max_bytes_per_row(), *c.max_bytes_per_row());

  ZX_DEBUG_ASSERT(acc->max_width_times_height().has_value());
  ZX_DEBUG_ASSERT(c.max_width_times_height().has_value());
  acc->max_width_times_height() =
      std::min(*acc->max_width_times_height(), *c.max_width_times_height());

  // For these, see also the conditional statement below that ensures these are fixed up with any
  // pixel-format-dependent adjustment.
  ZX_DEBUG_ASSERT(acc->size_alignment().has_value());
  ZX_DEBUG_ASSERT(c.size_alignment().has_value());
  acc->size_alignment()->width() =
      std::max(acc->size_alignment()->width(), c.size_alignment()->width());
  acc->size_alignment()->height() =
      std::max(acc->size_alignment()->height(), c.size_alignment()->height());

  auto combine_bytes_per_row_divisor_result =
      CombineBytesPerRowDivisor(*acc->bytes_per_row_divisor(), *c.bytes_per_row_divisor());
  if (!combine_bytes_per_row_divisor_result.is_ok()) {
    LogError(FROM_HERE, "!combine_bytes_per_row_divisor_result.is_ok()");
    return false;
  }
  acc->bytes_per_row_divisor() = *combine_bytes_per_row_divisor_result;
  acc->require_bytes_per_row_at_pixel_boundary() =
      *acc->require_bytes_per_row_at_pixel_boundary() ||
      *c.require_bytes_per_row_at_pixel_boundary();

  ZX_DEBUG_ASSERT(acc->start_offset_divisor().has_value());
  ZX_DEBUG_ASSERT(c.start_offset_divisor().has_value());
  acc->start_offset_divisor() = std::max(*acc->start_offset_divisor(), *c.start_offset_divisor());

  ZX_DEBUG_ASSERT(acc->display_rect_alignment().has_value());
  ZX_DEBUG_ASSERT(c.display_rect_alignment().has_value());
  acc->display_rect_alignment()->width() =
      std::max(acc->display_rect_alignment()->width(), c.display_rect_alignment()->width());
  acc->display_rect_alignment()->height() =
      std::max(acc->display_rect_alignment()->height(), c.display_rect_alignment()->height());

  // The required_ space is accumulated by taking the union, and must be fully
  // within the non-required_ space, else fail.  For example, this allows a
  // video decoder to indicate that it's capable of outputting a wide range of
  // output dimensions, but that it has specific current dimensions that are
  // presently required_ (min == max) for decode to proceed.
  ZX_DEBUG_ASSERT(acc->required_min_size().has_value());
  ZX_DEBUG_ASSERT(c.required_min_size().has_value());
  ZX_DEBUG_ASSERT(acc->required_max_size().has_value());
  ZX_DEBUG_ASSERT(c.required_max_size().has_value());

  // list has_value() but may be empty list
  ZX_DEBUG_ASSERT(acc->required_max_size_list().has_value());
  ZX_DEBUG_ASSERT(c.required_max_size_list().has_value());

  ZX_DEBUG_ASSERT(acc->required_min_size()->width() != 0);
  ZX_DEBUG_ASSERT(c.required_min_size()->width() != 0);
  ZX_DEBUG_ASSERT(acc->required_min_size()->height() != 0);
  ZX_DEBUG_ASSERT(c.required_min_size()->height() != 0);

  acc->required_min_size()->width() =
      std::min(acc->required_min_size()->width(), c.required_min_size()->width());
  acc->required_max_size()->width() =
      std::max(acc->required_max_size()->width(), c.required_max_size()->width());
  acc->required_min_size()->height() =
      std::min(acc->required_min_size()->height(), c.required_min_size()->height());
  acc->required_max_size()->height() =
      std::max(acc->required_max_size()->height(), c.required_max_size()->height());

  // We don't copy the aggregated required_max_size to an entry in required_max_size_list until
  // after the rest of Accumulate* is done. For now they remain separate.
  std::copy(c.required_max_size_list()->begin(), c.required_max_size_list()->end(),
            std::back_inserter(*acc->required_max_size_list()));
  // we don't clear *c.required_max_size_list() here; c will get deleted fairly soon

  if (c.is_alpha_present().has_value()) {
    if (!acc->is_alpha_present().has_value()) {
      acc->is_alpha_present() = *c.is_alpha_present();
    } else {
      // The FIDL wire format specification says bool is only 0 or 1, so we rely on FIDL server-side
      // code to enforce that.
      ZX_DEBUG_ASSERT_MSG(!!*acc->is_alpha_present() == *acc->is_alpha_present(),
                          " - FIDL server-side code needs to enforce bool is 0 or 1");
      ZX_DEBUG_ASSERT_MSG(!!*c.is_alpha_present() == *c.is_alpha_present(),
                          " - FIDL server-side code needs to enforce bool is 0 or 1");
      if (*acc->is_alpha_present() != *c.is_alpha_present()) {
        LogInfo(FROM_HERE, "*acc->is_alpha_present() != *c.is_alpha_present()");
        return false;
      }
    }
  }

  // {1, 1} is the default; already applied as-needed
  ZX_DEBUG_ASSERT(acc->pad_for_block_size().has_value());
  ZX_DEBUG_ASSERT(c.pad_for_block_size().has_value());
  // already enforced
  ZX_DEBUG_ASSERT(IsNonZeroPowerOf2(acc->pad_for_block_size()->width()));
  ZX_DEBUG_ASSERT(IsNonZeroPowerOf2(acc->pad_for_block_size()->height()));
  ZX_DEBUG_ASSERT(IsNonZeroPowerOf2(c.pad_for_block_size()->width()));
  ZX_DEBUG_ASSERT(IsNonZeroPowerOf2(c.pad_for_block_size()->height()));
  acc->pad_for_block_size()->width() =
      std::max(acc->pad_for_block_size()->width(), c.pad_for_block_size()->width());
  acc->pad_for_block_size()->height() =
      std::max(acc->pad_for_block_size()->height(), c.pad_for_block_size()->height());
  if (acc->pad_for_block_size()->width() > 1) {
    auto pixel_format_and_modifier =
        PixelFormatAndModifier(*acc->pixel_format(), *acc->pixel_format_modifier());
    uint32_t stride_bytes_per_width_pixel =
        ImageFormatStrideBytesPerWidthPixel(pixel_format_and_modifier);
    uint32_t block_width_in_bytes =
        stride_bytes_per_width_pixel * acc->pad_for_block_size()->width();
    // This is to avoid a block-using producer writing into valid pixels at the start of the next
    // row. In other words, each "pixel" in each block needs its own memory location. We apply this
    // constraint regardless of whether the block-using participant is writing or only reading. In
    // theory if the participant is only reading we could avoid this, but that would require more
    // complexity which seems not worth it to optimize that sub-case of a sub-case which we also
    // haven't encountered yet.
    //
    // The pad_for_block_size mechanism depends on this for being able to compute the needed padding
    // using ImageFormatImageSize as a tool.
    auto combine_bytes_per_row_divisor_result =
        CombineBytesPerRowDivisor(*acc->bytes_per_row_divisor(), block_width_in_bytes);
    if (!combine_bytes_per_row_divisor_result.is_ok()) {
      LogError(FROM_HERE,
               "!combine_bytes_per_row_divisor_result.is_ok() (for pad_for_block_size.width)");
      return false;
    }
    *acc->bytes_per_row_divisor() = *combine_bytes_per_row_divisor_result;
  }

  *acc->pad_beyond_image_size_bytes() =
      std::max(*acc->pad_beyond_image_size_bytes(), *c.pad_beyond_image_size_bytes());

  return true;
}

bool LogicalBufferCollection::AccumulateConstraintColorSpaces(
    std::vector<fuchsia_images2::ColorSpace>* acc, std::vector<fuchsia_images2::ColorSpace> c) {
  // Any ColorSpace kDoNotCare can only happen with count() == 1, checked previously.  If both acc
  // and c are indicating kDoNotCare, the result still needs to be kDoNotCare.  If only one of acc
  // or c is indicating kDoNotCare, we need to fan out the kDoNotCare into each color space
  // indicated by the other (of acc and c).  After this, accumulation can proceed as normal, with
  // kDoNotCare (if still present) treated as any other normal ColorSpaceType.  At the end of
  // overall accumulation, we must check (elsewhere) that we're not left with only a single
  // kDoNotCare ColorSpaceType.
  auto acc_is_do_not_care_result = IsColorSpaceArrayDoNotCare(*acc);
  // maintained during accumulation, largely thanks to having checked each c previously
  ZX_DEBUG_ASSERT(acc_is_do_not_care_result.is_ok());
  bool acc_is_do_not_care = acc_is_do_not_care_result.value();
  auto c_is_do_not_care_result = IsColorSpaceArrayDoNotCare(c);
  // checked previously
  ZX_DEBUG_ASSERT(c_is_do_not_care_result.is_ok());
  bool c_is_do_not_care = c_is_do_not_care_result.value();
  if (acc_is_do_not_care && !c_is_do_not_care) {
    // Replicate acc entries to correspond to c entries
    ReplicateColorSpaceDoNotCare(c, *acc);
  } else if (!acc_is_do_not_care && c_is_do_not_care) {
    // replicate c entries to corresponding acc entries
    ReplicateColorSpaceDoNotCare(*acc, c);
  } else {
    // Either both are ColorSpaceType kDoNotCare, or neither are.
    ZX_DEBUG_ASSERT(acc_is_do_not_care == c_is_do_not_care);
  }

  // Remove any color_space in acc that's not in c.  If zero color spaces
  // remain in acc, return false.

  for (uint32_t ai = 0; ai < acc->size(); ++ai) {
    uint32_t ci;
    for (ci = 0; ci < c.size(); ++ci) {
      if (IsColorSpaceEqual((*acc)[ai], c[ci])) {
        // We found the color space in c.  Break so we can move on to
        // the next color space.
        break;
      }
    }
    if (ci == c.size()) {
      // Remove from acc because not found in c.
      //
      // Move formerly last item on top of the item being removed, if not same item.
      if (ai != acc->size() - 1) {
        (*acc)[ai] = std::move((*acc)[acc->size() - 1]);
      } else {
        // Stuff under this item would get deleted later anyway, but delete now to avoid keeping
        // cruft we don't need.
        (*acc)[ai] = fuchsia_images2::ColorSpace();
      }
      // remove last item
      acc->resize(acc->size() - 1);
      // adjust ai to force current index to be processed again as it's
      // now a different item
      --ai;
    }
  }

  if (acc->empty()) {
    // It's ok for this check to be under Accumulate* because it's also
    // under CheckSanitize().  It's fine to provide a slightly more helpful
    // error message here and early out here.
    LogError(FROM_HERE, "Zero color_space overlap");
    return false;
  }

  return true;
}

// TODO: remove in a later CL.
bool LogicalBufferCollection::IsColorSpaceEqual(const fuchsia_images2::ColorSpace& a,
                                                const fuchsia_images2::ColorSpace& b) {
  return a == b;
}

fpromise::result<fuchsia_sysmem2::Heap, zx_status_t> LogicalBufferCollection::GetHeap(
    const fuchsia_sysmem2::BufferMemoryConstraints& constraints, Sysmem* sysmem) {
  if (*constraints.secure_required()) {
    // If two different secure heaps are mutually supported among all participants of a collection
    // (rare outside of tests), we pick the one with alphabetically lower bind string. As needed, we
    // could add a way to override the relative selection priority of secure heaps via sysmem config
    // (either in PlatformSysmemConfig -> structured config, or in fuchsia.sysmem2.Config).
    ZX_DEBUG_ASSERT(IsSecurePermitted(constraints));
    std::optional<fuchsia_sysmem2::Heap> best_heap;
    parent_sysmem_->ForeachSecureHeap([&constraints, &best_heap](
                                          const fuchsia_sysmem2::Heap& secure_heap) -> bool {
      if (!IsPermittedHeap(constraints, secure_heap)) {
        // keep going
        return true;
      }
      if (!best_heap.has_value() || secure_heap.heap_type()->compare(*best_heap->heap_type()) < 0) {
        best_heap = secure_heap;
      }
      // keep going
      return true;
    });
    // We already checked previosuly that IsSecurePermitted(), so we'll have a "best" heap here.
    ZX_DEBUG_ASSERT(best_heap.has_value());
    return fpromise::ok(std::move(*best_heap));
  }
  const static auto kSystemRamHeapSingleton =
      sysmem::MakeHeap(bind_fuchsia_sysmem_heap::HEAP_TYPE_SYSTEM_RAM, 0);
  if (IsPermittedHeap(constraints, kSystemRamHeapSingleton)) {
    return fpromise::ok(kSystemRamHeapSingleton);
  }

  for (size_t i = 0; i < constraints.permitted_heaps()->size(); ++i) {
    auto& heap = constraints.permitted_heaps()->at(i);
    const auto* heap_properties = sysmem->GetHeapProperties(heap);
    if (!heap_properties) {
      continue;
    }
    if (heap_properties->coherency_domain_support().has_value() &&
        ((*heap_properties->coherency_domain_support()->cpu_supported() &&
          *constraints.cpu_domain_supported()) ||
         (*heap_properties->coherency_domain_support()->ram_supported() &&
          *constraints.ram_domain_supported()) ||
         (*heap_properties->coherency_domain_support()->inaccessible_supported() &&
          *constraints.inaccessible_domain_supported()))) {
      return fpromise::ok(heap);
    }
  }
  return fpromise::error(ZX_ERR_NOT_FOUND);
}

static fpromise::result<fuchsia_sysmem2::CoherencyDomain> GetCoherencyDomain(
    const fuchsia_sysmem2::BufferCollectionConstraints& constraints,
    MemoryAllocator* memory_allocator) {
  ZX_DEBUG_ASSERT(constraints.buffer_memory_constraints().has_value());

  using fuchsia_sysmem2::CoherencyDomain;
  const auto& heap_properties = memory_allocator->heap_properties();
  ZX_DEBUG_ASSERT(heap_properties.coherency_domain_support().has_value());

  // Display prefers RAM coherency domain for now.
  if (*constraints.usage()->display() != 0) {
    if (*constraints.buffer_memory_constraints()->ram_domain_supported()) {
      // Display controllers generally aren't cache coherent, so prefer
      // RAM coherency domain.
      //
      // TODO - base on the system in use.
      return fpromise::ok(fuchsia_sysmem2::CoherencyDomain::kRam);
    }
  }

  if (*heap_properties.coherency_domain_support()->cpu_supported() &&
      *constraints.buffer_memory_constraints()->cpu_domain_supported()) {
    return fpromise::ok(CoherencyDomain::kCpu);
  }

  if (*heap_properties.coherency_domain_support()->ram_supported() &&
      *constraints.buffer_memory_constraints()->ram_domain_supported()) {
    return fpromise::ok(CoherencyDomain::kRam);
  }

  if (*heap_properties.coherency_domain_support()->inaccessible_supported() &&
      *constraints.buffer_memory_constraints()->inaccessible_domain_supported()) {
    // Intentionally permit treating as Inaccessible if we reach here, even
    // if the heap permits CPU access.  Only domain in common among
    // participants is Inaccessible.
    return fpromise::ok(fuchsia_sysmem2::CoherencyDomain::kInaccessible);
  }

  return fpromise::error();
}

fpromise::result<fuchsia_sysmem2::BufferCollectionInfo, zx_status_t>
LogicalBufferCollection::GenerateUnpopulatedBufferCollectionInfo(
    const fuchsia_sysmem2::BufferCollectionConstraints& constraints) {
  TRACE_DURATION("gfx", "LogicalBufferCollection:GenerateUnpopulatedBufferCollectionInfo", "this",
                 this);

  fuchsia_sysmem2::BufferCollectionInfo result;

  result.buffer_collection_id() = buffer_collection_id_;

  uint32_t min_buffer_count = *constraints.min_buffer_count_for_camping() +
                              *constraints.min_buffer_count_for_dedicated_slack() +
                              *constraints.min_buffer_count_for_shared_slack();
  min_buffer_count = std::max(min_buffer_count, *constraints.min_buffer_count());
  uint32_t max_buffer_count = *constraints.max_buffer_count();
  if (min_buffer_count > max_buffer_count) {
    LogError(FROM_HERE,
             "aggregate min_buffer_count > aggregate max_buffer_count - "
             "min: %u max: %u",
             min_buffer_count, max_buffer_count);
    return fpromise::error(ZX_ERR_NOT_SUPPORTED);
  }
  if (min_buffer_count > fuchsia_sysmem::kMaxCountBufferCollectionInfoBuffers) {
    LogError(FROM_HERE,
             "aggregate min_buffer_count (%d) > MAX_COUNT_BUFFER_COLLECTION_INFO_BUFFERS (%d)",
             min_buffer_count, fuchsia_sysmem::kMaxCountBufferCollectionInfoBuffers);
    return fpromise::error(ZX_ERR_NOT_SUPPORTED);
  }
  if (min_buffer_count == 0) {
    // Client(s) must request at least 1 buffer.
    LogError(FROM_HERE, "aggregate min_buffer_count == 0");
    return fpromise::error(ZX_ERR_NOT_SUPPORTED);
  }

  result.buffers().emplace(min_buffer_count);
  result.buffers()->resize(min_buffer_count);
  ZX_DEBUG_ASSERT(result.buffers()->size() == min_buffer_count);
  ZX_DEBUG_ASSERT(result.buffers()->size() <= max_buffer_count);

  uint64_t min_size_bytes = 0;
  uint64_t max_size_bytes = std::numeric_limits<uint64_t>::max();

  result.settings().emplace();
  fuchsia_sysmem2::SingleBufferSettings& settings = *result.settings();
  settings.buffer_settings().emplace();
  fuchsia_sysmem2::BufferMemorySettings& buffer_settings = *settings.buffer_settings();

  ZX_DEBUG_ASSERT(constraints.buffer_memory_constraints().has_value());
  const fuchsia_sysmem2::BufferMemoryConstraints& buffer_constraints =
      *constraints.buffer_memory_constraints();
  buffer_settings.is_physically_contiguous() = *buffer_constraints.physically_contiguous_required();
  // checked previously
  ZX_DEBUG_ASSERT(!*buffer_constraints.secure_required() || IsSecurePermitted(buffer_constraints));
  buffer_settings.is_secure() = *buffer_constraints.secure_required();

  auto result_get_heap = GetHeap(buffer_constraints, parent_sysmem_);
  if (!result_get_heap.is_ok()) {
    LogError(FROM_HERE, "Can not find a heap permitted by buffer constraints, error %d",
             result_get_heap.error());
    return fpromise::error(result_get_heap.error());
  }
  buffer_settings.heap() = result_get_heap.value();

  // We can't fill out buffer_settings yet because that also depends on
  // ImageFormatConstraints.  We do need the min and max from here though.
  min_size_bytes = *buffer_constraints.min_size_bytes();
  max_size_bytes = *buffer_constraints.max_size_bytes();

  // Get memory allocator for settings.
  MemoryAllocator* allocator = parent_sysmem_->GetAllocator(buffer_settings);
  if (!allocator) {
    LogError(FROM_HERE, "No memory allocator for buffer settings");
    return fpromise::error(ZX_ERR_NO_MEMORY);
  }

  auto coherency_domain_result = GetCoherencyDomain(constraints, allocator);
  if (!coherency_domain_result.is_ok()) {
    LogError(FROM_HERE, "No coherency domain found for buffer constraints");
    return fpromise::error(ZX_ERR_NOT_SUPPORTED);
  }
  buffer_settings.coherency_domain() = coherency_domain_result.value();

  // It's allowed for zero participants to have any ImageFormatConstraint(s),
  // in which case the combined constraints_ will have zero (and that's fine,
  // when allocating raw buffers that don't need any ImageFormatConstraint).
  //
  // At least for now, we pick which PixelFormat to use before determining if
  // the constraints associated with that PixelFormat imply a buffer size
  // range in min_size_bytes..max_size_bytes.
  if (!constraints.image_format_constraints()->empty()) {
    // Pick the best ImageFormatConstraints.
    uint32_t best_index = UINT32_MAX;
    bool found_unsupported_when_protected = false;
    for (uint32_t i = 0; i < constraints.image_format_constraints()->size(); ++i) {
      auto pixel_format_and_modifier =
          PixelFormatAndModifierFromConstraints(constraints.image_format_constraints()->at(i));
      if (*buffer_settings.is_secure() &&
          !ImageFormatCompatibleWithProtectedMemory(pixel_format_and_modifier)) {
        found_unsupported_when_protected = true;
        continue;
      }
      if (best_index == UINT32_MAX) {
        best_index = i;
      } else {
        if (CompareImageFormatConstraintsByIndex(constraints, i, best_index) < 0) {
          best_index = i;
        }
      }
    }
    if (best_index == UINT32_MAX) {
      ZX_DEBUG_ASSERT(found_unsupported_when_protected);
      LogError(FROM_HERE, "No formats were compatible with protected memory.");
      return fpromise::error(ZX_ERR_NOT_SUPPORTED);
    }
    // copy / clone from constraints to settings.
    settings.image_format_constraints() = constraints.image_format_constraints()->at(best_index);
    if (!HasAlphaOrX(settings.image_format_constraints()->pixel_format().value())) {
      // We want is_alpha_present to be un-set in the allocation result when the format has no A/X
      // channel.
      settings.image_format_constraints()->is_alpha_present().reset();
    }
  }

  // Compute the min buffer size implied by image_format_constraints, so we ensure the buffers can
  // hold the min-size image.
  if (settings.image_format_constraints().has_value()) {
    const fuchsia_sysmem2::ImageFormatConstraints& image_format_constraints =
        *settings.image_format_constraints();
    auto pixel_format_and_modifier =
        PixelFormatAndModifierFromConstraints(image_format_constraints);
    fuchsia_images2::ImageFormat min_image;
    min_image.pixel_format() = pixel_format_and_modifier.pixel_format;
    min_image.pixel_format_modifier() = pixel_format_and_modifier.pixel_format_modifier;

    min_image.size().emplace();

    // We need at least one entry in required_max_size_list to account for size_alignment etc in the
    // loop below, so if we have zero entries, use a local list with just {0, 0} in it.
    const std::vector<fuchsia_math::SizeU>* local_required_max_size_list = nullptr;
    std::optional<std::vector<fuchsia_math::SizeU>> local_required_max_size_list_storage;
    if (image_format_constraints.required_max_size_list()->empty()) {
      local_required_max_size_list_storage.emplace(std::vector{fuchsia_math::SizeU{0, 0}});
      local_required_max_size_list = &*local_required_max_size_list_storage;
    } else {
      local_required_max_size_list = &*image_format_constraints.required_max_size_list();
    }

    for (auto& required_max_size : *local_required_max_size_list) {
      // We use required_max_size.width because that's the max width that the producer (or
      // initiator) wants these buffers to be able to hold.
      min_image.size()->width() = fbl::round_up(
          std::max(image_format_constraints.min_size()->width(), required_max_size.width()),
          image_format_constraints.size_alignment()->width());
      if (min_image.size()->width() > image_format_constraints.max_size()->width()) {
        LogError(FROM_HERE, "size_alignment.width caused size.width > max_size.width");
        return fpromise::error(ZX_ERR_NOT_SUPPORTED);
      }
      // We use required_max_size.height because that's the max height that the producer (or
      // initiator) needs these buffers to be able to hold.
      min_image.size()->height() = fbl::round_up(
          std::max(image_format_constraints.min_size()->height(), required_max_size.height()),
          image_format_constraints.size_alignment()->height());
      if (min_image.size()->height() > image_format_constraints.max_size()->height()) {
        LogError(FROM_HERE, "size_alignment.height caused size.height > max_size.height");
        return fpromise::error(ZX_ERR_NOT_SUPPORTED);
      }

      uint32_t one_row_non_padding_bytes;
      if (!CheckMul(ImageFormatStrideBytesPerWidthPixel(pixel_format_and_modifier),
                    min_image.size()->width())
               .AssignIfValid(&one_row_non_padding_bytes)) {
        LogError(FROM_HERE, "stride_bytes_per_width_pixel * size.width failed");
        return fpromise::error(ZX_ERR_NOT_SUPPORTED);
      }
      // TODO: Make/use a safemath-y version of fbl::round_up().
      min_image.bytes_per_row() = fbl::round_up(
          std::max(one_row_non_padding_bytes, *image_format_constraints.min_bytes_per_row()),
          *image_format_constraints.bytes_per_row_divisor());
      if (*min_image.bytes_per_row() > *image_format_constraints.max_bytes_per_row()) {
        LogError(FROM_HERE,
                 "bytes_per_row_divisor caused bytes_per_row > "
                 "max_bytes_per_row");
        return fpromise::error(ZX_ERR_NOT_SUPPORTED);
      }
      uint32_t min_image_width_times_height;
      if (!CheckMul(min_image.size()->width(), min_image.size()->height())
               .AssignIfValid(&min_image_width_times_height)) {
        LogError(FROM_HERE, "min_image width*height failed");
        return fpromise::error(ZX_ERR_NOT_SUPPORTED);
      }
      if (min_image_width_times_height > *image_format_constraints.max_width_times_height()) {
        LogError(FROM_HERE, "min_image_width_times_height > max_width_times_height");
        return fpromise::error(ZX_ERR_NOT_SUPPORTED);
      }

      // The display size and valid size don't matter for computing size in bytes.
      ZX_DEBUG_ASSERT(!min_image.display_rect().has_value());
      ZX_DEBUG_ASSERT(!min_image.valid_size().has_value());

      // Checked previously.
      ZX_DEBUG_ASSERT(image_format_constraints.color_spaces()->size() >= 1);
      // This doesn't matter for computing size in bytes, as we trust the pixel_format to fully
      // specify the image size.  But set it to the first ColorSpace anyway, just so the
      // color_space.type is a valid value.
      min_image.color_space() = image_format_constraints.color_spaces()->at(0);

      uint64_t image_min_size_bytes = ImageFormatImageSize(min_image);

      if (image_min_size_bytes > min_size_bytes) {
        if (image_min_size_bytes > max_size_bytes) {
          LogError(FROM_HERE, "image_min_size_bytes > max_size_bytes");
          return fpromise::error(ZX_ERR_NOT_SUPPORTED);
        }
        min_size_bytes = image_min_size_bytes;
        ZX_DEBUG_ASSERT(min_size_bytes <= max_size_bytes);
      }
    }
  }

  // Currently redundant with earlier checks, but just in case... This check must occur before pad_*
  // checks below because padding isn't allowed to be the only thing causing buffers to be non-zero
  // size.
  if (min_size_bytes == 0) {
    LogError(FROM_HERE, "min_size_bytes == 0 (without padding from pad_* fields)");
    return fpromise::error(ZX_ERR_NOT_SUPPORTED);
  }
  ZX_DEBUG_ASSERT(min_size_bytes != 0);

  // Now that min_size_bytes accounts for any ImageFormatConstraints (other than pad_* constraints),
  // we can set buffer_settings.size_bytes(). Producers are required to ensure that
  // ImageFormatImageSize() is <= buffer_settings.size_bytes. The ImageFormatImageSize()
  // intentionally does not include extra size from pad_* constraints, so the size_bytes field also
  // intentionally does not include extra size from pad_* constraints.
  //
  // If an initiator (or a participant) wants to force buffers to be larger than the size implied by
  // minimum image dimensions, the initiator can use BufferMemorySettings.min_size_bytes to force
  // allocated buffers to be large enough. Or if the requirement is to ensure padding beyond valid
  // image data, the participant can use pad_* constraint fields to request padding which is applied
  // after this below to ensure that producers don't eat into the padding with their valid image
  // data.
  //
  // Don't modify buffer_settings.size_bytes() again below here.
  buffer_settings.size_bytes() = safe_cast<uint32_t>(min_size_bytes);

  // These pad_* checks happen after min_size_bytes == 0 check above because padding is not allowed
  // to be the only reason that buffers would be non-zero size, and after buffer_settings.size_bytes
  // is set since pad_* constraints add bytes that by definition (of "padding") don't include any
  // valid image data.
  //
  // The pad_* constraints ensure that any image in the space of allowed images given aggregated
  // constraints will have sufficient padding. These pad_* constraints are not allowed to imply a
  // narrower maximum achievable width than the aggregate constraints and buffer_settings.size_bytes
  // set above indicate so far.
  if (settings.image_format_constraints().has_value()) {
    const auto& image_format_constraints = *settings.image_format_constraints();

    ZX_DEBUG_ASSERT(buffer_settings.size_bytes().has_value());
    ZX_DEBUG_ASSERT(settings.image_format_constraints().has_value());
    auto& pad_for_block_size = *settings.image_format_constraints()->pad_for_block_size();
    uint64_t size_bytes_plus_pad_for_block_size_bytes;
    if (pad_for_block_size.width() > 1 || pad_for_block_size.height() > 1) {
      auto complain = [this](Location from_there, fuchsia_logging::LogSeverity severity,
                             const char* message) { LogError(from_there, "%s", message); };
      auto size_bytes_plus_pad_for_block_size_bytes_result = PaddedSizeFromBlockSize(
          image_format_constraints, *buffer_settings.size_bytes(), complain);
      if (!size_bytes_plus_pad_for_block_size_bytes_result.is_ok()) {
        LogError(FROM_HERE, "PaddedSizeFromBlockSize failed - params:");
        auto indent_tracker = IndentTracker(1);
        auto indent = indent_tracker.Current();
        LogError(FROM_HERE, "%*spad_for_block_size: {%u, %u}", indent.num_spaces(), "",
                 pad_for_block_size.width(), pad_for_block_size.height());
        LogError(FROM_HERE, "%*sBufferMemorySettings.size_bytes: %zu", indent.num_spaces(), "",
                 *buffer_settings.size_bytes());
        LogError(FROM_HERE, "%*simage_format_constraints:", indent.num_spaces(), "");
        LogImageFormatConstraints(indent_tracker, nullptr, image_format_constraints);
        return fpromise::error(ZX_ERR_NOT_SUPPORTED);
      }
      size_bytes_plus_pad_for_block_size_bytes =
          size_bytes_plus_pad_for_block_size_bytes_result.value();
    } else {
      size_bytes_plus_pad_for_block_size_bytes = *buffer_settings.size_bytes();
    }
    if (size_bytes_plus_pad_for_block_size_bytes > min_size_bytes) {
      if (size_bytes_plus_pad_for_block_size_bytes > max_size_bytes) {
        LogError(
            FROM_HERE,
            "BufferMemorySettings.size_bytes + pad_for_block_size_bytes > BufferMemoryConstraints.max_size_bytes");
        return fpromise::error(ZX_ERR_NOT_SUPPORTED);
      }
      min_size_bytes = size_bytes_plus_pad_for_block_size_bytes;
    }

    // The worst case for pad_beyond_image_size_bytes is simply to add those bytes to
    // BufferMemorySettings.size_bytes, because by definition no ImageFormatImageSize is allowed to
    // exceed BufferMemorySettings.size_bytes.
    uint64_t size_bytes_plus_padding_beyond_image_bytes =
        *buffer_settings.size_bytes() + *image_format_constraints.pad_beyond_image_size_bytes();
    if (size_bytes_plus_padding_beyond_image_bytes > min_size_bytes) {
      if (size_bytes_plus_padding_beyond_image_bytes > max_size_bytes) {
        LogError(
            FROM_HERE,
            "BufferMemorySettings.size_bytes + pad_beyond_image_size_bytes > BufferMemoryConstraints.max_size_bytes");
        return fpromise::error(ZX_ERR_NOT_SUPPORTED);
      }
      min_size_bytes = size_bytes_plus_padding_beyond_image_bytes;
    }
  }

  buffer_settings.raw_vmo_size() = fbl::round_up(min_size_bytes, zx_system_get_page_size());
  if (*buffer_settings.raw_vmo_size() < min_size_bytes) {
    LogError(FROM_HERE, "raw_vmo_size overflows when rounding to multiple of page_size");
    return fpromise::error(ZX_ERR_NO_MEMORY);
  }

  // For purposes of enforcing max_size_bytes, we intentionally don't care that a VMO can only be a
  // multiple of page size.

  uint64_t total_size_bytes = min_size_bytes * result.buffers()->size();
  if (total_size_bytes > kMaxTotalSizeBytesPerCollection) {
    LogError(FROM_HERE, "total_size_bytes > kMaxTotalSizeBytesPerCollection");
    return fpromise::error(ZX_ERR_NO_MEMORY);
  }

  if (min_size_bytes > kMaxSizeBytesPerBuffer) {
    LogError(FROM_HERE, "min_size_bytes > kMaxSizeBytesPerBuffer");
    return fpromise::error(ZX_ERR_NO_MEMORY);
  }
  ZX_DEBUG_ASSERT(min_size_bytes <= std::numeric_limits<uint32_t>::max());

  if (*buffer_settings.size_bytes() > parent_sysmem_->settings().max_allocation_size) {
    // This is different than max_size_bytes.  While max_size_bytes is part of the constraints,
    // max_allocation_size isn't part of the constraints.  The latter is used for simulating OOM or
    // preventing unpredictable memory pressure caused by a fuzzer or similar source of
    // unpredictability in tests.
    LogError(FROM_HERE,
             "GenerateUnpopulatedBufferCollectionInfo() failed because size %" PRIu64
             " > "
             "max_allocation_size %ld",
             *buffer_settings.size_bytes(), parent_sysmem_->settings().max_allocation_size);
    return fpromise::error(ZX_ERR_NO_MEMORY);
  }

  for (uint32_t i = 0; i < result.buffers()->size(); ++i) {
    fuchsia_sysmem2::VmoBuffer vmo_buffer;
    vmo_buffer.vmo_usable_start() = 0ul;
    result.buffers()->at(i) = std::move(vmo_buffer);
  }

  return fpromise::ok(std::move(result));
}

fpromise::result<fuchsia_sysmem2::BufferCollectionInfo, Error> LogicalBufferCollection::Allocate(
    const fuchsia_sysmem2::BufferCollectionConstraints& constraints,
    fuchsia_sysmem2::BufferCollectionInfo* builder) {
  TRACE_DURATION("gfx", "LogicalBufferCollection:Allocate", "this", this);

  fuchsia_sysmem2::BufferCollectionInfo& result = *builder;

  fuchsia_sysmem2::SingleBufferSettings& settings = *result.settings();
  fuchsia_sysmem2::BufferMemorySettings& buffer_settings = *settings.buffer_settings();

  // Get memory allocator for settings.
  MemoryAllocator* allocator = parent_sysmem_->GetAllocator(buffer_settings);
  if (!allocator) {
    LogError(FROM_HERE, "No memory allocator for buffer settings");
    return fpromise::error(Error::kNoMemory);
  }
  memory_allocator_ = allocator;

  // Register failure handler with memory allocator.
  allocator->AddDestroyCallback(reinterpret_cast<intptr_t>(this), [this]() {
    LogAndFailRootNode(FROM_HERE, Error::kUnspecified,
                       "LogicalBufferCollection memory allocator gone - now auto-failing self.");
  });

  if (settings.image_format_constraints().has_value()) {
    const fuchsia_sysmem2::ImageFormatConstraints& image_format_constraints =
        *settings.image_format_constraints();
    inspect_node_.CreateUint("pixel_format",
                             sysmem::fidl_underlying_cast(*image_format_constraints.pixel_format()),
                             &vmo_properties_);
    if (image_format_constraints.pixel_format_modifier().has_value()) {
      inspect_node_.CreateUint(
          "pixel_format_modifier",
          fidl::ToUnderlying(*image_format_constraints.pixel_format_modifier()), &vmo_properties_);
    }
    if (image_format_constraints.min_size()->width() > 0) {
      inspect_node_.CreateUint("min_size_width", image_format_constraints.min_size()->width(),
                               &vmo_properties_);
    }
    if (image_format_constraints.min_size()->height() > 0) {
      inspect_node_.CreateUint("min_size_height", image_format_constraints.min_size()->height(),
                               &vmo_properties_);
    }
    if (image_format_constraints.required_max_size()->width() > 0) {
      inspect_node_.CreateUint("deprecated_required_max_size_width",
                               image_format_constraints.required_max_size()->width(),
                               &vmo_properties_);
    }
    if (image_format_constraints.required_max_size()->height() > 0) {
      inspect_node_.CreateUint("deprecated_required_max_size_height",
                               image_format_constraints.required_max_size()->height(),
                               &vmo_properties_);
    }
    // the list ordering isn't significant here; it just helps if they don't all look the same
    size_t index = 0;
    for (auto& required_max_size : *image_format_constraints.required_max_size_list()) {
      ZX_DEBUG_ASSERT(required_max_size.width() > 0);
      ZX_DEBUG_ASSERT(required_max_size.height() > 0);
      inspect_node_.CreateUint(std::format("required_max_size_list[{}].width", index),
                               required_max_size.width(), &vmo_properties_);
      inspect_node_.CreateUint(std::format("required_max_size_list[{}].height", index),
                               required_max_size.height(), &vmo_properties_);
      ++index;
    }
  }

  inspect_node_.CreateUint("allocator_id", allocator->id(), &vmo_properties_);
  inspect_node_.CreateUint("size_bytes", *buffer_settings.size_bytes(), &vmo_properties_);
  inspect_node_.CreateString("heap_type", *buffer_settings.heap()->heap_type(), &vmo_properties_);
  inspect_node_.CreateUint("heap_id", *buffer_settings.heap()->id(), &vmo_properties_);
  inspect_node_.CreateUint("allocation_timestamp_ns", zx::clock::get_monotonic().get(),
                           &vmo_properties_);

  ZX_DEBUG_ASSERT(*buffer_settings.size_bytes() <= parent_sysmem_->settings().max_allocation_size);

  auto cleanup_buffers = fit::defer([this] { ClearBuffers(); });

  for (uint32_t i = 0; i < result.buffers()->size(); ++i) {
    auto allocate_result =
        AllocateVmo(allocator, settings,
                    constraints.buffer_memory_constraints()->min_physical_base_alignment(), i);
    if (!allocate_result.is_ok()) {
      LogError(FROM_HERE, "AllocateVmo() failed");
      return fpromise::error(Error::kNoMemory);
    }
    zx::vmo vmo = allocate_result.take_value();
    auto& vmo_buffer = result.buffers()->at(i);
    vmo_buffer.vmo() = std::move(vmo);
    ZX_DEBUG_ASSERT(vmo_buffer.vmo_usable_start().has_value());
    ZX_DEBUG_ASSERT(*vmo_buffer.vmo_usable_start() == 0);
  }
  vmo_count_property_ = inspect_node_.CreateUint("vmo_count", result.buffers()->size());
  // Make sure we have sufficient barrier after allocating/clearing/flushing any VMO newly allocated
  // by allocator above.
  BarrierAfterFlush();

  cleanup_buffers.cancel();
  return fpromise::ok(std::move(result));
}

fpromise::result<zx::vmo> LogicalBufferCollection::AllocateVmo(
    MemoryAllocator* allocator, const fuchsia_sysmem2::SingleBufferSettings& settings,
    std::optional<uint64_t> min_physical_alignment, uint32_t index) {
  TRACE_DURATION("gfx", "LogicalBufferCollection::AllocateVmo", "size_bytes",
                 *settings.buffer_settings()->size_bytes());

  // Physical VMOs only support slices where the size (and offset) are page_size aligned, so we
  // should also round up when allocating.
  size_t rounded_size_bytes = *settings.buffer_settings()->raw_vmo_size();
  ZX_DEBUG_ASSERT((rounded_size_bytes % zx_system_get_page_size()) == 0);

  if (min_physical_alignment.has_value()) {
    // This avoids any degenerate cases in MemoryAllocator(s). All sysmem allocators are
    // page-aligned per granularity of read/write permissions for mappings and so on.
    min_physical_alignment =
        std::max(*min_physical_alignment, static_cast<uint64_t>(zx_system_get_page_size()));
    // must be power of 2
    if ((*min_physical_alignment & (*min_physical_alignment - 1)) != 0) {
      // should be impossible, but if this somehow happens, don't abort() sysmem
      LogError(FROM_HERE, "*min_physical_alignment somehow not a power of 2");
      return fpromise::error();
    }
  }

  // raw_parent_vmo may itself be a child VMO of an allocator's overall contig VMO, but that's an
  // internal detail of the allocator.  The ZERO_CHILDREN signal will only be set when all direct
  // _and indirect_ child VMOs are fully gone (not just handles closed, but the kernel object is
  // deleted, which avoids races with handle close, and means there also aren't any mappings left).
  zx::vmo raw_parent_vmo;
  std::optional<std::string> name;
  if (name_.has_value()) {
    name = fbl::StringPrintf("%s:%d", name_->name.c_str(), index).c_str();
  }
  zx_status_t status = allocator->Allocate(rounded_size_bytes, min_physical_alignment, settings,
                                           name, buffer_collection_id_, index, &raw_parent_vmo);
  if (status != ZX_OK) {
    LogError(FROM_HERE,
             "allocator.Allocate failed - size_bytes: %zu "
             "status: %d",
             rounded_size_bytes, status);
    return fpromise::error();
  }

  // LogicalBuffer::Create() takes ownership of raw_parent_vmo in both error and success cases,
  // including the call to allocator->Delete() at the appropriate time. If the LogicalBuffer is
  // deleted prior to ZX_VMO_ZERO_CHILDREN, the waits will be cancelled and the allocator->Delete()
  // will be sync rather than async. This is mainly relevant if LogicalBufferCollection::Allocate()
  // fails to allocate a later buffer in the same collection.
  auto buffer_result = LogicalBuffer::Create(fbl::RefPtr(this), index, std::move(raw_parent_vmo));
  if (buffer_result.is_error()) {
    LogError(FROM_HERE, "LogicalBuffer::Create() failed - status: %d", buffer_result.error_value());
    return fpromise::error();
  }
  if (!buffer_result->is_ok()) {
    LogError(FROM_HERE, "LogicalBuffer::error(): %d", buffer_result->error());
    return fpromise::error();
  }

  // success from here down

  zx::vmo strong_child_vmo = buffer_result->TakeStrongChildVmo();
  if (name_.has_value()) {
    TRACE_DURATION("gfx", "strong_child_vmo.set_property", "buffer_index", index);
    status = strong_child_vmo.set_property(ZX_PROP_NAME, name->c_str(), name->size());
    if (status != ZX_OK) {
      LogInfo(FROM_HERE, "strong_child_vmo.set_property(name) failed (ignoring): %d", status);
      // intentionally ignore set_property() failure
    }
  }

  auto emplace_result = buffers_.try_emplace(index, std::move(buffer_result).value());
  ZX_ASSERT(emplace_result.second);

  return fpromise::ok(std::move(strong_child_vmo));
}

void LogicalBufferCollection::CreationTimedOut(async_dispatcher_t* dispatcher,
                                               async::TaskBase* task, zx_status_t status) {
  if (status != ZX_OK) {
    return;
  }

  // It's possible for the timer to fire after the root_ has been deleted, but before "this" has
  // been deleted (which also cancels the timer if it's still pending).  The timer doesn't need to
  // take any action in this case.
  if (!root_) {
    return;
  }

  ZX_DEBUG_ASSERT(creation_timer_run_count_ <= 1);
  ++creation_timer_run_count_;
  ZX_DEBUG_ASSERT(creation_timer_run_count_ <= 2);

  std::string name = name_.has_value() ? name_->name : "Unknown";
  zx::duration duration_so_far = zx::clock::get_monotonic() - create_time_monotonic_;

  LogError(
      FROM_HERE,
      "Allocation of %s is taking a while (creation_timer_run_count_: %u, monotonic duration so far: %" PRId64
      " ms). Waiting for (connected) tokens: ",
      name.c_str(), creation_timer_run_count_, duration_so_far.to_msecs());
  ZX_DEBUG_ASSERT(root_);
  // Current token Node(s), not logical token nodes that are presently OrphanedNode, as those can't
  // be blocking allocation.
  auto token_nodes = root_->BreadthFirstOrder([](const NodeProperties& node) {
    ZX_DEBUG_ASSERT(node.node());
    bool potentially_included_in_initial_allocation =
        IsPotentiallyIncludedInInitialAllocation(node);
    return NodeFilterResult{.keep_node = potentially_included_in_initial_allocation &&
                                         !!node.node()->buffer_collection_token(),
                            .iterate_children = potentially_included_in_initial_allocation};
  });
  for (auto node_properties : token_nodes) {
    if (!node_properties->client_debug_info().name.empty()) {
      LogError(FROM_HERE, "Name %s id %ld", node_properties->client_debug_info().name.c_str(),
               node_properties->client_debug_info().id);
    } else {
      LogError(FROM_HERE, "Unknown token");
    }
  }

  // Current collection Node(s), not logical collection nodes that are presently OrphanedNode, as
  // those can't be blocking allocation.
  LogError(FROM_HERE, "Connected collections:");
  auto collection_nodes = root_->BreadthFirstOrder([](const NodeProperties& node) {
    ZX_DEBUG_ASSERT(node.node());
    bool potentially_included_in_initial_allocation =
        IsPotentiallyIncludedInInitialAllocation(node);
    return NodeFilterResult{.keep_node = potentially_included_in_initial_allocation &&
                                         !!node.node()->buffer_collection(),
                            .iterate_children = potentially_included_in_initial_allocation};
  });
  for (auto node_properties : collection_nodes) {
    const char* constraints_state =
        node_properties->node()->buffer_collection()->has_constraints() ? "set" : "unset";
    if (!node_properties->client_debug_info().name.empty()) {
      LogError(FROM_HERE, "Name \"%s\" id %ld (constraints %s)",
               node_properties->client_debug_info().name.c_str(),
               node_properties->client_debug_info().id, constraints_state);
    } else {
      LogError(FROM_HERE, "Name unknown (constraints %s)", constraints_state);
    }
  }

  // Current group Node(s), not logical group nodes that are presently OrphanedNode, as those can't
  // be blocking allocation.
  LogError(FROM_HERE, "Connected groups:");
  auto group_nodes = root_->BreadthFirstOrder([](const NodeProperties& node) {
    ZX_DEBUG_ASSERT(node.node());
    bool potentially_included_in_initial_allocation =
        IsPotentiallyIncludedInInitialAllocation(node);
    return NodeFilterResult{.keep_node = potentially_included_in_initial_allocation &&
                                         !!node.node()->buffer_collection_token_group(),
                            .iterate_children = potentially_included_in_initial_allocation};
  });
  for (auto node_properties : group_nodes) {
    const char* children_created_state_string =
        node_properties->node()->ReadyForAllocation() ? "" : " NOT";
    if (!node_properties->client_debug_info().name.empty()) {
      LogError(FROM_HERE, "Name \"%s\" id %ld (children%s done creating)",
               node_properties->client_debug_info().name.c_str(),
               node_properties->client_debug_info().id, children_created_state_string);
    } else {
      LogError(FROM_HERE, "Name unknown (children%s done creating)", children_created_state_string);
    }
  }

  ZX_DEBUG_ASSERT(creation_timer_run_count_ <= 2);
  if (creation_timer_run_count_ >= 2) {
    // we intentionally don't continue complaining after two complaints to the log (by default at
    // approximately create + 5s and create + 35s)
    return;
  }

  // remember to complain one more time later if allocation still hasn't completed by then
  status = creation_timer_.PostForTime(parent_sysmem_->loop_dispatcher(),
                                       zx::deadline_after(zx::sec(30)));
  ZX_ASSERT(status == ZX_OK);
}

static int32_t clamp_difference(uint64_t a, uint64_t b) {
  if (a > b) {
    return 1;
  }
  if (a < b) {
    return -1;
  }
  return 0;
}

// 1 means a > b, 0 means ==, -1 means a < b.
//
// TODO(dustingreen): Pay attention to constraints_->usage, by checking any
// overrides that prefer particular PixelFormat based on a usage / usage
// combination.
int32_t LogicalBufferCollection::CompareImageFormatConstraintsTieBreaker(
    const fuchsia_sysmem2::ImageFormatConstraints& a,
    const fuchsia_sysmem2::ImageFormatConstraints& b) {
  // If there's not any cost difference, fall back to choosing the
  // pixel_format that has the larger type enum value as a tie-breaker.

  int32_t result = clamp_difference(sysmem::fidl_underlying_cast(*a.pixel_format()),
                                    sysmem::fidl_underlying_cast(*b.pixel_format()));

  if (result != 0) {
    return result;
  }

  result = clamp_difference(a.pixel_format_modifier().has_value(),
                            b.pixel_format_modifier().has_value());

  if (result != 0) {
    return result;
  }

  if (a.pixel_format_modifier().has_value() && b.pixel_format_modifier().has_value()) {
    result = clamp_difference(fidl::ToUnderlying(*a.pixel_format_modifier()),
                              fidl::ToUnderlying(*b.pixel_format_modifier()));
  }

  return result;
}

int32_t LogicalBufferCollection::CompareImageFormatConstraintsByIndex(
    const fuchsia_sysmem2::BufferCollectionConstraints& constraints, uint32_t index_a,
    uint32_t index_b) {
  int32_t cost_compare = usage_pixel_format_cost().Compare(constraints, index_a, index_b);
  if (cost_compare != 0) {
    return cost_compare;
  }

  // If we get this far, there's no known reason to choose one PixelFormat
  // over another, so just pick one based on a tie-breaker that'll distinguish
  // between PixelFormat(s).

  int32_t tie_breaker_compare =
      CompareImageFormatConstraintsTieBreaker(constraints.image_format_constraints()->at(index_a),
                                              constraints.image_format_constraints()->at(index_b));
  return tie_breaker_compare;
}

TrackedParentVmo::TrackedParentVmo(fbl::RefPtr<LogicalBufferCollection> buffer_collection,
                                   zx::vmo vmo, uint32_t buffer_index,
                                   TrackedParentVmo::DoDelete do_delete)
    : buffer_collection_(std::move(buffer_collection)),
      vmo_(std::move(vmo)),
      buffer_index_(buffer_index),
      do_delete_(std::move(do_delete)),
      zero_children_wait_(this, vmo_.get(), ZX_VMO_ZERO_CHILDREN) {
  ZX_DEBUG_ASSERT(buffer_collection_);
  ZX_DEBUG_ASSERT(vmo_);
  ZX_DEBUG_ASSERT(do_delete_);
}

TrackedParentVmo::~TrackedParentVmo() {
  // In some error paths, we just delete.
  CancelWait();

  if (do_delete_) {
    do_delete_(this);
  }
}

zx_status_t TrackedParentVmo::StartWait(async_dispatcher_t* dispatcher) {
  buffer_collection_->LogInfo(FROM_HERE, "LogicalBufferCollection::TrackedParentVmo::StartWait()");
  // The current thread is the dispatcher thread.
  ZX_DEBUG_ASSERT(!waiting_);
  zx_status_t status = zero_children_wait_.Begin(dispatcher);
  if (status != ZX_OK) {
    buffer_collection_->LogError(FROM_HERE, nullptr,
                                 "zero_children_wait_.Begin() failed - status: %d", status);
    return status;
  }
  waiting_ = true;
  return ZX_OK;
}

zx_status_t TrackedParentVmo::CancelWait() {
  waiting_ = false;
  return zero_children_wait_.Cancel();
}

zx::vmo TrackedParentVmo::TakeVmo() {
  ZX_DEBUG_ASSERT(!waiting_);
  ZX_DEBUG_ASSERT(vmo_);
  return std::move(vmo_);
}

const zx::vmo& TrackedParentVmo::vmo() const {
  ZX_DEBUG_ASSERT(vmo_);
  return vmo_;
}

fit::result<zx_status_t, zx::eventpair> LogicalBuffer::DupCloseWeakAsapClientEnd() {
  if (!close_weak_asap_created_) {
    decltype(close_weak_asap_) local;
    local.emplace();
    zx_status_t create_status = zx::eventpair::create(0, &local->client_end, &local->server_end);
    if (create_status != ZX_OK) {
      return fit::error(create_status);
    }
    close_weak_asap_ = std::move(local);
    ZX_DEBUG_ASSERT(close_weak_asap_->client_end.is_valid());
    ZX_DEBUG_ASSERT(close_weak_asap_->server_end.is_valid());
    // Later when ~strong_parent_vmo_, ~close_weak_asap_ will signal clients via
    // ZX_EVENTPAIR_PEER_CLOSED.
    close_weak_asap_created_ = true;
    // If the last strong VMO has already been deleted, we need to close server_end here.
    if (!strong_parent_vmo_) {
      close_weak_asap_->server_end.reset();
    }
  }
  ZX_DEBUG_ASSERT(close_weak_asap_created_);
  // The client_end is kept even after server_end is reset(), so we can still send the client_end to
  // the client, without creating a new kernel object for every GetVmoInfo() request.
  ZX_DEBUG_ASSERT(close_weak_asap_->client_end.is_valid());
  zx::eventpair dup_client_end;
  zx_status_t dup_status =
      close_weak_asap_->client_end.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup_client_end);
  if (dup_status != ZX_OK) {
    return fit::error(dup_status);
  }
  return fit::ok(std::move(dup_client_end));
}

void TrackedParentVmo::OnZeroChildren(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                                      zx_status_t status, const zx_packet_signal_t* signal) {
  TRACE_DURATION("gfx", "TrackedParentVmo::OnZeroChildren", "buffer_collection",
                 buffer_collection_.get(), "child_koid",
                 child_koid_.has_value() ? *child_koid_ : ZX_KOID_INVALID);
  buffer_collection_->LogInfo(FROM_HERE, "TrackedParentVmo::OnZeroChildren()");
  ZX_DEBUG_ASSERT(waiting_);
  waiting_ = false;
  if (status == ZX_ERR_CANCELED) {
    // The collection canceled all of these waits as part of destruction, do nothing.
    return;
  }
  ZX_DEBUG_ASSERT(status == ZX_OK);
  ZX_DEBUG_ASSERT(signal->trigger & ZX_VMO_ZERO_CHILDREN);
  ZX_DEBUG_ASSERT(do_delete_);
  TrackedParentVmo::DoDelete local_do_delete = std::move(do_delete_);
  ZX_DEBUG_ASSERT(!do_delete_);
  // will delete "this"
  local_do_delete(this);
  ZX_DEBUG_ASSERT(!local_do_delete);
}

void LogicalBufferCollection::AddCountsForNode(const Node& node) {
  AdjustRelevantNodeCounts(node, [](uint32_t& count) { ++count; });
}

void LogicalBufferCollection::RemoveCountsForNode(const Node& node) {
  AdjustRelevantNodeCounts(node, [](uint32_t& count) { --count; });
}

void LogicalBufferCollection::AdjustRelevantNodeCounts(
    const Node& node, fit::function<void(uint32_t& count)> visitor) {
  for (NodeProperties* iter = &node.node_properties(); iter; iter = iter->parent()) {
    visitor(iter->node_count_);
    if (node.is_connected_type()) {
      visitor(iter->connected_client_count_);
    }
    if (node.buffer_collection()) {
      visitor(iter->buffer_collection_count_);
    }
    if (node.buffer_collection_token()) {
      visitor(iter->buffer_collection_token_count_);
    }
  }
}

// Only for use by NodeProperties.
void LogicalBufferCollection::DeleteRoot() { root_.reset(); }

NodeProperties* LogicalBufferCollection::FindTreeToFail(NodeProperties* failing_node) {
  ZX_DEBUG_ASSERT(failing_node);
  for (NodeProperties* iter = failing_node; iter; iter = iter->parent()) {
    if (!iter->parent()) {
      LogClientInfo(FROM_HERE, iter, "The root should fail.");
      return iter;
    }
    NodeProperties* parent = iter->parent();
    ErrorPropagationMode mode = iter->error_propagation_mode();
    switch (mode) {
      case ErrorPropagationMode::kPropagate:
        // keep propagating the failure upward
        LogClientInfo(FROM_HERE, iter, "Propagate node failure to parent because kPropagate");
        continue;
      case ErrorPropagationMode::kPropagateBeforeAllocation:
        // Propagate failure if before allocation.  We also know in this case that parent and child
        // will allocate together.
        ZX_DEBUG_ASSERT(parent->buffers_logically_allocated() ==
                        iter->buffers_logically_allocated());
        if (!iter->buffers_logically_allocated()) {
          LogClientInfo(FROM_HERE, iter,
                        "Propagate node failure to parent because kPropagateBeforeAllocation and "
                        "!BuffersLogicallyAllocated");
          continue;
        } else {
          LogClientInfo(FROM_HERE, iter,
                        "Do not propagate node failure to parent because "
                        "kPropagateBeforeAllocation and BuffersLogicallyAllocated");
          return iter;
        }
      case ErrorPropagationMode::kDoNotPropagate:
        LogClientInfo(FROM_HERE, iter,
                      "Do not propagate node failure to parent because kDoNotPropagate");
        return iter;
      default:
        ZX_PANIC("unreachable 1");
        return nullptr;
    }
  }
  ZX_PANIC("unreachable 2");
  return nullptr;
}

// For tests.
std::vector<const BufferCollection*> LogicalBufferCollection::collection_views() const {
  std::vector<const BufferCollection*> result;
  if (!root_) {
    return result;
  }
  auto nodes = root_->BreadthFirstOrder();
  for (auto node_properties : nodes) {
    if (node_properties->node()->buffer_collection()) {
      result.push_back(node_properties->node()->buffer_collection());
    }
  }
  return result;
}

void LogicalBufferCollection::TrackNodeProperties(NodeProperties* node_properties) {
  node_properties_by_node_ref_keep_koid_.insert(
      std::make_pair(node_properties->node_ref_koid(), node_properties));
}

void LogicalBufferCollection::UntrackNodeProperties(NodeProperties* node_properties) {
  node_properties_by_node_ref_keep_koid_.erase(node_properties->node_ref_koid());
}

std::optional<NodeProperties*> LogicalBufferCollection::FindNodePropertiesByNodeRefKoid(
    zx_koid_t node_ref_keep_koid) {
  auto iter = node_properties_by_node_ref_keep_koid_.find(node_ref_keep_koid);
  if (iter == node_properties_by_node_ref_keep_koid_.end()) {
    return std::nullopt;
  }
  return iter->second;
}

#define LOG_UINT32_FIELD(location, indent, prefix, field_name)                                   \
  do {                                                                                           \
    if ((prefix).field_name().has_value()) {                                                     \
      LogClientInfo(location, node_properties, "%*s*%s.%s(): %u", indent.num_spaces(), "",       \
                    #prefix, #field_name, sysmem::fidl_underlying_cast(*(prefix).field_name())); \
    }                                                                                            \
  } while (0)

#define LOG_UINT32_BITS_FIELD(location, indent, prefix, field_name)                              \
  do {                                                                                           \
    if ((prefix).field_name().has_value()) {                                                     \
      LogClientInfo(location, node_properties, "%*s*%s.%s(): 0x%x", indent.num_spaces(), "",     \
                    #prefix, #field_name, sysmem::fidl_underlying_cast(*(prefix).field_name())); \
    }                                                                                            \
  } while (0)

#define LOG_UINT64_FIELD(location, indent, prefix, field_name)                                     \
  do {                                                                                             \
    if ((prefix).field_name().has_value()) {                                                       \
      LogClientInfo(location, node_properties, "%*s*%s.%s(): 0x%" PRIx64, indent.num_spaces(), "", \
                    #prefix, #field_name, sysmem::fidl_underlying_cast(*(prefix).field_name()));   \
    }                                                                                              \
  } while (0)

#define LOG_BOOL_FIELD(location, indent, prefix, field_name)                               \
  do {                                                                                     \
    if ((prefix).field_name().has_value()) {                                               \
      LogClientInfo(location, node_properties, "%*s*%s.%s(): %u", indent.num_spaces(), "", \
                    #prefix, #field_name, *(prefix).field_name());                         \
    }                                                                                      \
  } while (0)

#define LOG_SIZEU_FIELD(location, indent, prefix, field_name)                                      \
  do {                                                                                             \
    if ((prefix).field_name().has_value()) {                                                       \
      LogClientInfo(location, node_properties, "%*s%s.%s()->width(): %u", indent.num_spaces(), "", \
                    #prefix, #field_name, prefix.field_name()->width());                           \
      LogClientInfo(location, node_properties, "%*s%s.%s()->height(): %u", indent.num_spaces(),    \
                    "", #prefix, #field_name, prefix.field_name()->height());                      \
    }                                                                                              \
  } while (0)

void LogicalBufferCollection::LogConstraints(
    Location location, NodeProperties* node_properties,
    const fuchsia_sysmem2::BufferCollectionConstraints& constraints) const {
  ZX_DEBUG_ASSERT(is_verbose_logging());
  IndentTracker indent_tracker(0);
  auto indent = indent_tracker.Current();

  if (!node_properties) {
    LogInfo(FROM_HERE, "%*sConstraints (aggregated / previously chosen):", indent.num_spaces(), "");
  } else {
    LogInfo(FROM_HERE, "%*sConstraints - NodeProperties: %p", indent.num_spaces(), "",
            node_properties);
  }

  {  // scope indent
    auto indent = indent_tracker.Nested();

    const fuchsia_sysmem2::BufferCollectionConstraints& c = constraints;

    if (constraints.usage().has_value()) {
      const auto& usage = *c.usage();
      LOG_UINT32_BITS_FIELD(FROM_HERE, indent, usage, none);
      LOG_UINT32_BITS_FIELD(FROM_HERE, indent, usage, cpu);
      LOG_UINT32_BITS_FIELD(FROM_HERE, indent, usage, vulkan);
      LOG_UINT32_BITS_FIELD(FROM_HERE, indent, usage, display);
      LOG_UINT32_BITS_FIELD(FROM_HERE, indent, usage, video);
    }

    LOG_UINT32_FIELD(FROM_HERE, indent, c, min_buffer_count);
    LOG_UINT32_FIELD(FROM_HERE, indent, c, max_buffer_count);
    LOG_UINT32_FIELD(FROM_HERE, indent, c, min_buffer_count_for_camping);
    LOG_UINT32_FIELD(FROM_HERE, indent, c, min_buffer_count_for_dedicated_slack);
    LOG_UINT32_FIELD(FROM_HERE, indent, c, min_buffer_count_for_shared_slack);

    if (!c.buffer_memory_constraints().has_value()) {
      LogInfo(FROM_HERE, "%*s!c.buffer_memory_constraints().has_value()", indent.num_spaces(), "");
    } else {
      LogInfo(FROM_HERE, "%*sc.buffer_memory_constraints():", indent.num_spaces(), "");
      auto indent = indent_tracker.Nested();
      const fuchsia_sysmem2::BufferMemoryConstraints& bmc = *c.buffer_memory_constraints();
      LOG_UINT64_FIELD(FROM_HERE, indent, bmc, min_size_bytes);
      LOG_UINT64_FIELD(FROM_HERE, indent, bmc, max_size_bytes);
      LOG_BOOL_FIELD(FROM_HERE, indent, bmc, physically_contiguous_required);
      LOG_BOOL_FIELD(FROM_HERE, indent, bmc, secure_required);
      LOG_BOOL_FIELD(FROM_HERE, indent, bmc, cpu_domain_supported);
      LOG_BOOL_FIELD(FROM_HERE, indent, bmc, ram_domain_supported);
      LOG_BOOL_FIELD(FROM_HERE, indent, bmc, inaccessible_domain_supported);

      if (bmc.permitted_heaps().has_value()) {
        LogInfo(FROM_HERE, "%*spermitted_heaps.size() %zu", indent.num_spaces(), "",
                bmc.permitted_heaps()->size());
        // scope indent
        auto indent = indent_tracker.Nested();
        for (size_t i = 0; i < bmc.permitted_heaps()->size(); i++) {
          const auto& heap = bmc.permitted_heaps()->at(i);
          if (heap.heap_type().has_value()) {
            LogInfo(FROM_HERE, "%*spermitted_heaps[%zu].heap_type : %s", indent.num_spaces(), "", i,
                    heap.heap_type()->c_str());
          } else {
            LogInfo(FROM_HERE, "%*s!permitted_heaps[%zu].heap_type.has_value()",
                    indent.num_spaces(), "", i);
          }
          if (heap.id().has_value()) {
            LogInfo(FROM_HERE, "%*spermitted_heaps[%zu].id : 0x%" PRIx64, indent.num_spaces(), "",
                    i, *heap.id());
          } else {
            LogInfo(FROM_HERE, "%*s!permitted_heaps[%zu].id.has_value()", indent.num_spaces(), "",
                    i);
          }
        }
      } else {
        LogInfo(FROM_HERE, "%*s!bmc.permitted_heaps().has_value()", indent.num_spaces(), "");
      }
    }

    uint32_t image_format_constraints_count =
        c.image_format_constraints().has_value()
            ? safe_cast<uint32_t>(c.image_format_constraints()->size())
            : 0;
    LogInfo(FROM_HERE, "%*simage_format_constraints.count() %u", indent.num_spaces(), "",
            image_format_constraints_count);
    {  // scope indent
      auto indent = indent_tracker.Nested();
      for (uint32_t i = 0; i < image_format_constraints_count; ++i) {
        LogInfo(FROM_HERE, "%*simage_format_constraints[%u] (ifc):", indent.num_spaces(), "", i);
        const fuchsia_sysmem2::ImageFormatConstraints& ifc = c.image_format_constraints()->at(i);
        LogImageFormatConstraints(indent_tracker, node_properties, ifc);
      }
    }
  }
}

void LogicalBufferCollection::LogBufferCollectionInfo(
    IndentTracker& indent_tracker, const fuchsia_sysmem2::BufferCollectionInfo& bci) const {
  ZX_DEBUG_ASSERT(is_verbose_logging());
  auto indent = indent_tracker.Nested();
  NodeProperties* node_properties = nullptr;

  LOG_UINT64_FIELD(FROM_HERE, indent, bci, buffer_collection_id);
  LogInfo(FROM_HERE, "%*ssettings:", indent.num_spaces(), "");
  if (!bci.settings().has_value()) {
    LogInfo(FROM_HERE, "%*s!bci.settings().has_value()", indent.num_spaces(), "");
  } else {
    auto indent = indent_tracker.Nested();
    const auto& sbs = *bci.settings();
    if (!sbs.buffer_settings().has_value()) {
      LogInfo(FROM_HERE, "%*s!sbs.buffer_settings().has_value()", indent.num_spaces(), "");
    } else {
      LogInfo(FROM_HERE, "%*sbuffer_settings:", indent.num_spaces(), "");
      auto indent = indent_tracker.Nested();
      const auto& bms = *sbs.buffer_settings();
      LOG_UINT64_FIELD(FROM_HERE, indent, bms, size_bytes);
      LOG_UINT64_FIELD(FROM_HERE, indent, bms, raw_vmo_size);
      LOG_BOOL_FIELD(FROM_HERE, indent, bms, is_physically_contiguous);
      LOG_BOOL_FIELD(FROM_HERE, indent, bms, is_secure);
      if (bms.coherency_domain().has_value()) {
        LogInfo(FROM_HERE, "%*scoherency_domain: %u", indent.num_spaces(), "",
                *bms.coherency_domain());
      } else {
        LogInfo(FROM_HERE, "%*s!bms.coherency_domain().has_value()", indent.num_spaces(), "");
      }
      if (!bms.heap().has_value()) {
        LogInfo(FROM_HERE, "%*s!bms.heap().has_value()", indent.num_spaces(), "");
      } else {
        if (bms.heap()->heap_type().has_value()) {
          LogInfo(FROM_HERE, "%*sheap.heap_type: %s", indent.num_spaces(), "",
                  bms.heap()->heap_type().value().c_str());
        } else {
          LogInfo(FROM_HERE, "%*s!bms.heap()->heap_type().has_value()", indent.num_spaces(), "");
        }
        if (bms.heap()->id().has_value()) {
          LogInfo(FROM_HERE, "%*sheap.id: 0x%" PRIx64, indent.num_spaces(), "",
                  bms.heap()->id().value());
        } else {
          LogInfo(FROM_HERE, "%*s!bms.heap()->id().has_value()", indent.num_spaces(), "");
        }
      }
    }
    if (!sbs.image_format_constraints().has_value()) {
      LogInfo(FROM_HERE, "%*s!sbs.image_format_constraints().has_value()", indent.num_spaces(), "");
    } else {
      LogInfo(FROM_HERE, "%*simage_format_constraints:", indent.num_spaces(), "");
      const auto& ifc = *sbs.image_format_constraints();
      LogImageFormatConstraints(indent_tracker, nullptr, ifc);
    }
  }
  if (bci.buffers().has_value()) {
    LogInfo(FROM_HERE, "%*sbuffers.size(): %zu", indent.num_spaces(), "", bci.buffers()->size());
  } else {
    LogInfo(FROM_HERE, "%*s!bci.buffers().has_value()", indent.num_spaces(), "");
  }
  // For now we don't log per-buffer info.
}

void LogicalBufferCollection::LogPixelFormatAndModifier(
    IndentTracker& indent_tracker, NodeProperties* node_properties,
    const fuchsia_sysmem2::PixelFormatAndModifier& pixel_format_and_modifier) const {
  auto indent = indent_tracker.Current();
  LogInfo(FROM_HERE, "%*spixel_format(): %u", indent.num_spaces(), "",
          sysmem::fidl_underlying_cast(pixel_format_and_modifier.pixel_format()));
  LogInfo(FROM_HERE, "%*spixel_format_modifier(): 0x%" PRIx64, indent.num_spaces(), "",
          sysmem::fidl_underlying_cast(pixel_format_and_modifier.pixel_format_modifier()));
}

void LogicalBufferCollection::LogImageFormatConstraints(
    IndentTracker& indent_tracker, NodeProperties* node_properties,
    const fuchsia_sysmem2::ImageFormatConstraints& ifc) const {
  auto indent = indent_tracker.Nested();

  LOG_UINT32_FIELD(FROM_HERE, indent, ifc, pixel_format);
  LOG_UINT64_FIELD(FROM_HERE, indent, ifc, pixel_format_modifier);
  if (ifc.pixel_format_and_modifiers().has_value()) {
    LogInfo(FROM_HERE, "%*spixel_format_and_modifiers.size(): %zu", indent.num_spaces(), "",
            ifc.pixel_format_and_modifiers()->size());
    auto indent = indent_tracker.Nested();
    for (uint32_t i = 0; i < ifc.pixel_format_and_modifiers()->size(); ++i) {
      LogPixelFormatAndModifier(indent_tracker, node_properties,
                                ifc.pixel_format_and_modifiers()->at(i));
    }
  }

  LOG_SIZEU_FIELD(FROM_HERE, indent, ifc, min_size);
  LOG_SIZEU_FIELD(FROM_HERE, indent, ifc, max_size);

  LOG_UINT32_FIELD(FROM_HERE, indent, ifc, min_bytes_per_row);
  LOG_UINT32_FIELD(FROM_HERE, indent, ifc, max_bytes_per_row);

  LOG_UINT64_FIELD(FROM_HERE, indent, ifc, max_width_times_height);

  LOG_SIZEU_FIELD(FROM_HERE, indent, ifc, size_alignment);
  LOG_SIZEU_FIELD(FROM_HERE, indent, ifc, display_rect_alignment);

  LOG_SIZEU_FIELD(FROM_HERE, indent, ifc, required_min_size);
  LOG_SIZEU_FIELD(FROM_HERE, indent, ifc, required_max_size);
  if (ifc.required_max_size_list().has_value()) {
    LogInfo(FROM_HERE, "%*srequired_max_size_list.size(): %zu", indent.num_spaces(), "",
            ifc.required_max_size_list()->size());
    auto indent = indent_tracker.Nested();
    for (uint32_t i = 0; i < ifc.required_max_size_list()->size(); ++i) {
      auto& required_max_size = ifc.required_max_size_list()->at(i);
      LogInfo(FROM_HERE, "%*srequired_max_size_list[%u]: {%u, %u}", indent.num_spaces(), "", i,
              required_max_size.width(), required_max_size.height());
    }
  }

  LOG_UINT32_FIELD(FROM_HERE, indent, ifc, bytes_per_row_divisor);

  LOG_UINT32_FIELD(FROM_HERE, indent, ifc, start_offset_divisor);
}

void LogicalBufferCollection::LogPrunedSubTree(NodeProperties* subtree) const {
  ZX_DEBUG_ASSERT(is_verbose_logging());
  LogInfo(FROM_HERE, "collection name: %s", name().has_value() ? name()->c_str() : "<nullopt>");
  ignore_result(subtree->DepthFirstPreOrder(PrunedSubtreeFilter(*subtree, [this, subtree](
                                                                              const NodeProperties&
                                                                                  node_properties) {
    uint32_t depth = 0;
    for (auto iter = &node_properties; iter != subtree; iter = iter->parent()) {
      ++depth;
    }

    const char* logical_type_name = "?";
    if (node_properties.is_token()) {
      logical_type_name = "token";
    } else if (node_properties.is_token_group()) {
      logical_type_name = "group";
    } else if (node_properties.is_collection()) {
      logical_type_name = "collection";
    }

    const char* connection_version_string = "?";
    switch (node_properties.connection_version()) {
      case ConnectionVersion::kNoConnection:
        connection_version_string = "<none>";
        break;
      case ConnectionVersion::kVersion1:
        connection_version_string = "v1";
        break;
      case ConnectionVersion::kVersion2:
        connection_version_string = "v2";
        break;
    }

    LogInfo(
        FROM_HERE,
        "%*sNodeProperties: %p (%s;%s;%s) has_constraints: %u ready: %u weak: %u weak_ok: %u weak_ok_for_child_nodes_also: %u weak_ok_from_parent: %u client_name: %s",
        2 * depth, "", &node_properties, node_properties.node_type_name(),
        connection_version_string, logical_type_name, node_properties.has_constraints(),
        node_properties.node()->ReadyForAllocation(), node_properties.is_weak(),
        node_properties.is_weak_ok(), node_properties.is_weak_ok_for_child_nodes_also(),
        node_properties.is_weak_ok_from_parent(), node_properties.client_debug_info().name.c_str());
    // No need to keep the nodes in a list; we've already done what we need to do during the
    // visit.
    return false;
  })));
}

void LogicalBufferCollection::LogNodeConstraints(std::vector<NodeProperties*> nodes) const {
  ZX_DEBUG_ASSERT(is_verbose_logging());
  for (auto node : nodes) {
    LogInfo(FROM_HERE, "Constraints for NodeProperties: %p (%s)", node, node->node_type_name());
    if (!node->buffer_collection_constraints()) {
      LogInfo(FROM_HERE, "No constraints in node: %p (%s)", node, node->node_type_name());
    } else {
      LogConstraints(FROM_HERE, node, *node->buffer_collection_constraints());
    }
  }
}

void LogicalBuffer::ComplainLoudlyAboutStillExistingWeakVmoHandles() {
  if (weak_parent_vmos_.empty()) {
    return;
  }
  for (auto& [key, weak_parent] : weak_parent_vmos_) {
    const char* collection_name = logical_buffer_collection_->name_.has_value()
                                      ? logical_buffer_collection_->name_->name.c_str()
                                      : "Unknown";
    auto* client_debug_info = weak_parent->get_client_debug_info();
    ZX_DEBUG_ASSERT(client_debug_info);

    logical_buffer_collection_->parent_sysmem_->metrics().LogCloseWeakAsapTakingTooLong();

    // The client may have created a child VMO based on the handed-out VMO and closed the handed-out
    // VMO, which still counts as failure to close the handed-out weak VMO in a timely manner; the
    // originally-handed-out koid may or may not be useful.
    logical_buffer_collection_->LogError(
        FROM_HERE,
        "#####################################################################################");
    logical_buffer_collection_->LogError(
        FROM_HERE, "sysmem weak VMO handle still open long after close_weak_asap signalled:");
    logical_buffer_collection_->LogError(FROM_HERE, "  collection: 0x%" PRIx64 " %s",
                                         logical_buffer_collection_->buffer_collection_id_,
                                         collection_name);
    logical_buffer_collection_->LogError(FROM_HERE, "  buffer_index: %u", buffer_index_);
    logical_buffer_collection_->LogError(
        FROM_HERE, "  client: %s (id: 0x%" PRIx64 ")",
        client_debug_info->name.size() ? client_debug_info->name.c_str() : "<empty>",
        client_debug_info->id);
    logical_buffer_collection_->LogError(FROM_HERE, "  originally-handed-out koid: 0x%" PRIx64,
                                         *weak_parent->child_koid());
    logical_buffer_collection_->LogError(
        FROM_HERE,
        "#####################################################################################");
  }
}

void LogicalBufferCollection::IncStrongNodeTally() { ++strong_node_count_; }

void LogicalBufferCollection::DecStrongNodeTally() {
  ZX_DEBUG_ASSERT(strong_node_count_ >= 1);
  --strong_node_count_;
  CheckForZeroStrongNodes();
}

void LogicalBufferCollection::CheckForZeroStrongNodes() {
  if (strong_node_count_ == 0) {
    parent_sysmem_->PostTask([this, ref_this = fbl::RefPtr(this)] {
      if (allocation_result_info_.has_value()) {
        // Regardless of whether we'd allocated by the time of posting, we've allocated by the
        // time of running the posted task.
        //
        // If we allocated by the time of posting, we may have some strong VMOs outstanding with
        // clients, and weak nodes should continue to work until all outstanding strong VMOs are
        // closed by clients.
        //
        // If we hadn't allocated by the time of posting, we can still handle this the same way,
        // because CheckForZeroStrongParentVmoCount will take care of failing from the root_
        // down shortly after we reset() the vmo fields here (triggered async by
        // ZX_VMO_ZERO_CHILDREN signal to strong_parent_vmo_).
        for (auto& buffer : *allocation_result_info_->buffers()) {
          // The sysmem strong VMO handles in allocation_result_info_ are logically owned by
          // strong nodes. Once there are zero strong nodes, there can never be a strong node
          // again, so we reset the strong VMOs here. In some cases there can still be strong
          // VMOs outstanding, and those still prevent strong_parent_vmo_ entries from
          // disappearing until all the outstanding strong VMO handles are closed by clients.
          //
          // Once all strong_parent_vmo_ entries are gone, the LogicalBufferCollection will fail
          // from root_ down.
          buffer.vmo().reset();
        }
      } else {
        // Since we know we have zero strong VMOs outstanding (because no VMOs allocated yet),
        // this is conceptually skipping to the root_-down fail that we'd otherwise do elsewhere
        // upon CheckForZeroStrongParentVmoCount noticing zero strong VMOs, since we now know
        // that strong_parent_vmo_count_ never became non-zero and will remain zero from this
        // point onward.
        //
        // We also know in this path that we have zero weak VMOs outstanding (because no VMOs
        // allocated yet), so there's no need to signal any close_weak_asap(s) here since no
        // buffers are ever part of this LogicalBufferCollection, and no outstanding weak VMOs
        // will be holding up ~LogicalBufferCollection.
        if (!root_) {
          // This is just avoiding any redundant log output in case the LogicalBufferCollection
          // already failed from the root_ down already for a different reason.
          return;
        }
        LogAndFailRootNode(
            FROM_HERE, Error::kUnspecified,
            "Zero strong nodes remaining before allocation; failing LogicalBufferCollection");
      }
      // ~ref_this
    });
  }
}

void LogicalBufferCollection::IncStrongParentVmoCount() { ++strong_parent_vmo_count_; }

void LogicalBufferCollection::DecStrongParentVmoCount() {
  --strong_parent_vmo_count_;
  CheckForZeroStrongParentVmoCount();
}

void LogicalBufferCollection::CheckForZeroStrongParentVmoCount() {
  if (strong_parent_vmo_count_ == 0) {
    parent_sysmem_->PostTask([this, ref_this = fbl::RefPtr(this)] {
      FailRootNode(Error::kUnspecified);
      // ~ref_this
    });
  }
}

void LogicalBufferCollection::CreateParentVmoInspect(zx_koid_t parent_vmo_koid) {
  auto node =
      inspect_node_.CreateChild(fbl::StringPrintf("parent_vmo-%ld", parent_vmo_koid).c_str());
  node.CreateUint("koid", parent_vmo_koid, &vmo_properties_);
  vmo_properties_.emplace(std::move(node));
}

void LogicalBufferCollection::ClearBuffers() {
  // deallocate any previoulsy-allocated buffers sync instead of async (not relying on drop of
  // result to trigger ZX_VMO_ZERO_CHILDREN async etc)
  //
  // move out and delete that since ~LogicalBuffer mutates buffers_
  auto local_buffers = std::move(buffers_);
  buffers_.clear();
  local_buffers.clear();
}

bool LogicalBufferCollection::FlattenPixelFormatAndModifiers(
    const fuchsia_sysmem2::BufferUsage& buffer_usage,
    fuchsia_sysmem2::BufferCollectionConstraints& constraints) {
  auto src = std::move(*constraints.image_format_constraints());
  auto& dst = constraints.image_format_constraints().emplace();
  for (uint32_t i = 0; i < src.size(); ++i) {
    auto& ifc = src[i];

    if (!ifc.pixel_format().has_value()) {
      if (ifc.pixel_format_modifier().has_value()) {
        LogError(FROM_HERE,
                 "pixel_format must be set when pixel_format_modifier is set - index: %u", i);
        return false;
      }
      if (!ifc.pixel_format_and_modifiers().has_value() ||
          ifc.pixel_format_and_modifiers()->empty()) {
        // must have at least one pixel_format field set by participant, per ImageFormatConstraints
        LogError(FROM_HERE,
                 "pixel_format must be set when zero pixel_format_and_modifiers - index: %u", i);
        return false;
      }
    }

    if (ifc.pixel_format().has_value()) {
      if (*ifc.pixel_format() == fuchsia_images2::PixelFormat::kDoNotCare) {
        // pixel_format kDoNotCare and un-set pixel_format_modifier implies
        // kFormatModifierDoNotCare. A client that wants to constrain pixel_format_modifier can set
        // the pixel_format_modifier field.
        FIELD_DEFAULT(ifc, pixel_format_modifier, fuchsia_images2::PixelFormatModifier::kDoNotCare);
      } else if (IsAnyUsage(buffer_usage)) {
        // When pixel_format != kDoNotCare, kFormatModifierNone / kFormatModifierLinear is the
        // default when there is any usage. A client with usage which doesn't care what the pixel
        // format modifier is (what tiling is used) can explicitly specify kFormatModifierDoNotCare
        // instead of leaving pixel_format_modifier un-set..
        FIELD_DEFAULT(ifc, pixel_format_modifier, fuchsia_images2::PixelFormatModifier::kLinear);
      } else {
        // A client with no usage, which also doesn't set pixel_format_modifier, doesn't prevent
        // using a tiled format.
        FIELD_DEFAULT(ifc, pixel_format_modifier, fuchsia_images2::PixelFormatModifier::kDoNotCare);
      }
    }

    if (!ifc.pixel_format_and_modifiers().has_value()) {
      ZX_DEBUG_ASSERT(ifc.pixel_format().has_value());
      ZX_DEBUG_ASSERT(ifc.pixel_format_modifier().has_value());
      // no flattening needed for this src item
      dst.emplace_back(std::move(ifc));
      continue;
    }
    ZX_DEBUG_ASSERT(ifc.pixel_format_and_modifiers().has_value());

    // If pixel_format is set, move pixel_format, pixel_format_modifier into an additional entry of
    // pixel_format_and_modifiers, in preparation for flattening.

    auto pixel_format_and_modifiers = std::move(*ifc.pixel_format_and_modifiers());
    ifc.pixel_format_and_modifiers().reset();

    if (ifc.pixel_format().has_value()) {
      pixel_format_and_modifiers.emplace_back(fuchsia_sysmem2::PixelFormatAndModifier(
          *ifc.pixel_format(), *ifc.pixel_format_modifier()));
      ifc.pixel_format().reset();
      ifc.pixel_format_modifier().reset();
    }

    ZX_DEBUG_ASSERT(!ifc.pixel_format().has_value());
    ZX_DEBUG_ASSERT(!ifc.pixel_format_modifier().has_value());
    ZX_DEBUG_ASSERT(!ifc.pixel_format_and_modifiers().has_value());

    // If we can do a single move, go ahead and do that. Beyond this point we'll only clone.
    ZX_DEBUG_ASSERT(!pixel_format_and_modifiers.empty());
    if (pixel_format_and_modifiers.size() == 1) {
      auto& new_ifc = dst.emplace_back(std::move(ifc));
      new_ifc.pixel_format() = pixel_format_and_modifiers[0].pixel_format();
      new_ifc.pixel_format_modifier() = pixel_format_and_modifiers[0].pixel_format_modifier();
      continue;
    }

    for (auto& pixel_format_and_modifier : pixel_format_and_modifiers) {
      // intentional copy/clone
      auto& new_ifc = dst.emplace_back(ifc);
      new_ifc.pixel_format() = pixel_format_and_modifier.pixel_format();
      new_ifc.pixel_format_modifier() = pixel_format_and_modifier.pixel_format_modifier();
    }
  }

  return true;
}

void LogicalBufferCollection::LogSummary(IndentTracker& indent_tracker) {
  auto indent = indent_tracker.Current();
  LOG(INFO, "%*scollection %p:", indent.num_spaces(), "", this);
  {
    auto indent = indent_tracker.Nested();
    auto maybe_name = name();
    LOG(INFO, "%*sname: %s", indent.num_spaces(), "",
        maybe_name.has_value() ? (*maybe_name).c_str() : "<none>");
    if (!has_allocation_result_) {
      LOG(INFO, "%*spending allocation", indent.num_spaces(), "");
    } else {
      size_t orig_count = 0;
      std::optional<double> per_buffer_MiB;
      if (buffer_collection_info_before_population_.has_value()) {
        orig_count = buffer_collection_info_before_population_->buffers()->size();
        LOG(INFO, "%*sorig buffer count: %" PRId64, indent.num_spaces(), "", orig_count);
        per_buffer_MiB = static_cast<double>(*buffer_collection_info_before_population_->settings()
                                                  ->buffer_settings()
                                                  ->size_bytes()) /
                         1024.0 / 1024.0;
        LOG(INFO, "%*sper-buffer MiB: %g", indent.num_spaces(), "", *per_buffer_MiB);
        LOG(INFO, "%*sis_contiguous: %u", indent.num_spaces(), "",
            *buffer_collection_info_before_population_->settings()
                 ->buffer_settings()
                 ->is_physically_contiguous());
        LOG(INFO, "%*sis_secure: %u", indent.num_spaces(), "",
            *buffer_collection_info_before_population_->settings()->buffer_settings()->is_secure());
      }
      LOG(INFO, "%*sstrong_node_count_: %u allocation_result_info_.has_value(): %u",
          indent.num_spaces(), "", strong_node_count_, allocation_result_info_.has_value());
      uint32_t present_buffers = 0;
      if (!has_allocation_result_) {
        LOG(INFO, "%*spending allocation", indent.num_spaces(), "");
      } else {
        {
          auto indent = indent_tracker.Nested();
          for (uint32_t i = 0; i < orig_count; ++i) {
            auto iter = buffers_.find(i);
            if (iter == buffers_.end()) {
              LOG(INFO, "%*s[%u] absent", indent.num_spaces(), "", i);
            } else {
              ++present_buffers;
              auto& logical_buffer = iter->second;

              ZX_ASSERT(!!logical_buffer->parent_vmo_);
              zx_signals_t pending = 0;
              ZX_ASSERT(ZX_ERR_TIMED_OUT == logical_buffer->parent_vmo_->vmo().wait_one(
                                                0, zx::time::infinite_past(), &pending));
              bool is_zero_children = !!(pending & ZX_VMO_ZERO_CHILDREN);
              LOG(INFO, "%*s[%u] parent is_zero_children: %u", indent.num_spaces(), "", i,
                  is_zero_children);

              bool is_strong_parent_vmo_present = !!logical_buffer->strong_parent_vmo_;
              LOG(INFO, "%*s[%u] strong_parent_vmo_ present: %u", indent.num_spaces(), "", i,
                  is_strong_parent_vmo_present);
              if (is_strong_parent_vmo_present) {
                auto indent = indent_tracker.Nested();
                zx_signals_t pending = 0;
                ZX_ASSERT(ZX_ERR_TIMED_OUT == logical_buffer->strong_parent_vmo_->vmo().wait_one(
                                                  0, zx::time::infinite_past(), &pending));
                bool is_zero_children = !!(pending & ZX_VMO_ZERO_CHILDREN);
                LOG(INFO, "%*s[%u] strong is_zero_children: %u", indent.num_spaces(), "", i,
                    is_zero_children);
                // Log koid saved in parent. The strong_child_vmo_ is reset by this point.
                LOG(INFO, "%*s[%u] strong_child_vmo koid: %" PRIu64, indent.num_spaces(), "", i,
                    logical_buffer->strong_parent_vmo_->child_koid().value());
              }
            }
          }
        }
      }
      LOG(INFO, "%*spresent_buffers: %u", indent.num_spaces(), "", present_buffers);
      if (per_buffer_MiB.has_value()) {
        LOG(INFO, "%*sMiB: %g", indent.num_spaces(), "", *per_buffer_MiB * present_buffers);
      }
    }
  }
}

bool LogicalBufferCollection::IsSecureHeap(const fuchsia_sysmem2::Heap& heap) {
  bool found_in_secure_heaps = false;
  parent_sysmem_->ForeachSecureHeap(
      [&heap, &found_in_secure_heaps](const fuchsia_sysmem2::Heap& secure_heap) -> bool {
        if (heap == secure_heap) {
          found_in_secure_heaps = true;
          // stop iterating
          return false;
        }
        // keep going
        return true;
      });
  return found_in_secure_heaps;
}

bool LogicalBufferCollection::IsSecurePermitted(
    const fuchsia_sysmem2::BufferMemoryConstraints& constraints) {
  // Without this, we don't know about all the expected secure heaps yet, so can't determine if the
  // constraints are permitting a secure heap that is missing currently but will be present shortly.
  ZX_DEBUG_ASSERT_MSG(parent_sysmem_->is_secure_mem_ready(),
                      "IsSecurePermitted called before is_secure_mem_ready");
  if (!*constraints.inaccessible_domain_supported()) {
    return false;
  }
  bool found_permitted_secure_heap = false;
  parent_sysmem_->ForeachSecureHeap([&constraints, &found_permitted_secure_heap](
                                        const fuchsia_sysmem2::Heap secure_heap) -> bool {
    if (IsPermittedHeap(constraints, secure_heap)) {
      found_permitted_secure_heap = true;
      // stop iterating over secure heaps
      return false;
    }
    // keep going
    return true;
  });
  return found_permitted_secure_heap;
}

bool LogicalBufferCollection::CheckConstraintsAgainstExistingSettings(
    const fuchsia_sysmem2::BufferCollectionConstraints& constraints) {
  const auto& settings = single_buffer_settings();
  ConstraintsList constraints_list;
  std::vector<std::unique_ptr<NodeProperties>> tmp_nodes;
  // This tmp_node doesn't need BufferCollectionConstraints; we add must_match_constraints to
  // constraints_list directly below. We may remove the BufferCollectionConstraints parameter from
  // NewTemporary before long, or make the NodeProperties of Constraints optional.
  auto tmp_node = NodeProperties::NewTemporary(this, fuchsia_sysmem2::BufferCollectionConstraints{},
                                               "CheckConstraintsAgainstExistingSettings");
  auto* tmp_node_ptr = tmp_node.get();
  tmp_nodes.emplace_back(std::move(tmp_node));
  constraints_list.emplace_back(constraints, settings, *tmp_node_ptr);
  AugmentConstraintsList(constraints_list, tmp_nodes);
  return CombineConstraints(&constraints_list).is_ok();
}

fit::result<zx_status_t, std::unique_ptr<LogicalBuffer>> LogicalBuffer::Create(
    fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection, uint32_t buffer_index,
    zx::vmo parent_vmo) {
  auto result = std::unique_ptr<LogicalBuffer>(
      new LogicalBuffer(std::move(logical_buffer_collection), buffer_index, std::move(parent_vmo)));
  if (!result->is_ok()) {
    return fit::error(result->error());
  }
  return fit::ok(std::move(result));
}

LogicalBuffer::LogicalBuffer(fbl::RefPtr<LogicalBufferCollection> logical_buffer_collection,
                             uint32_t buffer_index, zx::vmo parent_vmo)
    : logical_buffer_collection_(logical_buffer_collection), buffer_index_(buffer_index) {
  TRACE_DURATION("gfx", "LogicalBuffer::LogicalBuffer", "buffer_index", buffer_index_);
  auto ensure_error_set = fit::defer([this] {
    if (error_ == ZX_OK) {
      error_ = ZX_ERR_INTERNAL;
    }
  });

  auto* allocator = logical_buffer_collection_->memory_allocator_;
  ZX_DEBUG_ASSERT(allocator);

  // Clean up in any error path until parent_vmo_ do_delete takes over.
  auto cleanup_parent_vmo =
      fit::defer([allocator, &parent_vmo] { allocator->Delete(std::move(parent_vmo)); });

  auto complain_about_leaked_weak_timer_task_scope =
      std::make_shared<std::optional<async_patterns::TaskScope>>();
  complain_about_leaked_weak_timer_task_scope->emplace(
      logical_buffer_collection_->parent_sysmem_->loop_dispatcher());

  zx_info_vmo_t info;
  zx_status_t status = parent_vmo.get_info(ZX_INFO_VMO, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    logical_buffer_collection_->LogError(
        FROM_HERE, "raw_parent_vmo.get_info(ZX_INFO_VMO) failed - status %d", status);
    error_ = status;
    return;
  }
  logical_buffer_collection_->CreateParentVmoInspect(info.koid);

  // Write zeroes to the VMO, so that the allocator doesn't need to.  Also flush those zeroes to
  // RAM so the newly-allocated VMO is fully zeroed in both RAM and CPU coherency domains.
  //
  // This is measured to be significantly more than half the overall time cost when repeatedly
  // allocating and deallocating a buffer collection with 4MiB buffer space per collection.  On
  // astro this was measured to be ~2100us out of ~2550us per-cycle duration.  Larger buffer space
  // per collection would take longer here.
  //
  // If we find this is taking too long, we could ask the allocator if it's already providing
  // pre-zeroed VMOs.  And/or zero allocator backing space async during deallocation, but wait on
  // deallocations to be done before failing a new allocation.
  //
  // TODO(https://fxbug.dev/42109914): Zero secure/protected VMOs.
  const auto& heap_properties = allocator->heap_properties();
  ZX_DEBUG_ASSERT(heap_properties.coherency_domain_support().has_value());
  ZX_DEBUG_ASSERT(heap_properties.need_clear().has_value());
  if (*heap_properties.need_clear() && !allocator->is_already_cleared_on_allocate()) {
    status = parent_vmo.op_range(ZX_VMO_OP_ZERO, 0, info.size_bytes, nullptr, 0);
    if (status != ZX_OK) {
      logical_buffer_collection_->LogError(
          FROM_HERE, "parent_vmo.op_range(ZX_VMO_OP_ZERO) failed - status: %d", status);
      error_ = status;
      return;
    }
  }
  if (*heap_properties.need_clear() ||
      (heap_properties.need_flush().has_value() && *heap_properties.need_flush())) {
    // Flush out the zeroes written above, or the zeroes that are already in the pages (but not
    // flushed yet) thanks to zx_vmo_create_contiguous(), or zeroes that are already in the pages
    // (but not necessarily flushed yet) thanks to whatever other allocator strategy.  The barrier
    // after this flush is in the caller after all the VMOs are allocated.
    status = parent_vmo.op_range(ZX_VMO_OP_CACHE_CLEAN, 0, info.size_bytes, nullptr, 0);
    if (status != ZX_OK) {
      logical_buffer_collection_->LogError(
          FROM_HERE, "raw_parent_vmo.op_range(ZX_VMO_OP_CACHE_CLEAN) failed - status: %d", status);
      error_ = status;
      return;
    }
    // We don't need a BarrierAfterFlush() here because Zircon takes care of that in the
    // zx_vmo_op_range(ZX_VMO_OP_CACHE_CLEAN).
  }

  // The tracked_parent_vmo takes care of calling allocator.Delete() if this method returns early.
  // We intentionally don't emplace into parent_vmo_ until StartWait() has succeeded.  In turn,
  // StartWait() requires a child VMO to have been created already (else ZX_VMO_ZERO_CHILDREN would
  // trigger too soon).
  //
  // We need to keep the raw_parent_vmo around so we can wait for ZX_VMO_ZERO_CHILDREN, and so we
  // can call allocator.Delete(raw_parent_vmo).
  //
  // Until that happens, we can't let LogicalBufferCollection itself go away, because it needs to
  // stick around to tell allocator that the allocator's VMO can be deleted/reclaimed.
  //
  // We let attenuated_strong_parent_vmo go away before returning from this method, since it's only
  // purpose was to attenuate the rights of strong_child_vmo.  The strong_child_vmo counts as a
  // child of parent_vmo for ZX_VMO_ZERO_CHILDREN.
  //
  // The fbl::RefPtr(this) is fairly similar (in this usage) to shared_from_this().
  //
  // We avoid letting any duplicates of raw_parent_vmo escape this method, to ensure that this
  // TrackedParentVmo will authoritatively know that when there are zero children of raw_parent_vmo,
  // there are also zero duplicated handles, ensuring coherency with respect to allocator->Delete()
  // called from this do_delete.
  parent_vmo_ = std::unique_ptr<TrackedParentVmo>(new TrackedParentVmo(
      fbl::RefPtr(logical_buffer_collection_), std::move(parent_vmo), buffer_index_,
      [this, allocator,
       complain_about_leaked_weak_timer_task_scope](TrackedParentVmo* tracked_parent_vmo) mutable {
        // This timer, if it was set, is cancelled now; the -> instead of . here is significant;
        // we're intentinoally ending the TaskScope, not just this shared_ptr<> to it.
        complain_about_leaked_weak_timer_task_scope->reset();

        if (close_weak_asap_time_.has_value()) {
          zx::duration close_weak_asap_duration =
              zx::clock::get_monotonic() - *close_weak_asap_time_;
          logical_buffer_collection_->parent_sysmem_->metrics().LogCloseWeakAsapDuration(
              close_weak_asap_duration);
        }

        // This won't find a node if do_delete is running in a sufficiently-early error path.
        auto buffer_node =
            logical_buffer_collection_->buffers_.extract(tracked_parent_vmo->buffer_index());
        ZX_DEBUG_ASSERT_MSG(!buffer_node || (buffer_node.mapped().get() == this),
                            "!!buffer_node: %u buffer_node.mapped().get(): %p this: %p",
                            !!buffer_node, buffer_node.mapped().get(), this);
        allocator->Delete(tracked_parent_vmo->TakeVmo());
        logical_buffer_collection_->SweepLifetimeTracking();
        // ~buffer_node may delete the LogicalBufferCollection (via ~parent_vmo_).
      }));
  // tracked_parent_vmo do_delete will take care of calling allocator->Delete(parent_vmo),
  // whether do_delete is called in an error path or later when tracked_parent_vmo sees
  // ZX_VMO_ZERO_CHILDREN.
  cleanup_parent_vmo.cancel();

  // As a child directly under parent_vmo, we have the strong_only VMO which in turn is the parent
  // of (only) all the sysmem strong VMO handles. When strong_only's do_delete runs, we know that
  // zero sysmem strong VMO handles remain, so we can close corresponding close_weak_asap handles,
  // signaling to clients to close all their sysmem weak VMO handles asap.
  zx::vmo strong_parent_vmo;
  status =
      parent_vmo_->vmo().create_child(ZX_VMO_CHILD_SLICE, 0, info.size_bytes, &strong_parent_vmo);
  if (status != ZX_OK) {
    logical_buffer_collection_->LogError(FROM_HERE, "zx::vmo::create_child() failed - status: %d",
                                         status);
    error_ = status;
    return;
  }

  // Since strong BufferCollection client_end(s) effectively keep a strong VMO (via
  // strong_collection_count_ not being zero yet -> keeping strong VMOs in allocation_result()), we
  // know that this do_delete won't run until all strong BufferCollection client_end(s) are also
  // gone (can be closed first from client_end or server_end, but either way, gone from server's
  // point of view), or in a creation-time error path.
  strong_parent_vmo_ = std::unique_ptr<TrackedParentVmo>(new TrackedParentVmo(
      fbl::RefPtr(logical_buffer_collection_), std::move(strong_parent_vmo), buffer_index_,
      [this, complain_about_leaked_weak_timer_task_scope](
          TrackedParentVmo* tracked_strong_parent_vmo) mutable {
        if (tracked_strong_parent_vmo->child_koid().has_value()) {
          logical_buffer_collection_->parent_sysmem_->RemoveVmoKoid(
              *tracked_strong_parent_vmo->child_koid());
        }

        // won't have a pointer if ~LogicalBuffer before ZX_VMO_ZERO_CHILDREN
        auto local_tracked_strong_parent_vmo = std::move(strong_parent_vmo_);
        ZX_DEBUG_ASSERT(!strong_parent_vmo_);

        logical_buffer_collection_->DecStrongParentVmoCount();

        // We know this still has_value() because node_handle in this scope has a vmo that must
        // close before the tracked_parent_vmo do_delete can run.
        ZX_DEBUG_ASSERT(complain_about_leaked_weak_timer_task_scope->has_value());
        (*complain_about_leaked_weak_timer_task_scope)
            ->PostDelayed([this] { ComplainLoudlyAboutStillExistingWeakVmoHandles(); }, zx::sec(5));

        // signal client_weak_asap client_end(s) to tell clients holding weak VMOs to close any weak
        // VMOs asap, now that there are zero strong VMOs
        if (close_weak_asap_.has_value()) {
          close_weak_asap_->server_end.reset();
        }

        if (!weak_parent_vmos_.empty()) {
          close_weak_asap_time_ = zx::clock::get_monotonic();
        }

        // ~local_tracked_strong_parent_vmo will close its vmo_, which may trigger
        // tracked_parent_vmo's do_delete defined above (if zero weak outstanding already)
      }));
  logical_buffer_collection_->IncStrongParentVmoCount();

  zx::vmo attenuated_strong_parent_vmo;
  status = strong_parent_vmo_->vmo().duplicate(kSysmemVmoRights, &attenuated_strong_parent_vmo);
  if (status != ZX_OK) {
    logical_buffer_collection_->LogError(FROM_HERE, "zx::object::duplicate() failed - status: %d",
                                         status);
    error_ = status;
    return;
  }

  zx::vmo strong_child_vmo;
  status = attenuated_strong_parent_vmo.create_child(ZX_VMO_CHILD_SLICE, 0, info.size_bytes,
                                                     &strong_child_vmo);
  if (status != ZX_OK) {
    logical_buffer_collection_->LogError(FROM_HERE, "zx::vmo::create_child() failed - status: %d",
                                         status);
    error_ = status;
    return;
  }

  zx_koid_t strong_child_vmo_koid = get_koid(strong_child_vmo);
  if (strong_child_vmo_koid == ZX_KOID_INVALID) {
    logical_buffer_collection_->LogError(FROM_HERE, "get_koid failed");
    error_ = ZX_ERR_INTERNAL;
    return;
  }

  logical_buffer_collection_->parent_sysmem_->AddVmoKoid(strong_child_vmo_koid, false, *this);
  strong_parent_vmo_->set_child_koid(strong_child_vmo_koid);

  TRACE_INSTANT("gfx", "Child VMO created", TRACE_SCOPE_THREAD, "koid", strong_child_vmo_koid);

  // Now that we know at least one child of parent_vmo_ exists, we can StartWait().
  status = parent_vmo_->StartWait(logical_buffer_collection_->parent_sysmem_->loop_dispatcher());
  if (status != ZX_OK) {
    logical_buffer_collection_->LogError(FROM_HERE,
                                         "tracked_parent->StartWait() failed - status: %d", status);
    error_ = status;
    return;
  }

  // Now that we know at least one child of strong_parent_vmo_ exists, we can StartWait().
  status =
      strong_parent_vmo_->StartWait(logical_buffer_collection_->parent_sysmem_->loop_dispatcher());
  if (status != ZX_OK) {
    logical_buffer_collection_->LogError(FROM_HERE,
                                         "tracked_parent->StartWait() failed - status: %d", status);
    error_ = status;
    return;
  }

  ensure_error_set.cancel();
  ZX_ASSERT(error_ == ZX_OK);
  strong_child_vmo_ = std::move(strong_child_vmo);

  // ~attenuated_strong_parent_vmo here is fine, since strong_child_vmo counts as a child of
  // strong_parent_vmo_ for ZX_VMO_ZERO_CHILDREN purposes
}

bool LogicalBuffer::is_ok() { return error_ == ZX_OK; }

zx_status_t LogicalBuffer::error() {
  ZX_DEBUG_ASSERT(error_ != ZX_OK);
  return error_;
}

fit::result<zx_status_t, std::optional<zx::vmo>> LogicalBuffer::CreateWeakVmo(
    const ClientDebugInfo& client_debug_info) {
  if (!strong_parent_vmo_) {
    // success, but no VMO
    return fit::ok(std::nullopt);
  }
  ZX_DEBUG_ASSERT(strong_parent_vmo_->vmo().is_valid());

  // Now that we know there's at least one sysmem strong VMO outstanding for this LogicalBuffer, by
  // definition it's ok to create a weak VMO for this logical buffer. FWIW, this is the same
  // condition that we'll use to determine whether it's ok to convert a weak sysmem VMO to a strong
  // sysmem VMO, if that's added in future.

  // If strong_parent_vmo_ still exists, then we know parent_vmo_ also still exists.
  ZX_DEBUG_ASSERT(!!parent_vmo_);
  // We use buffer_collection_info_before_population_ here instead of allocation_result_info_
  // because allocation_result_info_ gets cleared out by FailDownFrom, whereas
  // buffer_collection_info_before_population_ remains set (as the latter has no VMO handles). This
  // way Allocator.GetVmoInfo with need_weak true can get a weak VMO from an existing VMO regardless
  // of whether before or after FailDownFrom. This way we avoid GetVmoInfo behavior varying
  // depending on ordering of events at the server. If FailDownFrom has already happened, the
  // close_weak_asap mechanism will handle that as usual.
  ZX_DEBUG_ASSERT(!!logical_buffer_collection_);
  ZX_DEBUG_ASSERT(
      logical_buffer_collection_->buffer_collection_info_before_population_.has_value());
  ZX_DEBUG_ASSERT(logical_buffer_collection_->buffer_collection_info_before_population_->settings()
                      .has_value());
  ZX_DEBUG_ASSERT(logical_buffer_collection_->buffer_collection_info_before_population_->settings()
                      ->buffer_settings()
                      .has_value());
  ZX_DEBUG_ASSERT(logical_buffer_collection_->buffer_collection_info_before_population_->settings()
                      ->buffer_settings()
                      ->raw_vmo_size()
                      .has_value());
  size_t raw_size_bytes =
      *logical_buffer_collection_->buffer_collection_info_before_population_->settings()
           ->buffer_settings()
           ->raw_vmo_size();
  ZX_DEBUG_ASSERT(raw_size_bytes % zx_system_get_page_size() == 0);
  zx::vmo per_sent_weak_parent;
  zx_status_t status =
      parent_vmo_->vmo().create_child(ZX_VMO_CHILD_SLICE, 0, raw_size_bytes, &per_sent_weak_parent);
  if (status != ZX_OK) {
    logical_buffer_collection_->LogError(FROM_HERE, "create_child() failed: %d", status);
    return fit::error(status);
  }

  // The child weak VMO may not actually be sent by the caller; in that case just cleans up without
  // ever being sent.
  auto tracked_sent_weak_parent_vmo = std::make_unique<TrackedParentVmo>(
      fbl::RefPtr(logical_buffer_collection_), std::move(per_sent_weak_parent), buffer_index_,
      [this](TrackedParentVmo* tracked_sent_weak_parent_vmo) mutable {
        if (tracked_sent_weak_parent_vmo->child_koid().has_value()) {
          logical_buffer_collection_->parent_sysmem_->RemoveVmoKoid(
              *tracked_sent_weak_parent_vmo->child_koid());
        }
        // This erase can fail if ~tracked_sent_weak_parent_vmo in an error path before added to
        // weak_parent_vmos_.
        weak_parent_vmos_.erase(tracked_sent_weak_parent_vmo);
      });

  // Makes a copy since TrackedParentVmo can outlast Node.
  tracked_sent_weak_parent_vmo->set_client_debug_info(client_debug_info);

  zx::vmo child_same_rights;
  status = tracked_sent_weak_parent_vmo->vmo().create_child(ZX_VMO_CHILD_SLICE, 0, raw_size_bytes,
                                                            &child_same_rights);
  if (status != ZX_OK) {
    logical_buffer_collection_->LogError(FROM_HERE, "create_child() (2) failed: %d", status);
    return fit::error(status);
  }

  // The caller will dup the handle to attenuate handle rights, but that won't change the koid of
  // the VMO that gets handed out, so stash that here.
  zx_info_handle_basic_t child_info{};
  status = child_same_rights.get_info(ZX_INFO_HANDLE_BASIC, &child_info, sizeof(child_info),
                                      nullptr, nullptr);
  if (status != ZX_OK) {
    logical_buffer_collection_->LogError(FROM_HERE, "vmo.get_info() failed: %d", status);
    return fit::error(status);
  }

  tracked_sent_weak_parent_vmo->set_child_koid(child_info.koid);
  logical_buffer_collection_->parent_sysmem_->AddVmoKoid(child_info.koid, true, *this);

  // Now that there's a child VMO of tracked_sent_weak_parent_vmo, we can StartWait().
  status = tracked_sent_weak_parent_vmo->StartWait(
      logical_buffer_collection_->parent_sysmem_->loop_dispatcher());
  if (status != ZX_OK) {
    logical_buffer_collection_->LogError(FROM_HERE, "StartWait failed: %d", status);
    return fit::error(status);
  }

  auto emplace_result = weak_parent_vmos_.try_emplace(tracked_sent_weak_parent_vmo.get(),
                                                      std::move(tracked_sent_weak_parent_vmo));
  ZX_ASSERT(emplace_result.second);

  return fit::ok(std::move(child_same_rights));
}

zx::vmo LogicalBuffer::TakeStrongChildVmo() {
  ZX_DEBUG_ASSERT(strong_child_vmo_.is_valid());
  auto local_vmo = std::move(strong_child_vmo_);
  ZX_DEBUG_ASSERT(!strong_child_vmo_.is_valid());
  return local_vmo;
}

LogicalBufferCollection& LogicalBuffer::logical_buffer_collection() {
  return *logical_buffer_collection_;
}

uint32_t LogicalBuffer::buffer_index() { return buffer_index_; }

}  // namespace sysmem_service
