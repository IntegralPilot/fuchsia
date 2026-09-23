// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/utils/helpers.h"

#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/image-format/image_format.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/trace/event.h>

#include <fbl/algorithm.h>

#include "src/lib/fsl/handles/object_info.h"

#include <glm/gtc/constants.hpp>

namespace utils {

zx_koid_t ExtractKoid(const fuchsia_ui_views::ViewRef& view_ref) {
  TRACE_DURATION("gfx", "utils::ExtractKoid");
  return fsl::GetKoid(view_ref.reference().get());
}

zx_koid_t ExtractKoid(const fuchsia_ui_views::wire::ViewRef& view_ref) {
  TRACE_DURATION("gfx", "utils::ExtractKoid");
  return fsl::GetKoid(view_ref.reference.get());
}

bool IsEventSignalled(const zx::event& event, zx_signals_t signal) {
  zx_signals_t pending = 0u;
  event.wait_one(signal, zx::time(), &pending);
  return (pending & signal) != 0u;
}

// TODO: update file when the API level is updated in <lib/zx/counter.h>
// This is here so that we can use it within Scenic, i.e. outside of CTF tests.
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
bool IsCounterSignalled(const zx::counter& counter, zx_signals_t signal) {
  zx_signals_t pending = 0u;
  counter.wait_one(signal, zx::time(), &pending);
  return (pending & signal) != 0u;
}

int64_t ReadCounter(const zx::counter& counter) {
  int64_t value = 0;
  zx_status_t status = counter.read(&value);
  FX_DCHECK(status == ZX_OK);
  return value;
}
#endif

zx::event CreateEvent() {
  TRACE_DURATION("gfx", "CreateEvent");
  zx::event event;
  FX_CHECK(zx::event::create(0, &event) == ZX_OK);
  return event;
}

std::vector<zx::event> CreateEventArray(size_t n) {
  std::vector<zx::event> events;
  events.reserve(n);
  for (size_t i = 0; i < n; i++) {
    events.push_back(CreateEvent());
  }
  return events;
}

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
zx::counter CreateCounter() {
  TRACE_DURATION("gfx", "CreateCounter");
  zx::counter counter;
  FX_CHECK(zx::counter::create(0, &counter) == ZX_OK);
  return counter;
}

std::vector<zx::counter> CreateCounterArray(size_t n) {
  std::vector<zx::counter> counters;
  counters.reserve(n);
  for (size_t i = 0; i < n; i++) {
    counters.push_back(CreateCounter());
  }
  return counters;
}
#endif

std::vector<zx_koid_t> ExtractKoids(const std::vector<zx::event>& events) {
  TRACE_DURATION("gfx", "utils::ExtractKoids", "count", TA_UINT64(events.size()));
  std::vector<zx_koid_t> result;
  result.reserve(events.size());
  for (auto& evt : events) {
    zx_info_handle_basic_t info;
    zx_status_t status = evt.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
    FX_DCHECK(status == ZX_OK);
    result.push_back(info.koid);
  }
  return result;
}

fidl::WireClient<fuchsia_sysmem2::Allocator> CreateSysmemAllocatorClient(
    async_dispatcher_t* dispatcher, const std::string& debug_name_suffix) {
  return CreateSysmemAllocatorClientWithSvc(nullptr, dispatcher, debug_name_suffix);
}

#define ALLOCATOR_PROTOCOL "fuchsia.sysmem2.Allocator"

fidl::WireClient<fuchsia_sysmem2::Allocator> CreateSysmemAllocatorClientWithSvc(
    sys::ServiceDirectory* svc, async_dispatcher_t* dispatcher,
    const std::string& debug_name_suffix) {
  FX_CHECK(!debug_name_suffix.empty());
  auto endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::Allocator>();
  zx_status_t status = svc != nullptr
                           ? svc->Connect(ALLOCATOR_PROTOCOL, endpoints->server.TakeChannel())
                           : fdio_service_connect("/svc/" ALLOCATOR_PROTOCOL,
                                                  endpoints->server.TakeChannel().release());
  FX_DCHECK(status == ZX_OK);
  auto debug_name = fsl::GetCurrentProcessName() + " " + debug_name_suffix;
  FX_LOGS(INFO) << "CreateSysmemAllocatorClientWithSvc: debug_name=" << debug_name
                << " koid=" << fsl::GetCurrentProcessKoid();
  constexpr size_t kMaxNameLength = 64;  // from fuchsia.sysmem/allocator.fidl
  FX_DCHECK(debug_name.length() <= kMaxNameLength)
      << "Sysmem client debug name exceeded max length of " << kMaxNameLength << " (\""
      << debug_name << "\")";

  fidl::Arena arena;
  fidl::WireClient<fuchsia_sysmem2::Allocator> allocator(std::move(endpoints->client), dispatcher);
  fidl::OneWayStatus result = allocator->SetDebugClientInfo(
      fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena)
          .name(std::move(debug_name))
          .id(fsl::GetCurrentProcessKoid())
          .Build());
  FX_DCHECK(result.ok());
  return allocator;
}

