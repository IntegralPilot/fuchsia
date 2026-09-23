// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/examples/screen_recording/screen_capture_helper.h"

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/utils/helpers.h"

namespace screen_recording_example {

using fuchsia_ui_composition::RegisterBufferCollectionArgs;
using fuchsia_ui_composition::RegisterBufferCollectionUsages;

fuchsia_sysmem2::BufferCollectionConstraints CreateDefaultConstraints(
    uint32_t buffer_count, uint32_t width, uint32_t height, fuchsia_images2::PixelFormat format) {
  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  constraints.min_buffer_count(buffer_count);
  fuchsia_sysmem2::BufferUsage usage;
  usage.none(fuchsia_sysmem2::kNoneUsage);
  constraints.usage(std::move(usage));

  fuchsia_sysmem2::BufferMemoryConstraints mem_constraints;
  mem_constraints.ram_domain_supported(true);
  mem_constraints.cpu_domain_supported(true);
  constraints.buffer_memory_constraints(std::move(mem_constraints));

  fuchsia_sysmem2::ImageFormatConstraints image_constraints;
  image_constraints.pixel_format(format);
  image_constraints.pixel_format_modifier(fuchsia_images2::PixelFormatModifier::kLinear);
  image_constraints.color_spaces({{fuchsia_images2::ColorSpace::kSrgb}});
  image_constraints.min_size(fuchsia_math::SizeU(width, height));
  image_constraints.max_size(fuchsia_math::SizeU(width, height));
  constraints.image_format_constraints({{std::move(image_constraints)}});
  return constraints;
}

void AllocateBufferCollection(
    fuchsia_sysmem2::BufferCollectionConstraints constraints,
    fuchsia_ui_composition::BufferCollectionExportToken export_token,
    fidl::SyncClient<fuchsia_ui_composition::Allocator>& flatland_allocator,
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    RegisterBufferCollectionUsages usage) {
  FX_DCHECK(flatland_allocator.is_valid());
  FX_DCHECK(sysmem_allocator.is_valid());

  // Create Sysmem tokens.
  auto [local_token, dup_token] = utils::SysmemTokens::Create(sysmem_allocator);

  auto [collection_client, collection_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::SyncClient buffer_collection(std::move(collection_client));

  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(local_token))
          .buffer_collection_request(std::move(collection_server))
          .Build());
  FX_DCHECK(result.ok());

  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest constraints_request;
  constraints_request.constraints(std::move(constraints));
  auto set_res = buffer_collection->SetConstraints(std::move(constraints_request));
  FX_CHECK(set_res.is_ok());

  RegisterBufferCollectionArgs rbc_args;
  rbc_args.export_token(std::move(export_token));
  rbc_args.buffer_collection_token2(std::move(dup_token));
  rbc_args.usages(usage);
  fuchsia_ui_composition::AllocatorRegisterBufferCollectionRequest reg_req;
  reg_req.args(std::move(rbc_args));
  auto register_result = flatland_allocator->RegisterBufferCollection(std::move(reg_req));
  FX_CHECK(register_result.is_ok());

  // Wait for allocation.
  auto wait_result = buffer_collection->WaitForAllBuffersAllocated();
  FX_CHECK(wait_result.is_ok());

  auto rel_res = buffer_collection->Release();
  FX_CHECK(rel_res.is_ok());
}

}  // namespace screen_recording_example
