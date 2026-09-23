// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/testing/fake-buffer-collection.h"

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire_test_base.h>
#include <lib/image-format/image_format.h>
#include <zircon/assert.h>

#include <cstddef>
#include <cstdint>
#include <cinttypes>

#include <fbl/algorithm.h>
#include <gtest/gtest.h>

namespace intel_display {

FakeBufferCollection::FakeBufferCollection(const FakeBufferCollectionConfig& config)
    : config_(config) {}

FakeBufferCollection::~FakeBufferCollection() = default;

void FakeBufferCollection::SetConstraints(SetConstraintsRequestView request,
                                          SetConstraintsCompleter::Sync& _completer) {
  if (!request->has_constraints()) {
    return;
  }

  fuchsia_sysmem2::wire::BufferMemoryConstraints& buffer_memory_constraints =
      request->constraints().buffer_memory_constraints();
  if (!config_.cpu_domain_supported) {
    ZX_ASSERT(!buffer_memory_constraints.has_cpu_domain_supported() ||
              !buffer_memory_constraints.cpu_domain_supported());
  }
  if (!config_.ram_domain_supported) {
    ZX_ASSERT(!buffer_memory_constraints.has_ram_domain_supported() ||
              !buffer_memory_constraints.ram_domain_supported());
  }
  if (!config_.inaccessible_domain_supported) {
    ZX_ASSERT(!buffer_memory_constraints.has_inaccessible_domain_supported() ||
              !buffer_memory_constraints.inaccessible_domain_supported());
  }

  constraints_ = fidl::ToNatural(request->constraints());
}

void FakeBufferCollection::CheckAllBuffersAllocated(
    CheckAllBuffersAllocatedCompleter::Sync& completer) {
  completer.Reply(fit::ok());
}

void FakeBufferCollection::WaitForAllBuffersAllocated(
    WaitForAllBuffersAllocatedCompleter::Sync& completer) {
  ZX_ASSERT(HasConstraints());

  fidl::WireTableBuilder<fuchsia_sysmem2::wire::BufferCollectionInfo> info =
      fuchsia_sysmem2::wire::BufferCollectionInfo::Builder(arena_);

  std::vector<fuchsia_sysmem2::ImageFormatConstraints>& image_format_constraints_vector =
      *constraints_.image_format_constraints();
  auto image_format_constraints_it =
      std::find_if(image_format_constraints_vector.begin(), image_format_constraints_vector.end(),
                   [&](const fuchsia_sysmem2::ImageFormatConstraints& c) {
                     return c.pixel_format_modifier() == config_.format_modifier;
                   });
  ZX_ASSERT_MSG(image_format_constraints_it != image_format_constraints_vector.end(),
                "Failed to find image format constraints with `format_modifier` 0x%" PRIx64,
                static_cast<uint64_t>(config_.format_modifier));
  fuchsia_sysmem2::ImageFormatConstraints& image_format_constraints = *image_format_constraints_it;

  image_format_constraints.bytes_per_row_divisor(config_.bytes_per_row_divisor);
  info.settings(fuchsia_sysmem2::wire::SingleBufferSettings::Builder(arena_)
                    .image_format_constraints(fidl::ToWire(arena_, image_format_constraints))
                    .Build());

  int64_t width = config_.width_fallback_px;
  int64_t height = config_.height_fallback_px;
  const std::optional<fuchsia_math::SizeU>& required_min_size =
      image_format_constraints.required_min_size();
  if (required_min_size.has_value()) {
    width = required_min_size->width();
    height = required_min_size->height();
  }
  int64_t bytes_per_pixel = ImageFormatStrideBytesPerWidthPixel(
      PixelFormatAndModifierFromConstraints(image_format_constraints));
  int64_t bytes_per_row = fbl::round_up(static_cast<size_t>(width * bytes_per_pixel),
                                        static_cast<uint32_t>(config_.bytes_per_row_divisor));
  int64_t bytes_per_image = bytes_per_row * height;

  zx::vmo vmo;
  zx_status_t create_status = zx::vmo::create(bytes_per_image, 0, &vmo);
  ZX_ASSERT(create_status == ZX_OK);
  info.buffers(std::vector{fuchsia_sysmem2::wire::VmoBuffer::Builder(arena_)
                               .vmo(std::move(vmo))
                               .vmo_usable_start(0)
                               .Build()});
  auto response =
      fuchsia_sysmem2::wire::BufferCollectionWaitForAllBuffersAllocatedResponse::Builder(arena_)
          .buffer_collection_info(info.Build())
          .Build();
  completer.Reply(fit::ok(&response));
}

void FakeBufferCollection::NotImplemented_(const std::string& name,
                                           fidl::CompleterBase& completer) {
  ZX_PANIC("Not implemented: %s", name.c_str());
}

}  // namespace intel_display
