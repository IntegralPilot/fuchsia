// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/examples/screen_recording/view_provider.h"

#include <fidl/fuchsia.ui.app/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition.internal/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>
#include <lib/ui/scenic/cpp/view_creation_tokens.h>
#include <lib/ui/scenic/cpp/view_identity.h>
#include <zircon/status.h>

#include <cstdint>

#include <fbl/algorithm.h>

#include "src/ui/examples/screen_recording/screen_capture_helper.h"
#include "src/ui/scenic/lib/utils/helpers.h"

namespace screen_recording_example {

using fuchsia_ui_composition::RegisterBufferCollectionUsages;
using fuchsia_ui_composition_internal::FrameInfo;
using fuchsia_ui_composition_internal::ScreenCapture;
using fuchsia_ui_composition_internal::ScreenCaptureConfig;
using fuchsia_ui_composition_internal::ScreenCaptureError;

ViewProviderImpl::ViewProviderImpl(component::OutgoingDirectory& outgoing,
                                   async_dispatcher_t* dispatcher)
    : dispatcher_(dispatcher) {
  zx::result result = outgoing.AddUnmanagedProtocol<fuchsia_ui_app::ViewProvider>(
      bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure));
  FX_CHECK(result.is_ok()) << "Failed to add ViewProvider protocol: " << result.status_string();
}

void ViewProviderImpl::CreateView2(CreateView2Request& request,
                                   CreateView2Completer::Sync& completer) {
  auto sysmem_allocator_connect = component::Connect<fuchsia_sysmem2::Allocator>();
  FX_CHECK(sysmem_allocator_connect.is_ok());
  sysmem_allocator_.Bind(std::move(sysmem_allocator_connect.value()), dispatcher_);

  auto flatland_allocator_connect = component::Connect<fuchsia_ui_composition::Allocator>();
  FX_CHECK(flatland_allocator_connect.is_ok());
  flatland_allocator_.Bind(std::move(flatland_allocator_connect.value()));

  // Set ContentId to be 2 above ContentIds used from num_buffers_ and 1 above kFilledRectId.
  kSquareRectId = ContentId(num_buffers_ + 2);

  auto flatland_connect = component::Connect<fuchsia_ui_composition::Flatland>();
  FX_CHECK(flatland_connect.is_ok());
  flatland_connection_ = simple_present::FlatlandConnection::Create(
      dispatcher_, std::move(flatland_connect.value()), "ScreenRecordingExample");
  flatland_connection_->SetErrorCallback([] { FX_LOGS(ERROR) << "Lost connection to Flatland"; });

  auto [parent_watcher_client, parent_watcher_server] =
      fidl::Endpoints<fuchsia_ui_composition::ParentViewportWatcher>::Create();
  parent_watcher_ = fidl::Client(std::move(parent_watcher_client), dispatcher_);

  // Create ScreenCapture client.
  auto screen_capture_connect =
      component::Connect<fuchsia_ui_composition_internal::ScreenCapture>();
  FX_CHECK(screen_capture_connect.is_ok());
  screen_capture_ = fidl::Client(std::move(screen_capture_connect.value()), dispatcher_);

  auto view_identity = scenic::cpp::NewViewIdentityOnCreation();
  FX_CHECK(request.args().view_creation_token().has_value());
  fuchsia_ui_views::ViewCreationToken view_token =
      std::move(request.args().view_creation_token().value());

  fuchsia_ui_composition::FlatlandCreateView2Request create_view_req;
  create_view_req.token(std::move(view_token));
  create_view_req.view_identity(std::move(view_identity));
  create_view_req.protocols({});
  create_view_req.parent_viewport_watcher(std::move(parent_watcher_server));

  (void)flatland()->CreateView2(std::move(create_view_req));

  parent_watcher_->GetLayout().Then(
      [this](fidl::Result<fuchsia_ui_composition::ParentViewportWatcher::GetLayout>& result) {
        if (result.is_error()) {
          FX_LOGS(ERROR) << "Error from ParentViewportWatcher::GetLayout: "
                         << result.error_value().FormatDescription();
          return;
        }
        const auto& layout_info = result.value().info();
        // ParentViewportWatcher API doesn't guarantee this in the docs, but in practice the first
        // result will always have logical_size.
        FX_CHECK(layout_info.logical_size().has_value());
        display_width_ = layout_info.logical_size()->width();
        display_height_ = layout_info.logical_size()->height();
        half_display_width_ = display_width_ / 2;
        num_pixels_ = display_width_ * display_height_;

        SetUpFlatland();

        // Create buffer collection to render into for GetNextFrame() and to duplicate for
        // creating images.
        allocation::cpp::BufferCollectionImportExportTokens scr_ref_pair =
            allocation::cpp::BufferCollectionImportExportTokens::New();

        fuchsia_ui_composition::RegisterBufferCollectionUsages usage_types =
            fuchsia_ui_composition::RegisterBufferCollectionUsages::kDefault |
            fuchsia_ui_composition::RegisterBufferCollectionUsages::kScreenshot;

        AllocateBufferCollection(
            CreateDefaultConstraints(num_buffers_, half_display_width_, display_height_),
            std::move(scr_ref_pair.export_token), flatland_allocator_, sysmem_allocator_,
            usage_types);

        // Initialize images with ContentId of their buffer index + 1.
        for (uint32_t i = 0; i < num_buffers_; i++) {
          fuchsia_ui_composition::BufferCollectionImportToken import_token_copy =
              scr_ref_pair.DuplicateImportToken();
          fuchsia_ui_composition::ImageProperties image_properties;
          image_properties.size(fuchsia_math::SizeU(half_display_width_, display_height_));

          fuchsia_ui_composition::FlatlandCreateImageRequest create_image_req;
          create_image_req.image_id(ContentId(i + 1));
          create_image_req.import_token(std::move(import_token_copy));
          create_image_req.vmo_index(i);
          create_image_req.properties(std::move(image_properties));
          (void)flatland()->CreateImage(std::move(create_image_req));
          (void)flatland()->SetImageBlendingFunction(
              {ContentId(i + 1), fuchsia_ui_composition::BlendMode::kSrc});
        }

        ScreenCaptureConfig sc_args;
        sc_args.import_token(std::move(scr_ref_pair.import_token));
        sc_args.image_size(fuchsia_math::SizeU(half_display_width_, display_height_));

        screen_capture_->Configure(std::move(sc_args))
            .Then([this](fidl::Result<fuchsia_ui_composition_internal::ScreenCapture::Configure>&
                             result) {
              if (result.is_ok()) {
                present_release_fences_.resize(num_buffers_);
                ScreenCaptureCallback();
              } else {
                FX_LOGS(ERROR) << "ScreenCapture::Configure failed: "
                               << result.error_value().FormatDescription();
              }
            });
        PresentCallback();
      });

  fuchsia_ui_composition::PresentArgs present_args;
  present_args.release_fences({});
  flatland_connection_->Present(std::move(present_args), [](auto) {});
}

