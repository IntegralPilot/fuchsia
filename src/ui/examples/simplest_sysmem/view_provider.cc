// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/examples/simplest_sysmem/view_provider.h"

#include <fuchsia/sysmem2/cpp/fidl.h>
#include <fuchsia/ui/composition/cpp/fidl.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>
#include <zircon/rights.h>
#include <zircon/status.h>

#include <cstdint>

#include <fbl/algorithm.h>

#include "src/ui/examples/simplest_sysmem/png_helper.h"
#include "src/ui/examples/simplest_sysmem/sysmem_helper.h"

namespace sysmem_example {

ViewProviderImpl::ViewProviderImpl(sys::ComponentContext* component_context, RenderType type)
    : context_(component_context), render_type_(type) {
  context_->outgoing()->AddPublicService<fuchsia::ui::app::ViewProvider>(
      bindings_.GetHandler(this));
}

ViewProviderImpl::~ViewProviderImpl() {
  context_->outgoing()->RemovePublicService<fuchsia::ui::app::ViewProvider>();
}

void ViewProviderImpl::CreateView2(fuchsia::ui::app::CreateView2Args args) {
  flatland_ = context_->svc()->Connect<fuchsia::ui::composition::Flatland>();
  flatland_.set_error_handler([](zx_status_t status) {
    FX_LOGS(ERROR) << "Lost connection to Scenic: " << zx_status_get_string(status);
  });
  parent_watcher_.set_error_handler([](zx_status_t status) {
    FX_LOGS(ERROR) << "Error from fuchsia::ui::composition::ParentViewportWatcher: "
                   << zx_status_get_string(status);
  });

  auto view_identity = scenic::NewViewIdentityOnCreation();

  flatland_->CreateView2(std::move(*args.mutable_view_creation_token()), std::move(view_identity),
                         /* protocols = */ {}, parent_watcher_.NewRequest());

  flatland_->CreateTransform(fuchsia::ui::composition::TransformId({.value = kRootTransformId}));
  flatland_->SetRootTransform(fuchsia::ui::composition::TransformId({.value = kRootTransformId}));

  // NOTE: These examples only respond to the first GetLayout() calls. Any future
  // size change is ignored.
  if (render_type_ == sysmem_example::RenderType::RECTANGLE) {
    const fuchsia::ui::composition::ContentId kFilledRectId = {1};

    flatland_->CreateFilledRect(kFilledRectId);
    flatland_->SetSolidFill(kFilledRectId, {1, 0, 1, 1} /* The color fuchsia*/,
                            {200, 100} /* size */);
    // Here we attach view content directly to `kRootTransformId`.
    // See |WriteToSysmem| for a simple example of creating a child transform and attaching it to
    // the root.
    flatland_->SetTranslation(fuchsia::ui::composition::TransformId({.value = kRootTransformId}),
                              {0, 0});
    flatland_->SetContent(fuchsia::ui::composition::TransformId({.value = kRootTransformId}),
                          kFilledRectId);
  }

  if (render_type_ == sysmem_example::RenderType::COLOR_BLOCK) {
    uint32_t image_width = 256;
    uint32_t image_height = 256;
    uint32_t total_bytes = image_width * image_height * 4;
    uint8_t* image_bytes = new uint8_t[total_bytes];

    GenerateColorBlockImage(image_width, image_height, image_bytes);
    WriteToSysmem(image_bytes, image_width, image_height, fuchsia::images2::PixelFormat::B8G8R8A8);
  }

  if (render_type_ == sysmem_example::RenderType::PNG) {
    std::vector<uint8_t> write_values;
    png_helper::PNGImageSize image_size = {};

    uint8_t* image_bytes;
    png_helper::LoadPngFromFile(&image_size, &image_bytes);
    WriteToSysmem(image_bytes, image_size.width, image_size.height,
                  fuchsia::images2::PixelFormat::R8G8B8A8);
  }

  flatland_->Present(fuchsia::ui::composition::PresentArgs{});
}

void ViewProviderImpl::WriteToSysmem(uint8_t* write_values, uint32_t image_width,
                                     uint32_t image_height,
                                     fuchsia::images2::PixelFormat pixel_format) {
  context_->svc()->Connect(sysmem_allocator_.NewRequest());
  context_->svc()->Connect(flatland_allocator_.NewRequest());
  sysmem_helper::BufferCollectionImportExportTokens ref_pair =
      sysmem_helper::BufferCollectionImportExportTokens::New();

  fuchsia::sysmem2::BufferCollectionTokenSyncPtr local_token;
  fuchsia::sysmem2::BufferCollectionTokenSyncPtr dup_token;
  fuchsia::sysmem2::AllocatorAllocateSharedCollectionRequest allocate_request;
  allocate_request.set_token_request(local_token.NewRequest());
  zx_status_t status = sysmem_allocator_->AllocateSharedCollection(std::move(allocate_request));
  FX_CHECK(status == ZX_OK) << "Cannot allocate shared collection: "
                            << zx_status_get_string(status);

  fuchsia::sysmem2::BufferCollectionTokenDuplicateRequest dup_request;
  dup_request.set_rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS);
  dup_request.set_token_request(dup_token.NewRequest());
  status = local_token->Duplicate(std::move(dup_request));
  FX_CHECK(status == ZX_OK) << "Cannot duplicate token: " << zx_status_get_string(status);
  fuchsia::sysmem2::Node_Sync_Result sync_result;
  status = local_token->Sync(&sync_result);
  FX_CHECK(sync_result.is_response());

