// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/screenshot/flatland_screenshot.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <optional>
#include <string>

#include "src/ui/scenic/lib/allocation/allocator.h"
#include "src/ui/scenic/lib/flatland/buffers/util.h"
#include "src/ui/scenic/lib/image-compression/image_compression.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "zircon/system/ulib/fbl/include/fbl/algorithm.h"

using allocation::Allocator;
using fuchsia_ui_composition::GetNextFrameArgs;
using fuchsia_ui_composition::ScreenCaptureConfig;
using fuchsia_ui_composition::ScreenshotFormat;
using fuchsia_ui_composition::ScreenshotTakeFileResponse;
using fuchsia_ui_composition::ScreenshotTakeRequest;
using fuchsia_ui_composition::ScreenshotTakeResponse;
using screen_capture::ScreenCapture;

namespace {

constexpr uint32_t kBufferIndex = 0;
constexpr auto kBytesPerPixel = 4;

fuchsia_images2::PixelFormat CompositionToImages2Format(ScreenshotFormat format) {
  switch (format) {
    case ScreenshotFormat::kBgraRaw:
      return fuchsia_images2::PixelFormat::kB8G8R8A8;
    case ScreenshotFormat::kRgbaRaw:
      return fuchsia_images2::PixelFormat::kR8G8B8A8;
    default:
      return fuchsia_images2::PixelFormat::kInvalid;
  }
}

}  // namespace

namespace screenshot {

FlatlandScreenshot::FlatlandScreenshot(
    sys::ComponentContext* app_context, async_dispatcher_t* dispatcher,
    std::unique_ptr<ScreenCapture> screen_capturer, std::shared_ptr<Allocator> allocator,
    fuchsia_math::SizeU display_size, int display_rotation,
    fit::function<void(FlatlandScreenshot*)> destroy_instance_function)
    : app_context_(app_context),
      dispatcher_(dispatcher),
      screen_capturer_(std::move(screen_capturer)),
      flatland_allocator_(allocator),
      display_size_(display_size),
      display_rotation_(display_rotation),
      destroy_instance_function_(std::move(destroy_instance_function)),
      weak_factory_(this) {
  sysmem_allocator_ = utils::CreateSysmemAllocatorClientWithSvc(app_context->svc().get(),
                                                                dispatcher, "FlatlandScreenshot");

  FX_DCHECK(screen_capturer_);
  FX_DCHECK(flatland_allocator_);
  FX_DCHECK(sysmem_allocator_.is_valid());
  FX_DCHECK(display_size_.width());
  FX_DCHECK(display_size_.height());
  FX_DCHECK(destroy_instance_function_);

  // Create event and wait for initialization purposes.
  zx_status_t status = zx::event::create(0, &init_event_);
  FX_DCHECK(status == ZX_OK);
  init_wait_ = std::make_shared<async::WaitOnce>(init_event_.get(), ZX_EVENT_SIGNALED);

  if (display_rotation_ == 90 || display_rotation_ == 270) {
    std::swap(display_size_.width(), display_size_.height());
  }
}

void FlatlandScreenshot::AllocateBuffers() {
  // Do all sysmem initialization up front.
  allocation::cpp::BufferCollectionImportExportTokens ref_pair =
      allocation::cpp::BufferCollectionImportExportTokens::New();

  // Create sysmem tokens.
  auto [local_token_client, local_token_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator_->AllocateSharedCollection(
      fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
          .token_request(std::move(local_token_server))
          .Build());
  FX_DCHECK(result.ok());

  auto [dup_token_client, dup_token_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  result =
      fidl::WireCall(local_token_client)
          ->Duplicate(fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateRequest::Builder(arena)
                          .rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS)
                          .token_request(std::move(dup_token_server))
                          .Build());
  FX_DCHECK(result.ok());
  auto sync_result = fidl::WireCall(local_token_client)->Sync();
  FX_DCHECK(sync_result.ok());

  auto [bc_client, bc_server] = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  result = sysmem_allocator_->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(local_token_client))
          .buffer_collection_request(std::move(bc_server))
          .Build());
  FX_DCHECK(result.ok());

