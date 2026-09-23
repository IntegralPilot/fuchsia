// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <zircon/availability.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(29)
#include <elf-search.h>
#include <fidl/fuchsia.memory.sampler/cpp/fidl.h>
#include <fidl/fuchsia.memory.sampler/cpp/natural_types.h>
#include <fidl/fuchsia.memory.sampler/cpp/wire_types.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/zx/result.h>
#include <pthread.h>
#include <zircon/assert.h>
#include <zircon/sanitizer.h>
#include <zircon/syscalls/object.h>

#include <atomic>
#include <cstdint>
#include <unordered_set>
#include <vector>

#include <fbl/auto_lock.h>

#include "poisson_sampler.h"
#include "recorder.h"

namespace {
// Note: The destructor is never called. This is on purpose.
std::atomic<memory_sampler::Recorder*> singleton;

// Threshold for truncating overly long stack frames.
constexpr size_t kMaxStackFramesLength = 512;

// Note: this implementation currently relies on `thread_local` to
// track the allocated bytes and thresholds, which means that each
// thread implements a different Poisson process.
memory_sampler::PoissonSampler& GetNonDeterministicPoissonSampler() {
  thread_local memory_sampler::PoissonSampler sampler{
      memory_sampler::Recorder::kSamplingIntervalBytes};
  return sampler;
}
}  // namespace

namespace memory_sampler {

// Upon connecting, transmits the memory layout of the current process.
void Recorder::InitSingletonOnce() {
  alignas(Recorder) static std::byte storage[sizeof(Recorder)];
  auto result = component::Connect<fuchsia_memory_sampler::Sampler>();
  ZX_ASSERT(result.is_ok());

  fidl::SyncClient client{std::move(result.value())};
  zx::socket client_socket;

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  zx::socket server_socket;
  zx_status_t status = zx::socket::create(ZX_SOCKET_DATAGRAM, &client_socket, &server_socket);
  ZX_ASSERT(status == ZX_OK);

  auto set_socket_result = client->SetSharedSocket({{.socket = std::move(server_socket)}});
  ZX_ASSERT(set_socket_result.is_ok());
#endif

  auto* recorder = new (storage)
      Recorder(std::move(client), std::move(client_socket), GetNonDeterministicPoissonSampler);

  recorder->SetModulesInfo();

  // Only set the singleton once all allocations are done, to avoid a deadlock.
  singleton.store(recorder, std::memory_order_release);
}

Recorder* Recorder::Get() {
  static std::once_flag once_flag;
  std::call_once(once_flag, InitSingletonOnce);
  return singleton.load();
}

Recorder* Recorder::GetIfReady() { return singleton.load(std::memory_order_acquire); }

void Recorder::Disconnect() {
  if (bool expected = false;
      !is_disabled_.compare_exchange_strong(expected, true, std::memory_order_acq_rel)) {
    return;
  }

  if (this == singleton.load(std::memory_order_relaxed)) {
    singleton.store(nullptr, std::memory_order_release);
  }

  // Release the tracked allocations, but do so outside of `lock_`: clearing
  // the set frees one node per entry, and this runs inside an allocator hook
  // on an arbitrary application thread. Swapping the set out under the lock is
  // O(1), and the nodes are then freed once `stale_allocations` goes out of
  // scope, without any other thread waiting on us.
  std::unordered_set<void*> stale_allocations;
  {
    fbl::AutoLock lock(&lock_);
    client_ = {};
    stale_allocations.swap(recorded_allocations_);
  }
}

// Profiling every single allocation has a large impact on the
// performance of the instrumented process. Sampling allocation
// profilers work around this issue by sampling a subset of the
// allocations, reducing the overhead while hopefully capturing enough
// relevant data to be useful.
void Recorder::MaybeRecordAllocation(void* address, size_t size) {
  if (is_disabled_.load(std::memory_order_relaxed)) {
    return;
  }

  if (!GetPoissonSampler().ShouldSampleAllocation(size))
    return;

  if (!RecordAllocation(address, size)) {
    return;
  }

  // Store the address of the allocation if still active.
  {
    fbl::AutoLock lock(&lock_);
    if (!is_disabled_.load(std::memory_order_relaxed)) {
      recorded_allocations_.emplace(address);
    }
  }
}

bool Recorder::RecordAllocation(void* address, size_t size) {
  if (is_disabled_.load(std::memory_order_relaxed)) {
    return false;
  }
  uint64_t pc_buffer[kMaxStackFramesLength]{0};
  const size_t count = __sanitizer_fast_backtrace(pc_buffer, kMaxStackFramesLength);

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  fuchsia_memory_sampler::RecordAllocationEvent event{{
      .address = std::optional{reinterpret_cast<uint64_t>(address)},
      .stack_trace = std::optional<fuchsia_memory_sampler::StackTrace>{{{
          .stack_frames = std::optional{std::vector<uint64_t>(pc_buffer, pc_buffer + count)},
      }}},
      .size = std::optional<uint64_t>{size},
  }};

  if (socket_.is_valid()) {
    auto datagram = fuchsia_memory_sampler::SamplerDatagram::WithRecordAllocation(std::move(event));
    fit::result encoded = fidl::Persist(datagram);
    if (encoded.is_ok()) {
      zx_status_t status = socket_.write(0, encoded->data(), encoded->size(), nullptr);
      if (status == ZX_OK) {
        return true;
      }
      if (status == ZX_ERR_PEER_CLOSED) {
        Disconnect();
      } else if (status == ZX_ERR_SHOULD_WAIT) {
        if (!peer_signaled_.exchange(true, std::memory_order_relaxed)) {
          if (socket_.signal_peer(0, ZX_USER_SIGNAL_0) == ZX_ERR_PEER_CLOSED) {
            Disconnect();
          }
        }
      }
    }
    return false;
  }

  // Fallback FIDL path
  bool is_peer_closed = false;
  bool is_ok = false;
  {
    fbl::AutoLock lock(&lock_);
    if (!client_.is_valid()) {
      return false;
    }
    zx_signals_t signals = 0;
    if (client_.client_end().channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(),
                                                &signals) == ZX_OK &&
        (signals & ZX_CHANNEL_PEER_CLOSED)) {
      is_peer_closed = true;
    } else {
      auto result = client_->RecordAllocation(event);
      if (result.is_error()) {
        is_peer_closed = true;
      } else {
        is_ok = true;
      }
    }
  }
  if (is_peer_closed) {
    Disconnect();
    return false;
  }
  return is_ok;
#else
  bool is_peer_closed = false;
  bool is_ok = false;
  {
    fbl::AutoLock lock(&lock_);
    if (!client_.is_valid()) {
      return false;
    }
    zx_signals_t signals = 0;
    if (client_.client_end().channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(),
                                                &signals) == ZX_OK &&
        (signals & ZX_CHANNEL_PEER_CLOSED)) {
      is_peer_closed = true;
    } else {
      auto result = client_->RecordAllocation({{
          .address = std::optional{reinterpret_cast<uint64_t>(address)},
          .stack_trace = std::optional<fuchsia_memory_sampler::StackTrace>(
              {{.stack_frames =
                    std::optional{std::vector<uint64_t>(pc_buffer, pc_buffer + count)}}}),
          .size = std::optional<uint64_t>{size},
      }});
      if (result.is_error()) {
        is_peer_closed = true;
      } else {
        is_ok = true;
      }
    }
  }
  if (is_peer_closed) {
    Disconnect();
    return false;
  }
  return is_ok;