  fuchsia::ui::composition::RegisterBufferCollectionArgs args = {};
  args.set_export_token(std::move(ref_pair.export_token));

  // BufferCollectionToken zircon handles are interchangeable between fuchsia::sysmem2
  // and fuchsia::sysmem(1).
  args.set_buffer_collection_token2(std::move(dup_token).Unbind());
  args.set_usages(fuchsia::ui::composition::RegisterBufferCollectionUsages::DEFAULT);

  fuchsia::sysmem2::BufferCollectionSyncPtr buffer_collection;
  fuchsia::sysmem2::AllocatorBindSharedCollectionRequest bind_request;
  bind_request.set_token(std::move(local_token));
  bind_request.set_buffer_collection_request(buffer_collection.NewRequest());
  status = sysmem_allocator_->BindSharedCollection(std::move(bind_request));
  FX_CHECK(status == ZX_OK) << "Cannot bind shared collection: " << zx_status_get_string(status);

  const uint8_t kBytesPerPixel = 4;
  fuchsia::sysmem2::BufferCollectionSetConstraintsRequest constraints_request;
  constraints_request.set_constraints(
      sysmem_helper::CreateDefaultConstraints(sysmem_helper::BufferConstraint{
          .buffer_count = 1,
          .image_width = image_width,
          .image_height = image_height,
          .bytes_per_pixel = kBytesPerPixel,
          .pixel_format_type = pixel_format,
      }));
  buffer_collection->SetConstraints(std::move(constraints_request));

  fuchsia::ui::composition::Allocator_RegisterBufferCollection_Result result;
  flatland_allocator_->RegisterBufferCollection(std::move(args), &result);
  FX_CHECK(!result.is_err()) << "register buffer collection errored.";

  fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Result wait_result;
  status = buffer_collection->WaitForAllBuffersAllocated(&wait_result);
  FX_CHECK(wait_result.is_response());
  FX_CHECK(status == ZX_OK) << "WaitForBuffersAllocated failed with status: "
                            << zx_status_get_string(status);
  status = buffer_collection->Release();
  FX_CHECK(status == ZX_OK);

  uint32_t buffer_collection_idx = 0;
  const auto& buffer_collection_info = wait_result.response().buffer_collection_info();
  uint32_t bytes_per_row = fbl::round_up(
      std::max(image_width * kBytesPerPixel,
               buffer_collection_info.settings().image_format_constraints().min_bytes_per_row()),
      buffer_collection_info.settings().image_format_constraints().bytes_per_row_divisor());

