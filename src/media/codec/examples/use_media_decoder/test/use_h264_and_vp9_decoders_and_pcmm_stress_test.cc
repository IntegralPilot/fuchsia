// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.sysinfo/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/media/codec_impl/fourcc.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zx/time.h>
#include <stdio.h>
#include <stdlib.h>
#include <zircon/system/public/zircon/syscalls.h>

#include <map>
#include <mutex>
#include <random>
#include <set>
#include <thread>

#include <bind/fuchsia/amlogic/platform/sysmem/heap/cpp/bind.h>

#include "src/media/codec/examples/use_media_decoder/use_video_decoder.h"
#include "src/media/codec/examples/use_media_decoder/util.h"
#include "use_video_decoder_test.h"

namespace {

constexpr char kH264InputFilePath[] = "/pkg/data/bear.h264";
constexpr int kH264InputFileFrameCount = 30;
// const char* kH264GoldenSha256 =
// "a4418265eaa493604731d6871523ac2a0d606f40cddd48e2a8cd0b0aa5f152e1";

constexpr char kVp9InputFilePath[] = "/pkg/data/bear-vp9.ivf";
constexpr int kVp9InputFileFrameCount = 82;
// const char* kVp9GoldenSha256 =
// "8317a8c078a0c27b7a524a25bf9964ee653063237698411361b415a449b23014";

constexpr uint32_t kThreadCount = 3;

constexpr uint32_t kAllocationChunkSize = 128 * 1024;
constexpr uint32_t kMaxChunksPerBuffer = 4;
constexpr uint32_t kMaxProtectedSpaceUsageMiB = 16;
constexpr uint32_t kMaxVmos =
    kMaxProtectedSpaceUsageMiB * 1024 * 1024 / kAllocationChunkSize / kMaxChunksPerBuffer;

}  // namespace

bool is_board_with_amlogic_secure();

void stress_pcmm(std::vector<zx::vmo>& vmos, std::mutex& vmos_lock,
                 std::function<uint32_t()> get_random);

int main(int argc, char* argv[]) {
  std::atomic_bool passing = true;
  constexpr uint32_t kIterations = 1;
  for (uint32_t iteration = 0; iteration < kIterations; ++iteration) {
    zx::time done_time = zx::deadline_after(zx::sec(30));

    ZX_ASSERT(kAllocationChunkSize / zx_system_get_page_size());
    LOGF("kMaxVmos: %u", kMaxVmos);

    std::mutex vmos_lock;
    std::vector<zx::vmo> vmos;
    vmos.resize(kMaxVmos);

    // Setting kForcedSeed isn't likely to help much in getting a repro, but might slightly help?
    static constexpr std::optional<uint64_t> kForcedSeed{};
    std::mutex random_lock;
    std::random_device random_device;
    std::uint_fast64_t seed{kForcedSeed ? *kForcedSeed : random_device()};
    printf("seed (non-deterministic overall though): %" PRIu64 "\n", seed);
    std::mt19937_64 prng{seed};
    std::uniform_int_distribution<uint32_t> uint32_distribution(
        0, std::numeric_limits<uint32_t>::max());
    auto get_random = [&] {
      std::lock_guard<std::mutex> lock(random_lock);
      return uint32_distribution(prng);
    };

    const UseVideoDecoderTestParams h264_test_params{
        .mime_type = "video/h264",
        //.golden_sha256 = kH264GoldenSha256,
        .skip_formatting_output_pixels = true,
    };

    const UseVideoDecoderTestParams vp9_test_params{
        .mime_type = "video/vp9",
        //.golden_sha256 = kVp9GoldenSha256,
        .skip_formatting_output_pixels = true,
    };

    std::atomic_bool go = false;
    std::unique_ptr<std::thread> threads[kThreadCount];
    for (uint32_t i = 0; i < kThreadCount; ++i) {
      threads[i] = std::make_unique<std::thread>([&] {
        while (!go) {
          zx::nanosleep(zx::deadline_after(zx::usec(1)));
        }
        zx::time now;
        do {
          int result = 0;
          switch (get_random() % 3) {
            case 0:
              result = use_video_decoder_test(kVp9InputFilePath, kVp9InputFileFrameCount,
                                              use_vp9_decoder,
                                              /*is_secure_output=*/is_board_with_amlogic_secure(),
                                              /*is_secure_input=*/false,
                                              /*min_output_buffer_count=*/0, &vp9_test_params);
              break;
            case 1:
              result = use_video_decoder_test(kH264InputFilePath, kH264InputFileFrameCount,
                                              use_h264_decoder,
                                              /*is_secure_output=*/is_board_with_amlogic_secure(),
                                              /*is_secure_input=*/false,
                                              /*min_output_buffer_count=*/0, &h264_test_params);
              break;
            case 2:
              stress_pcmm(vmos, vmos_lock, get_random);
              break;
          }
          if (result != 0) {
            LOGF("passing = false");
            passing = false;
          }
          now = zx::clock::get_monotonic();
        } while (now < done_time && passing);
      });
    }

    go = true;
    for (uint32_t i = 0; i < kThreadCount; ++i) {
      threads[i]->join();
    }
    if (!passing) {
      LOGF("RESULT: FAIL");
      return -1;
    }
  }
  if (passing) {
    LOGF("RESULT: PASS");
    return 0;
  } else {
    LOGF("RESULT: FAIL");
    return -1;
  }
}