  auto buffer_collection = std::make_shared<fidl::Client<fuchsia_sysmem2::BufferCollection>>(
      std::move(bc_client), dispatcher_);

  // We only need 1 buffer since it gets reused on every Take() call.
  // Inform sysmem of the exact dimensions to prevent wasted memory from sysmem padding for
  // lavapipe block size.
  constexpr bool kSetMinMaxSize = true;
  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  set_constraints_request.constraints(utils::CreateDefaultConstraints(
      /*buffer_count=*/1, display_size_.width(), display_size_.height(),
      CompositionToImages2Format(raw_format_), kSetMinMaxSize));
  auto set_constraints_status =
      (*buffer_collection)->SetConstraints(std::move(set_constraints_request));
  FX_DCHECK(set_constraints_status.is_ok());

  fuchsia_sysmem2::NodeSetNameRequest set_name_request;
  set_name_request.priority(11u);
  set_name_request.name("FlatlandScreenshotMemory");
  auto set_name_status = (*buffer_collection)->SetName(std::move(set_name_request));
  FX_DCHECK(set_name_status.is_ok());

  // Initialize Flatland allocator state.
  fuchsia_ui_composition::RegisterBufferCollectionArgs args;
  args.export_token(std::move(ref_pair.export_token));
  args.buffer_collection_token2(std::move(dup_token_client));
  args.usages(fuchsia_ui_composition::RegisterBufferCollectionUsages::kScreenshot);

  flatland_allocator_->RegisterBufferCollection(std::move(args),
                                                [](auto result) { FX_DCHECK(!result.is_error()); });

  ScreenCaptureConfig sc_args;
  sc_args.import_token(std::move(ref_pair.import_token));
  sc_args.buffer_count(1);
  sc_args.size(display_size_);

  switch (display_rotation_) {
    case 0:
      sc_args.rotation(fuchsia_ui_composition::Rotation::kCw0Degrees);
      break;
    // If the display in rotated by 90 degrees, we need to apply a clockwise rotation of 270 degrees
    // in order to cancel the overall rotation and render the correct screenshot.
    case 90:
      sc_args.rotation(fuchsia_ui_composition::Rotation::kCw270Degrees);
      break;
    case 180:
      sc_args.rotation(fuchsia_ui_composition::Rotation::kCw180Degrees);
      break;
      // If the display in rotated by 270 degrees, we need to apply a clockwise rotation of 90
      // degrees in order to cancel the overall rotation and render the correct screenshot.
    case 270:
      sc_args.rotation(fuchsia_ui_composition::Rotation::kCw90Degrees);
      break;
    default:
      FX_LOGS(ERROR) << "Invalid display rotation value: " << display_rotation_;
  }

  (*buffer_collection)
      ->WaitForAllBuffersAllocated()
      .Then([weak_ptr = weak_factory_.GetWeakPtr(), buffer_collection,
             sc_args = std::move(sc_args)](auto& wait_result) mutable {
        if (!weak_ptr) {
          return;
        }
        FX_DCHECK(wait_result.is_ok());
        weak_ptr->buffer_collection_info_.insert(
            {weak_ptr->raw_format_,
             std::move(wait_result.value().buffer_collection_info().value())});
        auto release_status = (*buffer_collection)->Release();
        FX_DCHECK(release_status.is_ok());

        weak_ptr->screen_capturer_->Configure(std::move(sc_args),
                                              [weak_ptr = std::move(weak_ptr)](auto result) {
                                                FX_DCHECK(!result.is_error());
                                                if (!weak_ptr) {
                                                  return;
                                                }
                                                weak_ptr->init_event_.signal(0u, ZX_EVENT_SIGNALED);
                                              });
      });
}

void FlatlandScreenshot::Take(TakeRequest& request, TakeCompleter::Sync& completer) {
  Take(std::move(request), [completer = completer.ToAsync()](auto response) mutable {
    completer.Reply(std::move(response));
  });
}