fuchsia_sysmem2::BufferCollectionConstraints CreateDefaultConstraints(
    uint32_t buffer_count, uint32_t width, uint32_t height, fuchsia_images2::PixelFormat format,
    bool set_min_max_size) {
  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  fuchsia_sysmem2::BufferMemoryConstraints buffer_memory_constraints;
  buffer_memory_constraints.cpu_domain_supported(true);
  buffer_memory_constraints.ram_domain_supported(true);
  constraints.buffer_memory_constraints(std::move(buffer_memory_constraints));

  fuchsia_sysmem2::BufferUsage usage;
  usage.cpu(fuchsia_sysmem2::kCpuUsageReadOften | fuchsia_sysmem2::kCpuUsageWriteOften);
  constraints.usage(std::move(usage));
  constraints.min_buffer_count(buffer_count);

  fuchsia_sysmem2::ImageFormatConstraints image_constraints;
  image_constraints.color_spaces(std::vector{fuchsia_images2::ColorSpace::kSrgb});
  image_constraints.pixel_format(format);
  image_constraints.pixel_format_modifier(fuchsia_images2::PixelFormatModifier::kLinear);

  image_constraints.required_min_size(fuchsia_math::SizeU{{.width = width, .height = height}});
  image_constraints.required_max_size(fuchsia_math::SizeU{{.width = width, .height = height}});
  if (set_min_max_size) {
    image_constraints.min_size(fuchsia_math::SizeU{{.width = width, .height = height}});
    image_constraints.max_size(fuchsia_math::SizeU{{.width = width, .height = height}});
  }
  image_constraints.bytes_per_row_divisor(4);
  constraints.image_format_constraints(std::vector{std::move(image_constraints)});
  return constraints;
}

// Prints in row-major order.
void PrettyPrintMat3(std::string name, const std::array<float, 9>& mat3) {
  FX_LOGS(INFO) << "\n"
                << name << ":\n"
                << mat3[0] << "," << mat3[3] << "," << mat3[6] << "\n"
                << mat3[1] << "," << mat3[4] << "," << mat3[7] << "\n"
                << mat3[2] << "," << mat3[5] << "," << mat3[8];
}

float GetOrientationAngle(fuchsia_ui_composition::Orientation orientation) {
  switch (orientation) {
    case fuchsia_ui_composition::Orientation::kCcw0Degrees:
      return 0.f;
    case fuchsia_ui_composition::Orientation::kCcw90Degrees:
      return -glm::half_pi<float>();
    case fuchsia_ui_composition::Orientation::kCcw180Degrees:
      return -glm::pi<float>();
    case fuchsia_ui_composition::Orientation::kCcw270Degrees:
      return -glm::three_over_two_pi<float>();
  }
}

namespace {

uint32_t GetBytesPerRow(const fuchsia_sysmem2::ImageFormatConstraints& image_format_constraints,
                        uint32_t image_width, uint32_t bytes_per_pixel) {
  uint32_t bytes_per_row_divisor = image_format_constraints.bytes_per_row_divisor().value_or(1);
  uint32_t min_bytes_per_row = image_format_constraints.min_bytes_per_row().value_or(0);
  uint32_t bytes_per_row = fbl::round_up(std::max(image_width * bytes_per_pixel, min_bytes_per_row),
                                         bytes_per_row_divisor);
  return bytes_per_row;
}

}  // namespace

uint32_t GetBytesPerPixel(const fuchsia_sysmem2::SingleBufferSettings& settings) {
  return GetBytesPerPixel(settings.image_format_constraints().value());
}

uint32_t GetBytesPerPixel(const fuchsia_sysmem2::ImageFormatConstraints& image_format_constraints) {
  fuchsia_images2::PixelFormat pixel_format = image_format_constraints.pixel_format().value();
  fuchsia_images2::PixelFormatModifier pixel_format_modifier =
      image_format_constraints.pixel_format_modifier().value_or(
          fuchsia_images2::PixelFormatModifier::kLinear);
  PixelFormatAndModifier pixel_format_and_modifier(pixel_format, pixel_format_modifier);
  return ImageFormatStrideBytesPerWidthPixel(pixel_format_and_modifier);
}

uint32_t GetBytesPerRow(const fuchsia_sysmem2::SingleBufferSettings& settings,
                        uint32_t image_width) {
  return GetBytesPerRow(settings.image_format_constraints().value(), image_width);
}

uint32_t GetBytesPerRow(const fuchsia_sysmem2::ImageFormatConstraints& image_format_constraints,
                        uint32_t image_width) {
  uint32_t bytes_per_pixel = GetBytesPerPixel(image_format_constraints);
  return GetBytesPerRow(image_format_constraints, image_width, bytes_per_pixel);
}

uint32_t GetPixelsPerRow(const fuchsia_sysmem2::SingleBufferSettings& settings,
                         uint32_t image_width) {
  return GetPixelsPerRow(settings.image_format_constraints().value(), image_width);
}

uint32_t GetPixelsPerRow(const fuchsia_sysmem2::ImageFormatConstraints& image_format_constraints,
                         uint32_t image_width) {
  uint32_t bytes_per_pixel = GetBytesPerPixel(image_format_constraints);
  return GetBytesPerRow(image_format_constraints, image_width, bytes_per_pixel) / bytes_per_pixel;
}

void SignalReleaseFences(const std::vector<zx::event>& fences) {
  for (auto& e : fences) {
    e.signal(0u, ZX_EVENT_SIGNALED);
  }
}

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
void SignalCounterFences(const std::vector<zx::counter>& fences, zx::time timestamp) {
  for (auto& c : fences) {
    c.write(timestamp.get());
    c.signal(0u, ZX_COUNTER_SIGNALED);
  }
}
#endif

}  // namespace utils