  sysmem_helper::MapHostPointer(buffer_collection_info, buffer_collection_idx,
                                [write_values, bytes_per_row, image_width, image_height](
                                    uint8_t* vmo_host, uint32_t num_bytes) {
                                  uint32_t valid_bytes_per_row = image_width * kBytesPerPixel;
                                  FX_CHECK(num_bytes >= bytes_per_row * image_height);
                                  FX_CHECK(bytes_per_row >= valid_bytes_per_row);

                                  if (bytes_per_row == valid_bytes_per_row) {
                                    // When the memory buffer allocated is exactly the same size
                                    // as the image to draw. Note the
                                    // buffer_collection_info.settings.image_format_constraints.bytes_per_row_divisor
                                    // value, buffer row size must be divisible by
                                    // `image_format_constraints.bytes_per_row_divisor`, which isn't
                                    // always the same as the image's actual size (width * height *
                                    // kBytesPerPixel).
                                    uint32_t total_bytes = valid_bytes_per_row * image_height;
                                    memcpy(vmo_host, write_values, total_bytes);
                                  } else {
                                    // Copy over row-by-row.
                                    for (size_t i = 0; i < image_height; ++i) {
                                      memcpy(&vmo_host[i * bytes_per_row],
                                             write_values + (i * valid_bytes_per_row),
                                             valid_bytes_per_row);
                                    }
                                  }
                                });
  if (buffer_collection_info.settings().buffer_settings().coherency_domain() ==
      fuchsia::sysmem2::CoherencyDomain::RAM) {
    buffer_collection_info.buffers()
        .at(buffer_collection_idx)
        .vmo()
        .op_range(ZX_VMO_OP_CACHE_CLEAN, 0,
                  buffer_collection_info.settings().buffer_settings().size_bytes(), nullptr, 0);
  }

  // Call flatland to show image
  const fuchsia::ui::composition::ContentId kChildContentId = {1};
  const fuchsia::ui::composition::TransformId kChildTransformId = {2};

  fuchsia::ui::composition::ImageProperties image_properties = {};
  image_properties.set_size({image_width, image_height});
  flatland_->CreateImage(kChildContentId, std::move(ref_pair.import_token), buffer_collection_idx,
                         std::move(image_properties));
  flatland_->CreateTransform(kChildTransformId);
  flatland_->SetContent(kChildTransformId, kChildContentId);
  flatland_->SetImageDestinationSize(kChildContentId, {image_width, image_height});
  flatland_->SetTranslation(kChildTransformId, {0, 0} /*translation*/);
  flatland_->AddChild(fuchsia::ui::composition::TransformId{kRootTransformId}, kChildTransformId);
}

void ViewProviderImpl::GenerateColorBlockImage(uint32_t image_width, uint32_t image_height,
                                               uint8_t* write_values) {
  uint64_t i = 0;
  for (uint32_t row = 0; row < image_height; row++) {
    for (uint32_t col = 0; col < image_width; col++) {
      // Top-left
      if (row < image_height / 2 && col < image_width / 2) {
        write_values[i++] = kRed[0];
        write_values[i++] = kRed[1];
        write_values[i++] = kRed[2];
        write_values[i++] = kRed[3];
      }
      // Top-right
      else if (row < image_height / 2 && col >= image_width / 2) {
        write_values[i++] = kGreen[0];
        write_values[i++] = kGreen[1];
        write_values[i++] = kGreen[2];
        write_values[i++] = kGreen[3];
      }
      // Bottom-left
      else if (row >= image_height / 2 && col < image_width / 2) {
        write_values[i++] = kYellow[0];
        write_values[i++] = kYellow[1];
        write_values[i++] = kYellow[2];
        write_values[i++] = kYellow[3];
      }
      // Bottom-right
      else if (row >= image_height / 2 && col >= image_width / 2) {
        write_values[i++] = kBlue[0];
        write_values[i++] = kBlue[1];
        write_values[i++] = kBlue[2];
        write_values[i++] = kBlue[3];
      }
    }
  }
}
}  // namespace sysmem_example