void FlatlandScreenshot::Take(fuchsia_ui_composition::ScreenshotTakeRequest params,
                              fit::function<void(ScreenshotTakeResponse)> callback) {
  // Check if there is already a Take() call pending. Either the setup is done (|init_wait_| is
  // signaled) or the setup is still in progress.
  //
  // If the setup is done, then a Take() call would set |take_callback_|.
  // If the setup is not done, then a Take() call would make |init_wait| pending.
  if (take_callback_ != nullptr || init_wait_->is_pending()) {
    FX_LOGS(ERROR) << "Screenshot::Take() already in progress, closing connection. Wait for return "
                      "before calling again.";
    destroy_instance_function_(this);
    return;
  }

  ScreenshotFormat format = raw_format_;
  if (params.format().has_value()) {
    format = params.format().value();
    if (format != ScreenshotFormat::kPng) {
      raw_format_ = format;
    }
  }
  if (!buffer_collection_info_.contains(raw_format_)) {
    AllocateBuffers();
  }

  if (!utils::IsEventSignalled(init_event_, ZX_EVENT_SIGNALED)) {
    // Begin to asynchronously wait on the event. Retry the Take() call when the initialization is
    // complete.
    zx_status_t status = init_wait_->Begin(
        async_get_default_dispatcher(),
        [weak_ptr = weak_factory_.GetWeakPtr(), params = std::move(params),
         callback = std::move(callback)](async_dispatcher_t*, async::WaitOnce*, zx_status_t status,
                                         const zx_packet_signal_t* signal) mutable {
          if (!weak_ptr) {
            return;
          }
          FX_DCHECK(status == ZX_OK || status == ZX_ERR_CANCELED);

          // Retry the Take() call.
          weak_ptr->Take(std::move(params), std::move(callback));
        });
    FX_DCHECK(status == ZX_OK);
    return;
  }

  take_callback_ = std::move(callback);

  GetNextFrame();

  // Wait for the frame to render in an async fashion.
  render_wait_ = std::make_shared<async::WaitOnce>(render_event_.get(), ZX_EVENT_SIGNALED);
  zx_status_t status = render_wait_->Begin(
      async_get_default_dispatcher(), [this, weak_ptr = weak_factory_.GetWeakPtr(), format](
                                          async_dispatcher_t*, async::WaitOnce*, zx_status_t status,
                                          const zx_packet_signal_t*) mutable {
        FX_DCHECK(status == ZX_OK || status == ZX_ERR_CANCELED);
        if (!weak_ptr) {
          return;
        }
        FX_DCHECK(take_callback_);
        zx::vmo raw_vmo = weak_ptr->HandleFrameRender();

        if (format == ScreenshotFormat::kPng) {
          MaybeConnectToImageCompressor();
          zx::vmo response_vmo;
          zx::vmo response_vmo_copy;
          const auto response_vmo_size =
              (static_cast<uint64_t>(display_size_.width()) *
               static_cast<uint64_t>(display_size_.height()) * kBytesPerPixel) +
              zx_system_get_page_size();
          FX_CHECK(zx::vmo::create(response_vmo_size, ZX_VMO_RESIZABLE, &response_vmo) == ZX_OK);
          FX_CHECK(response_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &response_vmo_copy) == ZX_OK);

          fuchsia_ui_compression_internal::ImageCompressorEncodePngRequest request;
          request.raw_vmo() = std::move(raw_vmo);
          request.image_dimensions(display_size_);
          request.png_vmo() = std::move(response_vmo);

          client_->EncodePng(std::move(request))
              .ThenExactlyOnce(
                  [weak_ptr, vmo = std::move(response_vmo_copy)](
                      fidl::Result<fuchsia_ui_compression_internal::ImageCompressor::EncodePng>
                          result) mutable {
                    if (!weak_ptr) {
                      return;
                    }
                    FX_DCHECK(weak_ptr->take_callback_);
                    if (result.is_error()) {
                      FX_LOGS(ERROR) << result.error_value().FormatDescription();
                      weak_ptr->render_event_.reset();
                      // Release the buffer to allow for subsequent screenshots.
                      weak_ptr->screen_capturer_->ReleaseFrame(kBufferIndex, [](auto result) {});
                      return;
                    }
                    weak_ptr->FinishTake(std::move(vmo));
                  });
          return;
        }
        weak_ptr->FinishTake(std::move(raw_vmo));
      });
  FX_DCHECK(status == ZX_OK);
}