#endif
}

void Recorder::MaybeForgetAllocation(void* address) {
  if (is_disabled_.load(std::memory_order_relaxed)) {
    return;
  }

  {
    fbl::AutoLock lock(&lock_);
    auto allocation = recorded_allocations_.find(address);
    if (allocation == recorded_allocations_.end()) {
      return;
    }
    recorded_allocations_.erase(allocation);
  }

  ForgetAllocation(address);
}

void Recorder::ForgetAllocation(void* address) {
  if (is_disabled_.load(std::memory_order_relaxed)) {
    return;
  }
  uint64_t pc_buffer[kMaxStackFramesLength]{0};
  const size_t count = __sanitizer_fast_backtrace(pc_buffer, kMaxStackFramesLength);

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  fuchsia_memory_sampler::RecordDeallocationEvent event{{
      .address = std::optional{reinterpret_cast<uint64_t>(address)},
      .stack_trace = std::optional<fuchsia_memory_sampler::StackTrace>{{{
          .stack_frames = std::optional{std::vector<uint64_t>(pc_buffer, pc_buffer + count)},
      }}},
  }};

  if (socket_.is_valid()) {
    auto datagram =
        fuchsia_memory_sampler::SamplerDatagram::WithRecordDeallocation(std::move(event));
    fit::result encoded = fidl::Persist(datagram);
    if (encoded.is_ok()) {
      zx_status_t status = socket_.write(0, encoded->data(), encoded->size(), nullptr);
      if (status == ZX_ERR_PEER_CLOSED) {
        Disconnect();
      } else if (status == ZX_ERR_SHOULD_WAIT) {
        if (!peer_signaled_.exchange(true, std::memory_order_relaxed)) {
          if (socket_.signal_peer(0, ZX_USER_SIGNAL_0) == ZX_ERR_PEER_CLOSED) {
            Disconnect();
          }
        }
      }
    }
    return;
  }

  // Fallback FIDL path
  bool is_peer_closed = false;
  {
    fbl::AutoLock lock(&lock_);
    if (!client_.is_valid()) {
      return;
    }
    zx_signals_t signals = 0;
    if (client_.client_end().channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(),
                                                &signals) == ZX_OK &&
        (signals & ZX_CHANNEL_PEER_CLOSED)) {
      is_peer_closed = true;
    } else {
      auto result = client_->RecordDeallocation(event);
      if (result.is_error()) {
        is_peer_closed = true;
      }
    }
  }
  if (is_peer_closed) {
    Disconnect();
  }
