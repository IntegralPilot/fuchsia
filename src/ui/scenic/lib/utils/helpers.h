// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_UTILS_HELPERS_H_
#define SRC_UI_SCENIC_LIB_UTILS_HELPERS_H_

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <zircon/availability.h>

// TODO: update file when the API level is updated in <lib/zx/counter.h>
// This is here so that we can use it within Scenic, i.e. outside of CTF tests.
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
#include <lib/zx/counter.h>
#endif
#include <lib/zx/event.h>

namespace utils {

constexpr std::array<float, 2> kDefaultPixelScale = {1.f, 1.f};

struct SysmemTokens {
  // Token for setting client side constraints.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> local_token;

  // Token for setting server side constraints.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> dup_token;

  static SysmemTokens Create(fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator) {
    auto [local_token_client, local_token_server] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator->AllocateSharedCollection(
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

    return {
        .local_token = std::move(local_token_client),
        .dup_token = std::move(dup_token_client),
    };
  }
};

// Helper for extracting the koid from a ViewRef.
zx_koid_t ExtractKoid(const fuchsia_ui_views::ViewRef& view_ref);
zx_koid_t ExtractKoid(const fuchsia_ui_views::wire::ViewRef& view_ref);

// Create an unsignalled zx::event.
zx::event CreateEvent();

// Create a std::vector populated with |n| unsignalled zx::event elements.
std::vector<zx::event> CreateEventArray(size_t n);

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
// Create a zx::counter with initial value 0.
zx::counter CreateCounter();

// Create a std::vector populated with |n| zx::counter elements.
std::vector<zx::counter> CreateCounterArray(size_t n);
#endif

// Create a std::vector populated with koids of the input vector of zx:event.
std::vector<zx_koid_t> ExtractKoids(const std::vector<zx::event>& events);

// Copy a zx object handle.
template <typename T>
T CopyZxHandle(const T& handle) {
  TRACE_DURATION("gfx", "utils::CopyZxHandle");
  T handle_copy;
  if (handle.duplicate(ZX_RIGHT_SAME_RIGHTS, &handle_copy) != ZX_OK) {
    FX_LOGS(ERROR) << "Copying zx object handle failed.";
    FX_DCHECK(false);
  }
  return handle_copy;
}

// Copy a std::vector of zx object handles.
template <typename T>
std::vector<T> CopyZxHandleVector(const std::vector<T>& handles) {
  std::vector<T> result;
  result.reserve(handles.size());
  for (const auto& h : handles) {
    result.push_back(CopyZxHandle(h));
  }
  return result;
}

// Synchronously checks whether the event has signalled any of the bits in |signal|.
bool IsEventSignalled(const zx::event& event, zx_signals_t signal);

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
// Synchronously checks whether the counter has signalled any of the bits in |signal|.
bool IsCounterSignalled(const zx::counter& counter, zx_signals_t signal);

// Synchronously reads the value of the counter.
int64_t ReadCounter(const zx::counter& counter);
#endif

// Create sysmem allocator.
fidl::WireClient<fuchsia_sysmem2::Allocator> CreateSysmemAllocatorClient(
    async_dispatcher_t* dispatcher, const std::string& debug_name_suffix = std::string());

// Create sysmem allocator.
fidl::WireClient<fuchsia_sysmem2::Allocator> CreateSysmemAllocatorClientWithSvc(
    sys::ServiceDirectory* svc, async_dispatcher_t* dispatcher,
    const std::string& debug_name_suffix = std::string());

// Creates default constraints for |buffer_collection|
fuchsia_sysmem2::BufferCollectionConstraints CreateDefaultConstraints(
    uint32_t buffer_count, uint32_t width, uint32_t height,
    fuchsia_images2::PixelFormat format = fuchsia_images2::PixelFormat::kB8G8R8A8,
    bool set_min_max_size = false);

void PrettyPrintMat3(std::string, const std::array<float, 9>& mat3);

template <typename ArrayT>
std::string GetArrayString(const std::string& name, const ArrayT& array) {
  std::string result = name + ": [";
  for (size_t i = 0; i < array.size(); i++) {
    result += std::to_string(array[i]);
    if (i < array.size() - 1) {
      result += ", ";
    }
  }
  result += "]\n";
  return result;
}

float GetOrientationAngle(fuchsia_ui_composition::Orientation orientation);

uint32_t GetBytesPerPixel(const fuchsia_sysmem2::SingleBufferSettings& settings);
uint32_t GetBytesPerPixel(const fuchsia_sysmem2::ImageFormatConstraints& image_format_constraints);

uint32_t GetBytesPerRow(const fuchsia_sysmem2::SingleBufferSettings& settings,
                        uint32_t image_width);
uint32_t GetBytesPerRow(const fuchsia_sysmem2::ImageFormatConstraints& image_format_constraints,
                        uint32_t image_width);

uint32_t GetPixelsPerRow(const fuchsia_sysmem2::SingleBufferSettings& settings,
                         uint32_t image_width);
uint32_t GetPixelsPerRow(const fuchsia_sysmem2::ImageFormatConstraints& image_format_constraints,
                         uint32_t image_width);

// Signal all fences with ZX_EVENT_SIGNALED.
void SignalReleaseFences(const std::vector<zx::event>& fences);

// For each fence:
// - write the timestamp into it
// - signal it with ZX_COUNTER_SIGNALED
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
void SignalCounterFences(const std::vector<zx::counter>& fences, zx::time timestamp);
#endif

}  // namespace utils

#endif  // SRC_UI_SCENIC_LIB_UTILS_HELPERS_H_