void FlatlandScreenshot::FinishTake(zx::vmo response_vmo) {
  ScreenshotTakeResponse response;
  response.vmo(std::move(response_vmo));
  response.size(display_size_);
  take_callback_(std::move(response));

  take_callback_ = nullptr;
  render_event_.reset();

  // Release the buffer to allow for subsequent screenshots.
  screen_capturer_->ReleaseFrame(kBufferIndex, [](auto result) {});
}

zx::vmo FlatlandScreenshot::HandleFrameRender() {
  // Copy ScreenCapture output for inspection. Note that the stride of the buffer may be different
  // than the width of the image, if the width of the image is not a multiple of 64.
  //
  // For instance, is the original image were 1024x600, the new width is 600. 600*4=2400 bytes,
  // which is not a multiple of 64. The next multiple would be 2432, which would mean the buffer
  // is actually a 608x1024 "pixel" buffer, since 2432/4=608. We must account for that 8 byte
  // padding when copying the bytes over to be inspected.

  FX_DCHECK(kBytesPerPixel ==
            utils::GetBytesPerPixel(buffer_collection_info_[raw_format_].settings().value()));
  const uint32_t pixels_per_row = utils::GetPixelsPerRow(
      buffer_collection_info_[raw_format_].settings().value(), display_size_.width());
  uint32_t bytes_per_row = pixels_per_row * kBytesPerPixel;
  uint32_t valid_bytes_per_row = display_size_.width() * kBytesPerPixel;

  // SL4Fs requires vmo to be readable for transfer, so we need to copy into a new one.
  std::vector<uint8_t> buf(4, 0);
  const bool vmo_is_readable =
      (buffer_collection_info_[raw_format_].buffers().value()[kBufferIndex].vmo()->read(
           buf.data(), 0, 1) == ZX_OK);
  zx::vmo response_vmo;
  if (vmo_is_readable && bytes_per_row == valid_bytes_per_row) {
    // Do not need to map the buffer in this case so cannot use zx_cache_flush on the mapping.
    // Attempt to use the ZX_VMO_OP_CACHE_CLEAN_INVALIDATE, falling back to creating a temporary
    // mapping if the operation fails due to being a physical vmo.
    zx_status_t status =
        buffer_collection_info_[raw_format_].buffers().value()[kBufferIndex].vmo()->op_range(
            ZX_VMO_OP_CACHE_CLEAN_INVALIDATE, 0,
            buffer_collection_info_[raw_format_]
                .settings()
                .value()
                .buffer_settings()
                .value()
                .size_bytes()
                .value(),
            nullptr, 0);
    if (status == ZX_ERR_NOT_SUPPORTED) {
      // Receiving ZX_ERR_NOT_SUPPORTED from ZX_VMO_OP_CACHE_CLEAN_INVALIDATE indicates it is a
      // physical VMO that does not support cache operations. In this case map it in to use
      // zx_cache_flush.
      flatland::MapHostPointer(
          buffer_collection_info_[raw_format_], kBufferIndex,
          flatland::HostPointerAccessMode::kReadOnly, [](uint8_t* vmo_host, uint32_t num_bytes) {
            FX_DCHECK(ZX_OK == zx_cache_flush(vmo_host, num_bytes,
                                              ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));
          });
    } else {
      FX_DCHECK(status == ZX_OK);
    }
    status = buffer_collection_info_[raw_format_].buffers().value()[kBufferIndex].vmo()->duplicate(
        ZX_RIGHT_READ | ZX_RIGHT_MAP | ZX_RIGHT_TRANSFER | ZX_RIGHT_GET_PROPERTY, &response_vmo);
    FX_DCHECK(status == ZX_OK);
  } else {
    const auto response_vmo_size = static_cast<uint64_t>(display_size_.width()) *
                                   static_cast<uint64_t>(display_size_.height()) * kBytesPerPixel;
    FX_CHECK(ZX_OK == zx::vmo::create(response_vmo_size, 0, &response_vmo));
    uint8_t* response_vmo_base;
    FX_CHECK(ZX_OK == zx::vmar::root_self()->map(ZX_VM_PERM_WRITE | ZX_VM_PERM_READ, 0,
                                                 response_vmo, 0, response_vmo_size,
                                                 reinterpret_cast<uintptr_t*>(&response_vmo_base)));
    flatland::MapHostPointer(
        buffer_collection_info_[raw_format_], kBufferIndex,
        flatland::HostPointerAccessMode::kReadOnly,
        [&response_vmo_base, bytes_per_row, display_size = display_size_, valid_bytes_per_row,
         response_vmo_size](uint8_t* vmo_host, uint32_t num_bytes) {
          FX_CHECK(ZX_OK ==
                   zx_cache_flush(vmo_host,
                                  static_cast<size_t>(display_size.height()) * bytes_per_row,
                                  ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));
          for (size_t i = 0; i < display_size.height(); ++i) {
            FX_DCHECK(i * display_size.width() * kBytesPerPixel < response_vmo_size);
            memcpy(&response_vmo_base[i * display_size.width() * kBytesPerPixel],
                   &vmo_host[i * bytes_per_row], valid_bytes_per_row);
          }
        });

    FX_CHECK(ZX_OK == zx_cache_flush(response_vmo_base, response_vmo_size,
                                     ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));
    FX_CHECK(ZX_OK == zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(response_vmo_base),
                                                   response_vmo_size));
  }

  return response_vmo;
}