#else
  bool is_peer_closed = false;
  {
    fbl::AutoLock lock(&lock_);
    if (!client_.is_valid()) {
      return;
    }
    zx_signals_t signals = 0;
    if (client_.client_end().channel().wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite_past(),
                                                &signals) == ZX_OK &&
        (signals & ZX_CHANNEL_PEER_CLOSED)) {
      is_peer_closed = true;
    } else {
      auto result = client_->RecordDeallocation(
          {{.address = std::optional{reinterpret_cast<uint64_t>(address)},
            .stack_trace = std::optional<fuchsia_memory_sampler::StackTrace>{
                {{.stack_frames =
                      std::optional{std::vector<uint64_t>(pc_buffer, pc_buffer + count)}}}}}});
      if (result.is_error()) {
        is_peer_closed = true;
      }
    }
  }
  if (is_peer_closed) {
    Disconnect();
  }
#endif
}

void Recorder::SetModulesInfo() {
  // Collect the layout of the modules loaded in memory.
  const zx_handle_t process = zx_process_self();
  std::vector<fuchsia_memory_sampler::ModuleMap> modules;

  // Iterate through modules to map code memory ranges to the
  // corresponding build id.
  elf_search::ForEachModule(
      *zx::unowned_process{process}, [&modules](const elf_search::ModuleInfo& info) mutable {
        const size_t kPageSize = zx_system_get_page_size();
        std::vector<fuchsia_memory_sampler::ExecutableSegment> segments;

        // Iterate through program segments.
        for (const auto& phdr : info.phdrs) {
          // Skip non-loadable sections.
          if (phdr.p_type != PT_LOAD) {
            continue;
          }
          // Skip non-executable sections.
          bool executable = !!(phdr.p_flags & PF_X);
          if (!executable) {
            continue;
          }

          const uintptr_t start = phdr.p_vaddr & -kPageSize;
          const uintptr_t end = (phdr.p_vaddr + phdr.p_memsz + kPageSize - 1) & -kPageSize;
          auto& segment = segments.emplace_back();
          segment.start_address(info.vaddr + start);
          segment.size(end - start);
          segment.relative_address(start);
        }

        auto& module_map = modules.emplace_back();
        module_map.build_id({{info.build_id.begin(), info.build_id.end()}});
        module_map.executable_segments(segments);
      });

  // Retrieve the name of the current process.
  char name[ZX_MAX_NAME_LEN];
  zx_object_get_property(process, ZX_PROP_NAME, name, ZX_MAX_NAME_LEN);

  // Perform the FIDL call.
  {
    fbl::AutoLock lock(&lock_);
    auto result = client_->SetProcessInfo({{.process_name = name, .module_map = modules}});
    ZX_ASSERT(result.is_ok());
  }
}

Recorder::Recorder(fidl::SyncClient<fuchsia_memory_sampler::Sampler> client, zx::socket socket,
                   std::function<PoissonSampler&()> get_poisson_sampler)
    : client_(std::move(client)),
      socket_(std::move(socket)),
      GetPoissonSampler(std::move(get_poisson_sampler)) {}

Recorder Recorder::CreateRecorderForTesting(
    fidl::SyncClient<fuchsia_memory_sampler::Sampler> client,
    std::function<PoissonSampler&()> get_poisson_sampler, bool use_socket) {
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  if (use_socket) {
    zx::socket client_socket, server_socket;
    zx_status_t status = zx::socket::create(ZX_SOCKET_DATAGRAM, &client_socket, &server_socket);
    ZX_ASSERT(status == ZX_OK);
    auto set_socket_result = client->SetSharedSocket({{.socket = std::move(server_socket)}});
    ZX_ASSERT(set_socket_result.is_ok());
    return Recorder{std::move(client), std::move(client_socket), std::move(get_poisson_sampler)};
  }
#endif
  return Recorder{std::move(client), zx::socket{}, std::move(get_poisson_sampler)};
}
}  // namespace memory_sampler
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(29)
