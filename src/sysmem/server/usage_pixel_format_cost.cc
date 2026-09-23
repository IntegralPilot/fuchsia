// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "usage_pixel_format_cost.h"

#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/image-format/image_format.h>
#include <zircon/assert.h>

#include <list>
#include <map>

#include "macros.h"
#include "utils.h"

namespace sysmem_service {

namespace {

const double kDefaultCost = std::numeric_limits<double>::max();

// |a| to check
// |r| required bits
bool HasAllRequiredBits(uint32_t a, uint32_t r) { return (r & a) == r; }

// |a| to check
// |r| required bits
bool HasAllRequiredUsageBits(const fuchsia_sysmem2::BufferUsage& a,
                             const fuchsia_sysmem2::BufferUsage& r) {
  const uint32_t a_cpu = a.cpu().has_value() ? a.cpu().value() : 0;
  const uint32_t a_vulkan = a.vulkan().has_value() ? a.vulkan().value() : 0;
  const uint32_t a_display = a.display().has_value() ? a.display().value() : 0;
  const uint32_t a_video = a.video().has_value() ? a.video().value() : 0;
  const uint32_t r_cpu = r.cpu().has_value() ? r.cpu().value() : 0;
  const uint32_t r_vulkan = r.vulkan().has_value() ? r.vulkan().value() : 0;
  const uint32_t r_display = r.display().has_value() ? r.display().value() : 0;
  const uint32_t r_video = r.video().has_value() ? r.video().value() : 0;
  return HasAllRequiredBits(a_cpu, r_cpu) && HasAllRequiredBits(a_vulkan, r_vulkan) &&
         HasAllRequiredBits(a_display, r_display) && HasAllRequiredBits(a_video, r_video);
}

uint32_t SharedBitsCount(uint32_t a, uint32_t b) {
  uint32_t set_in_both = a & b;

  // TODO(dustingreen): Consider using popcount intrinsic (or equivalent).
  uint32_t count = 0;
  for (uint32_t i = 0; i < sizeof(uint32_t) * 8; ++i) {
    if (set_in_both & (1 << i)) {
      ++count;
    }
  }

  return count;
}

uint32_t SharedUsageBitsCount(const fuchsia_sysmem2::BufferUsage& a,
                              const fuchsia_sysmem2::BufferUsage& b) {
  const uint32_t a_cpu = a.cpu().has_value() ? a.cpu().value() : 0;
  const uint32_t a_vulkan = a.vulkan().has_value() ? a.vulkan().value() : 0;
  const uint32_t a_display = a.display().has_value() ? a.display().value() : 0;
  const uint32_t a_video = a.video().has_value() ? a.video().value() : 0;
  const uint32_t b_cpu = b.cpu().has_value() ? b.cpu().value() : 0;
  const uint32_t b_vulkan = b.vulkan().has_value() ? b.vulkan().value() : 0;
  const uint32_t b_display = b.display().has_value() ? b.display().value() : 0;
  const uint32_t b_video = b.video().has_value() ? b.video().value() : 0;
  return SharedBitsCount(a_cpu, b_cpu) + SharedBitsCount(a_vulkan, b_vulkan) +
         SharedBitsCount(a_display, b_display) + SharedBitsCount(a_video, b_video);
}

// This comparison has nothing to do with the cost of a or cost of b.  This is
// only about finding the best-match UsagePixelFormatCostEntry for the given
// query.
//
// |constraints| the query's constraints
//
// |image_format_constraints_index| the query's image_format_constraints_index
//
// |a| the new UsagePixelFormatCostEntry to consider
//
// |b| the existing UsagePixelFormatCostEntry that a is being compared against
bool IsBetterMatch(const fuchsia_sysmem2::BufferCollectionConstraints& constraints,
                   uint32_t image_format_constraints_index,
                   const fuchsia_sysmem2::FormatCostEntry* a,
                   const fuchsia_sysmem2::FormatCostEntry* b) {
  ZX_DEBUG_ASSERT(a);
  ZX_DEBUG_ASSERT(image_format_constraints_index < constraints.image_format_constraints()->size());
  // We intentionally allow b to be nullptr.

  auto& key = a->key();
  PixelFormatAndModifier a_pixel_format_and_modifier(
      *key->pixel_format(), key->pixel_format_modifier().has_value()
                                ? *key->pixel_format_modifier()
                                : fuchsia_images2::PixelFormatModifier::kLinear);
  if (!ImageFormatIsPixelFormatEqual(
          a_pixel_format_and_modifier,
          PixelFormatAndModifierFromConstraints(
              constraints.image_format_constraints()->at(image_format_constraints_index)))) {
    return false;
  }

  fuchsia_sysmem2::BufferUsage default_usage;
  const fuchsia_sysmem2::BufferUsage* usage_ptr;
  if (constraints.usage().has_value()) {
    usage_ptr = &constraints.usage().value();
  } else {
    usage_ptr = &default_usage;
  }
  const fuchsia_sysmem2::BufferUsage& usage = *usage_ptr;

  const fuchsia_sysmem2::BufferUsage* a_usage_ptr;
  if (a->key()->buffer_usage_bits().has_value()) {
    a_usage_ptr = &*a->key()->buffer_usage_bits();
  } else {
    a_usage_ptr = &default_usage;
  }
  const fuchsia_sysmem2::BufferUsage& a_usage = *a_usage_ptr;

  if (!HasAllRequiredUsageBits(usage, a_usage)) {
    return false;
  }
  // We intentionally allow b to be nullptr.
  if (b == nullptr) {
    return true;
  }

  const fuchsia_sysmem2::BufferUsage* b_usage_ptr;
  if (b->key()->buffer_usage_bits().has_value()) {
    b_usage_ptr = &*b->key()->buffer_usage_bits();
  } else {
    b_usage_ptr = &default_usage;
  }
  const fuchsia_sysmem2::BufferUsage& b_usage = *b_usage_ptr;

  ZX_DEBUG_ASSERT(HasAllRequiredUsageBits(usage, b_usage));
  uint32_t a_shared_bits = SharedUsageBitsCount(usage, a_usage);
  uint32_t b_shared_bits = SharedUsageBitsCount(usage, b_usage);
  return a_shared_bits >= b_shared_bits;
}

}  // namespace

UsagePixelFormatCost::UsagePixelFormatCost(std::vector<fuchsia_sysmem2::FormatCostEntry> entries)
    : entries_(std::move(entries)) {}

double UsagePixelFormatCost::GetCost(
    const fuchsia_sysmem2::BufferCollectionConstraints& constraints,
    uint32_t image_format_constraints_index) const {
  const fuchsia_sysmem2::FormatCostEntry* best_match = nullptr;
  for (auto& entry : entries_) {
    if (IsBetterMatch(constraints, image_format_constraints_index, &entry, best_match)) {
      best_match = &entry;
    }
  }
  if (!best_match) {
    return kDefaultCost;
  }
  return *best_match->cost();
}

int32_t UsagePixelFormatCost::Compare(
    const fuchsia_sysmem2::BufferCollectionConstraints& constraints,
    uint32_t image_format_constraints_index_a, uint32_t image_format_constraints_index_b) const {
  double cost_a = GetCost(constraints, image_format_constraints_index_a);
  double cost_b = GetCost(constraints, image_format_constraints_index_b);

  if (cost_a < cost_b) {
    return -1;
  } else if (cost_a > cost_b) {
    return 1;
  } else {
    return 0;
  }
}

}  // namespace sysmem_service