const std::string& GetBoardName() {
  static std::string s_board_name;
  if (s_board_name.empty()) {
    auto client_end = component::Connect<fuchsia_sysinfo::SysInfo>();
    ZX_ASSERT(client_end.is_ok());

    fidl::WireSyncClient sysinfo{std::move(client_end.value())};
    auto result = sysinfo->GetBoardName();
    ZX_ASSERT(result.ok());
    ZX_ASSERT(result.value().status == ZX_OK);

    s_board_name = result.value().name.get();
    printf("\nFound board %s\n", s_board_name.c_str());
  }
  return s_board_name;
}

bool is_board_astro() { return GetBoardName() == "astro"; }

bool is_board_sherlock() { return GetBoardName() == "sherlock"; }

bool is_board_luis() { return GetBoardName() == "luis"; }

bool is_board_nelson() { return GetBoardName() == "nelson"; }

bool is_board_with_amlogic_secure() {
  if (is_board_astro()) {
    return true;
  }
  if (is_board_sherlock()) {
    return true;
  }
  if (is_board_luis()) {
    return true;
  }
  if (is_board_nelson()) {
    return true;
  }
  return false;
}

zx::result<fidl::SyncClient<fuchsia_sysmem2::Allocator>> connect_to_sysmem_service() {
  auto client_end = component::Connect<fuchsia_sysmem2::Allocator>();
  ZX_ASSERT(client_end.is_ok());
  if (!client_end.is_ok()) {
    return zx::error(client_end.status_value());
  }
  fidl::SyncClient allocator{std::move(client_end.value())};
  fuchsia_sysmem2::AllocatorSetDebugClientInfoRequest set_debug_request;
  set_debug_request.name() = "use_h264_and_vp9_decoders_and_pcmm_stress_test";
  set_debug_request.id() = 0u;
  auto result = allocator->SetDebugClientInfo(std::move(set_debug_request));
  ZX_ASSERT(result.is_ok());
  return zx::ok(std::move(allocator));
}

void set_picky_protected_constraints(
    fidl::SyncClient<fuchsia_sysmem2::BufferCollection>& collection, uint32_t exact_buffer_size) {
  ZX_ASSERT(exact_buffer_size % zx_system_get_page_size() == 0);
  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  auto& constraints = set_constraints_request.constraints().emplace();
  constraints.usage().emplace().video() = fuchsia_sysmem2::kVideoUsageHwDecoder;
  constraints.min_buffer_count_for_camping() = 1;
  auto& bmc = constraints.buffer_memory_constraints().emplace();
  bmc.min_size_bytes() = exact_buffer_size;
  // Allow a max that's just large enough to accommodate the size implied
  // by the min frame size and PixelFormat.
  bmc.max_size_bytes() = exact_buffer_size, bmc.physically_contiguous_required() = true,
  bmc.secure_required() = true, bmc.ram_domain_supported() = false,
  bmc.cpu_domain_supported() = false, bmc.inaccessible_domain_supported() = true,
  bmc.permitted_heaps().emplace().emplace_back(
      sysmem::MakeHeap(bind_fuchsia_amlogic_platform_sysmem_heap::HEAP_TYPE_SECURE, 0));
  auto result = collection->SetConstraints(std::move(set_constraints_request));
  ZX_ASSERT(result.is_ok());
}

// stress protected contiguous memory management
void stress_pcmm(std::vector<zx::vmo>& vmos, std::mutex& vmos_lock,
                 std::function<uint32_t()> get_random) {
  if (!is_board_with_amlogic_secure()) {
    return;
  }
  auto maybe_allocator = connect_to_sysmem_service();
  ZX_ASSERT(maybe_allocator.is_ok());
  auto allocator = std::move(maybe_allocator.value());
  zx::time done_time = zx::deadline_after(zx::sec(3));
  zx::time now;
  do {
    now = zx::clock::get_monotonic();
    auto [collection_client, collection_server] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
    fuchsia_sysmem2::AllocatorAllocateNonSharedCollectionRequest allocate_non_share_request;
    allocate_non_share_request.collection_request() = std::move(collection_server);
    auto allocate_result =
        allocator->AllocateNonSharedCollection(std::move(allocate_non_share_request));
    ZX_ASSERT(allocate_result.is_ok());
    fidl::SyncClient collection{std::move(collection_client)};
    auto sync_result = collection->Sync();
    ZX_ASSERT(sync_result.is_ok());
    uint32_t chunk_count = get_random() % (kMaxChunksPerBuffer - 1) + 1;
    uint32_t buffer_size = chunk_count * kAllocationChunkSize;
    set_picky_protected_constraints(collection, buffer_size);
    auto wait_result = collection->WaitForAllBuffersAllocated();
    ZX_ASSERT(wait_result.is_ok());
    uint32_t which_vmo = get_random() % kMaxVmos;
    // Keep the VMO for a while.  The VMO space won't be reclaimed until this handle closes, despite
    // us dropping the collection channel every time through this loop.
    {  // scope lock
      std::lock_guard<std::mutex> lock(vmos_lock);
      vmos[which_vmo] =
          std::move(wait_result->buffer_collection_info()->buffers()->at(0).vmo().value());
    }
    // Potentially avoid some sysmem log output by doing a clean Close().
    auto close_result = collection->Release();
    ZX_ASSERT(close_result.is_ok());
  } while (now < done_time);
}