void FlatlandScreenshot::GetNextFrame() {
  FX_DCHECK(!render_event_);
  zx::event dup;
  zx_status_t status = zx::event::create(0, &render_event_);
  FX_DCHECK(status == ZX_OK);
  render_event_.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup);

  GetNextFrameArgs frame_args;
  frame_args.event(std::move(dup));
  FX_LOGS(INFO) << "Capturing next frame for screenshot.";
  screen_capturer_->GetNextFrame(std::move(frame_args), [](auto result) {
    if (result.is_error()) {
      FX_LOGS(ERROR) << "Error in GetNextFrame.";
    }
  });
}

void FlatlandScreenshot::TakeFile(TakeFileRequest& request, TakeFileCompleter::Sync& completer) {
  TakeFile(std::move(request), [completer = completer.ToAsync()](auto response) mutable {
    completer.Reply(std::move(response));
  });
}

void FlatlandScreenshot::TakeFile(fuchsia_ui_composition::ScreenshotTakeFileRequest params,
                                  fit::function<void(ScreenshotTakeFileResponse)> callback) {
  if (take_file_callback_ != nullptr) {
    FX_LOGS(ERROR)
        << "Screenshot::TakeFile() already in progress, closing connection. Wait for return "
           "before calling again.";
    destroy_instance_function_(this);
    return;
  }

  ScreenshotFormat format = raw_format_;
  if (params.format().has_value()) {
    format = params.format().value();
    if (format != ScreenshotFormat::kPng) {
      raw_format_ = format;
    }
  }
  if (!buffer_collection_info_.contains(raw_format_)) {
    AllocateBuffers();
  }

  if (!utils::IsEventSignalled(init_event_, ZX_EVENT_SIGNALED)) {
    // Begin to asynchronously wait on the event. Retry the Take() call when the initialization is
    // complete.
    zx_status_t status = init_wait_->Begin(
        async_get_default_dispatcher(),
        [weak_ptr = weak_factory_.GetWeakPtr(), params = std::move(params),
         callback = std::move(callback)](async_dispatcher_t*, async::WaitOnce*, zx_status_t status,
                                         const zx_packet_signal_t* signal) mutable {
          if (!weak_ptr) {
            return;
          }
          FX_DCHECK(status == ZX_OK || status == ZX_ERR_CANCELED);

          // Retry the Take() call.
          weak_ptr->TakeFile(std::move(params), std::move(callback));
        });
    FX_DCHECK(status == ZX_OK);
    return;
  }

  take_file_callback_ = std::move(callback);

  GetNextFrame();

  // Wait for the frame to render in an async fashion.
  render_wait_ = std::make_shared<async::WaitOnce>(render_event_.get(), ZX_EVENT_SIGNALED);
  zx_status_t status = render_wait_->Begin(
      async_get_default_dispatcher(), [this, weak_ptr = weak_factory_.GetWeakPtr(), format](
                                          async_dispatcher_t*, async::WaitOnce*, zx_status_t status,
                                          const zx_packet_signal_t*) mutable {
        FX_DCHECK(status == ZX_OK || status == ZX_ERR_CANCELED);
        if (!weak_ptr) {
          return;
        }
        FX_DCHECK(take_file_callback_);

        zx::vmo raw_vmo = weak_ptr->HandleFrameRender();

        if (format == ScreenshotFormat::kPng) {
          MaybeConnectToImageCompressor();
          zx::vmo response_vmo;
          zx::vmo response_vmo_copy;
          // Make |resonpnse_vmo| large enough to hold any potential PNG encoding of |raw_vmo|.
          // Once compression is complete |resonpnse_vmo| gets resized back down.
          const auto response_vmo_size =
              (static_cast<uint64_t>(display_size_.width()) *
               static_cast<uint64_t>(display_size_.height()) * kBytesPerPixel) +
              zx_system_get_page_size();
          FX_CHECK(zx::vmo::create(response_vmo_size, ZX_VMO_RESIZABLE, &response_vmo) == ZX_OK);
          FX_CHECK(response_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &response_vmo_copy) == ZX_OK);

          fuchsia_ui_compression_internal::ImageCompressorEncodePngRequest request;
          request.raw_vmo() = std::move(raw_vmo);
          request.image_dimensions(display_size_);
          request.png_vmo() = std::move(response_vmo);

          client_->EncodePng(std::move(request))
              .ThenExactlyOnce(
                  [weak_ptr, vmo = std::move(response_vmo_copy)](
                      fidl::Result<fuchsia_ui_compression_internal::ImageCompressor::EncodePng>
                          result) mutable {
                    if (!weak_ptr) {
                      return;
                    }
                    FX_DCHECK(weak_ptr->take_file_callback_);
                    if (result.is_error()) {
                      FX_LOGS(ERROR) << result.error_value().FormatDescription();
                      weak_ptr->render_event_.reset();
                      // Release the buffer to allow for subsequent screenshots.
                      weak_ptr->screen_capturer_->ReleaseFrame(kBufferIndex, [](auto result) {});
                      return;
                    }
                    weak_ptr->FinishTakeFile(std::move(vmo));
                  });
          return;
        }
        weak_ptr->FinishTakeFile(std::move(raw_vmo));
      });
  FX_DCHECK(status == ZX_OK);
}