void ViewProviderImpl::PresentCallback() {
  TRACE_DURATION("gfx", "Example::PresentCallback");
  DrawSquare();
  fuchsia_ui_composition::PresentArgs present_args;
  present_args.release_fences({});
  flatland_connection_->Present(std::move(present_args), [this](auto) { PresentCallback(); });
}

void ViewProviderImpl::ScreenCaptureCallback() {
  TRACE_DURATION("gfx", "Example::ScreenCaptureCallback");
  screen_capture_->GetNextFrame().Then(
      [this](fidl::Result<fuchsia_ui_composition_internal::ScreenCapture::GetNextFrame>& result) {
        if (!result.is_ok()) {
          FX_LOGS(ERROR) << "ScreenCapture::GetNextFrame failed: "
                         << result.error_value().FormatDescription();
          return;
        }
        auto& frame_info = result.value();
        FX_CHECK(frame_info.buffer_index().has_value());
        FX_CHECK(frame_info.buffer_release_token().has_value());
        uint64_t buffer_index = frame_info.buffer_index().value();
        TRACE_DURATION("gfx", "GetNextFrameCallback", "buffer_index", buffer_index);
        FX_CHECK(buffer_index < num_buffers_);

        (void)flatland()->SetContent({kChildTransformId2, ContentId(buffer_index + 1)});

        // Set up event to drop buffer when frame is presented.
        zx::event release_fence;
        zx::event::create(0, &release_fence);
        present_release_fences_[buffer_index] = utils::CopyZxHandle(release_fence);

        std::vector<zx::event> current_release_fences;
        current_release_fences.push_back(std::move(release_fence));
        fuchsia_ui_composition::PresentArgs present_args;
        present_args.release_fences(std::move(current_release_fences));
        present_args.unsquashable(true);

        auto wait = std::make_shared<async::WaitOnce>(present_release_fences_[buffer_index].get(),
                                                      ZX_EVENT_SIGNALED);
        zx::eventpair buffer_release_token = std::move(frame_info.buffer_release_token().value());
        zx_status_t status = wait->Begin(
            dispatcher_, [copy_ref = wait, token = std::move(buffer_release_token), buffer_index](
                             async_dispatcher_t*, async::WaitOnce*, zx_status_t status,
                             const zx_packet_signal_t* signal) mutable {
              TRACE_DURATION("gfx", "ScreenCapture Frame Released", "buffer_index", buffer_index);
              FX_DCHECK(status == ZX_OK);
              // Drop token.
              return;
            });
        FX_DCHECK(status == ZX_OK);

        flatland_connection_->Present(std::move(present_args), [](auto) {});
        ScreenCaptureCallback();
      });
}