void FlatlandScreenshot::FinishTakeFile(zx::vmo response_vmo) {
  ScreenshotTakeFileResponse response;
  auto [file_client, file_server] = fidl::Endpoints<fuchsia_io::File>::Create();
  const size_t screenshot_index = served_screenshots_next_id_++;
  if (ServeScreenshot(std::move(file_server), std::move(response_vmo), screenshot_index,
                      &served_screenshots_)) {
    response.file() = std::move(file_client);
    response.size(display_size_);
  }
  take_file_callback_(std::move(response));
  take_file_callback_ = nullptr;
  render_event_.reset();

  // Release the buffer to allow for subsequent screenshots.
  screen_capturer_->ReleaseFrame(kBufferIndex, [](auto result) {});
}

void FlatlandScreenshot::MaybeConnectToImageCompressor() {
  if (client_.is_valid()) {
    return;
  }

  auto endpoints = fidl::Endpoints<fuchsia_ui_compression_internal::ImageCompressor>::Create();

  // We are using natural bindings for the |ImageCompressor| protocol, but |app_context_|
  // uses HLCPP. We bridge them by manually connecting to the service using the Natural
  // protocol name.
  app_context_->svc()->Connect(
      fidl::DiscoverableProtocolName<fuchsia_ui_compression_internal::ImageCompressor>,
      endpoints.server.TakeChannel());

  client_.Bind(std::move(endpoints.client), async_get_default_dispatcher(), &event_handler_);
}

}  // namespace screenshot