void ViewProviderImpl::SetUpFlatland() {
  (void)flatland()->CreateTransform({kRootTransformId});
  (void)flatland()->CreateTransform({kChildTransformId1});
  (void)flatland()->CreateTransform({kChildTransformId2});
  (void)flatland()->CreateTransform({kBouncingSquareTransformId});

  (void)flatland()->SetTranslation({kChildTransformId1, {0, 0}});
  (void)flatland()->SetTranslation(
      {kChildTransformId2, {static_cast<int32_t>(half_display_width_), 0}});
  (void)flatland()->SetTranslation({kBouncingSquareTransformId, {bs_.x, bs_.y}});

  // Set up children of root transform.
  (void)flatland()->SetRootTransform({kRootTransformId});
  (void)flatland()->AddChild({kRootTransformId, kChildTransformId1});
  (void)flatland()->AddChild({kRootTransformId, kChildTransformId2});
  (void)flatland()->AddChild({kChildTransformId1, kBouncingSquareTransformId});

  // Set background of left section. Id is 1 above previously set for ScreenRecordingImages.
  const ContentId kFilledRectId{num_buffers_ + 1};
  (void)flatland()->CreateFilledRect({kFilledRectId});
  (void)flatland()->SetImageBlendingFunction(
      {kFilledRectId, fuchsia_ui_composition::BlendMode::kSrc});
  (void)flatland()->SetSolidFill({kFilledRectId, fuchsia_ui_composition::ColorRgba(0, 0, 0, 0),
                                  fuchsia_math::SizeU(half_display_width_, display_height_)});

  // Draw the bouncing square initially.
  (void)flatland()->CreateFilledRect({kSquareRectId});
  (void)flatland()->SetImageBlendingFunction(
      {kSquareRectId, fuchsia_ui_composition::BlendMode::kSrc});
  (void)flatland()->SetSolidFill({kSquareRectId, fuchsia_ui_composition::ColorRgba(1, 1, 0, 1),
                                  fuchsia_math::SizeU(bs_.size.width(), bs_.size.height())});

  (void)flatland()->SetContent({kChildTransformId1, kFilledRectId});
  (void)flatland()->SetContent({kBouncingSquareTransformId, kSquareRectId});
}

void ViewProviderImpl::DrawSquare() {
  bs_.x += bs_.x_speed;
  bs_.y += bs_.y_speed;

  (void)flatland()->SetTranslation({kBouncingSquareTransformId, {bs_.x, bs_.y}});

  CheckHit();
}

void ViewProviderImpl::CheckHit() {
  if (bs_.x + static_cast<int32_t>(bs_.size.width()) >= static_cast<int32_t>(half_display_width_) ||
      bs_.x <= 0) {
    bs_.x_speed *= -1;
    (void)flatland()->SetSolidFill(
        {kSquareRectId, RandomColor(), fuchsia_math::SizeU(bs_.size.width(), bs_.size.height())});
  }

  if (bs_.y + static_cast<int32_t>(bs_.size.height()) >= static_cast<int32_t>(display_height_) ||
      bs_.y <= 0) {
    bs_.y_speed *= -1;
    (void)flatland()->SetSolidFill(
        {kSquareRectId, RandomColor(), fuchsia_math::SizeU(bs_.size.width(), bs_.size.height())});
  }
}

fuchsia_ui_composition::ColorRgba ViewProviderImpl::RandomColor() {
  float r = static_cast<float>(rand()) / static_cast<float>(RAND_MAX);
  float g = static_cast<float>(rand()) / static_cast<float>(RAND_MAX);
  float b = static_cast<float>(rand()) / static_cast<float>(RAND_MAX);
  float a = static_cast<float>(rand()) / static_cast<float>(RAND_MAX);
  return fuchsia_ui_composition::ColorRgba(r, g, b, a);
}

}  // namespace screen_recording_example
