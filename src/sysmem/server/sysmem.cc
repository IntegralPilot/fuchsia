// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sysmem.h"

#include <dirent.h>
#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <inttypes.h>
#include <lib/async-loop/loop.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/sync/cpp/completion.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zx/channel.h>
#include <lib/zx/event.h>
#include <lib/zx/thread.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/processargs.h>
#include <zircon/syscalls/iommu.h>
#include <zircon/threads.h>

#include <algorithm>
#include <memory>
#include <string>
#include <thread>

#include <bind/fuchsia/sysmem/heap/cpp/bind.h>
#include <fbl/string_printf.h>
#include <sdk/lib/sys/cpp/service_directory.h>

#include "lib/async/cpp/task.h"
#include "src/sysmem/metrics/metrics.cb.h"
#include "src/sysmem/server/allocator.h"
#include "src/sysmem/server/buffer_collection_token.h"
#include "src/sysmem/server/contiguous_pooled_memory_allocator.h"
#include "src/sysmem/server/external_memory_allocator.h"
#include "src/sysmem/server/macros.h"
#include "src/sysmem/server/utils.h"
#include "zircon/status.h"

namespace sysmem_service {

namespace {

constexpr bool kLogAllCollectionsPeriodically = false;
constexpr zx::duration kLogAllCollectionsInterval = zx::sec(20);

// These defaults only take effect if there is no
// fuchsia.hardware.sysmem/SYSMEM_METADATA_TYPE, and also neither of these
// kernel cmdline parameters set: driver.sysmem.contiguous_memory_size
// driver.sysmem.protected_memory_size
//
// Typically these defaults are overriden.
//
// By default there is no protected memory pool.
constexpr int64_t kDefaultProtectedMemorySize = 0;
// By default we pre-reserve 5% of physical memory for contiguous memory
// allocation via sysmem.
//
// This is enough to allow tests in sysmem_tests.cc to pass, and avoids relying
// on zx::vmo::create_contiguous() after early boot (by default), since it can
// fail if physical memory has gotten too fragmented.
constexpr int64_t kDefaultContiguousMemorySize = -5;

constexpr char kSysmemConfigFilename[] = "/sysmem-config/config.sysmem_config_persistent_fidl";

// fbl::round_up() doesn't work on signed types.
template <typename T>
T AlignUp(T value, T divisor) {
  return (value + divisor - 1) / divisor * divisor;
}

// Helper function to build owned HeapProperties table with coherency domain support.
fuchsia_hardware_sysmem::HeapProperties BuildHeapPropertiesWithCoherencyDomainSupport(
    bool cpu_supported, bool ram_supported, bool inaccessible_supported, bool need_clear,
    bool need_flush) {
  using fuchsia_hardware_sysmem::CoherencyDomainSupport;
  using fuchsia_hardware_sysmem::HeapProperties;

  CoherencyDomainSupport coherency_domain_support;
  coherency_domain_support.cpu_supported().emplace(cpu_supported);
  coherency_domain_support.ram_supported().emplace(ram_supported);
  coherency_domain_support.inaccessible_supported().emplace(inaccessible_supported);

  HeapProperties heap_properties;
  heap_properties.coherency_domain_support().emplace(std::move(coherency_domain_support));
  heap_properties.need_clear().emplace(need_clear);
  heap_properties.need_flush().emplace(need_flush);
  return heap_properties;
}

class SystemRamMemoryAllocator : public MemoryAllocator {
 public:
  explicit SystemRamMemoryAllocator(Owner* parent_device)
      : MemoryAllocator(BuildHeapPropertiesWithCoherencyDomainSupport(
            true /*cpu*/, true /*ram*/, true /*inaccessible*/,
            // Zircon guarantees created VMO are filled with 0; sysmem doesn't
            // need to clear it once again.  There's little point in flushing a
            // demand-backed VMO that's only virtually filled with 0.
            /*need_clear=*/false, /*need_flush=*/false)) {
    node_ = parent_device->heap_node()->CreateChild("SysmemRamMemoryAllocator");
    node_.CreateUint("id", id(), &properties_);
  }

  zx_status_t Allocate(uint64_t raw_vmo_size, std::optional<uint64_t> min_physical_alignment,
                       const fuchsia_sysmem2::SingleBufferSettings& settings,
                       std::optional<std::string> name, uint64_t buffer_collection_id,
                       uint32_t buffer_index, zx::vmo* parent_vmo) override {
    ZX_DEBUG_ASSERT_MSG(raw_vmo_size % zx_system_get_page_size() == 0, "raw_vmo_size: 0x%" PRIx64,
                        raw_vmo_size);
    ZX_DEBUG_ASSERT_MSG(*settings.buffer_settings()->raw_vmo_size() == raw_vmo_size,
                        "settings raw_vmo_size: %" PRIu64 " raw_vmo_size: %" PRIu64,
                        *settings.buffer_settings()->raw_vmo_size(), raw_vmo_size);
    if (min_physical_alignment.has_value()) {
      // SystemRamMemoryAllocator can't allocate physically-contiguous.
      LOG(ERROR, "min_physical_alignment.has_value() unexpected");
      return ZX_ERR_NOT_SUPPORTED;
    }
    zx_status_t status = zx::vmo::create(raw_vmo_size, 0, parent_vmo);
    if (status != ZX_OK) {
      return status;
    }
    constexpr const char vmo_name[] = "Sysmem-core";
    parent_vmo->set_property(ZX_PROP_NAME, vmo_name, sizeof(vmo_name));
    return status;
  }

  void Delete(zx::vmo parent_vmo) override {
    // ~parent_vmo
  }
  // Since this allocator only allocates independent VMOs, it's fine to orphan those VMOs from the
  // allocator since the VMOs independently track what pages they're using.  So this allocator can
  // always claim is_empty() true.
  bool is_empty() override { return true; }

 private:
  inspect::Node node_;
  inspect::ValueList properties_;
};

class ContiguousSystemRamMemoryAllocator : public MemoryAllocator {
 public:
  explicit ContiguousSystemRamMemoryAllocator(Owner* parent_device)
      : MemoryAllocator(BuildHeapPropertiesWithCoherencyDomainSupport(
            /*cpu_supported=*/true, /*ram_supported=*/true,
            /*inaccessible_supported=*/true,
            // Zircon guarantees contagious VMO created are filled with 0;
            // sysmem doesn't need to clear it once again.  Unlike non-contiguous
            // VMOs which haven't backed pages yet, contiguous VMOs have backed
            // pages, and it's effective to flush the zeroes to RAM.  Some current
            // sysmem clients rely on contiguous allocations having their initial
            // zero-fill already flushed to RAM (at least for the RAM coherency
            // domain, this should probably remain true).
            /*need_clear=*/false, /*need_flush=*/true)),
        parent_device_(parent_device) {
    node_ = parent_device_->heap_node()->CreateChild("ContiguousSystemRamMemoryAllocator");
    node_.CreateUint("id", id(), &properties_);
  }

  zx_status_t Allocate(uint64_t raw_vmo_size, std::optional<uint64_t> min_physical_alignment,
                       const fuchsia_sysmem2::SingleBufferSettings& settings,
                       std::optional<std::string> name, uint64_t buffer_collection_id,
                       uint32_t buffer_index, zx::vmo* parent_vmo) override {
    ZX_DEBUG_ASSERT_MSG(raw_vmo_size % zx_system_get_page_size() == 0, "size: 0x%" PRIx64,
                        raw_vmo_size);
    ZX_DEBUG_ASSERT_MSG(*settings.buffer_settings()->raw_vmo_size() == raw_vmo_size,
                        "settings raw_vmo_size: %" PRIu64 " raw_vmo_size: %" PRIu64,
                        *settings.buffer_settings()->raw_vmo_size(), raw_vmo_size);
    uint32_t alignment_log2 = 0;
    if (min_physical_alignment.has_value()) {
      alignment_log2 = __builtin_ctzll(*min_physical_alignment);
    }
    zx::vmo result_parent_vmo;
    // This code is unlikely to work after running for a while and physical
    // memory is more fragmented than early during boot. The
    // ContiguousPooledMemoryAllocator handles that case by keeping
    // a separate pool of contiguous memory.
    zx_status_t status = zx::vmo::create_contiguous(parent_device_->bti(), raw_vmo_size,
                                                    alignment_log2, &result_parent_vmo);
    if (status != ZX_OK) {
      LOG(ERROR, "zx::vmo::create_contiguous() failed - raw_vmo_size: %" PRIu64 " status: %d",
          raw_vmo_size, status);
      // sanitize to ZX_ERR_NO_MEMORY regardless of why.
      status = ZX_ERR_NO_MEMORY;
      return status;
    }
    constexpr const char vmo_name[] = "Sysmem-contig-core";
    result_parent_vmo.set_property(ZX_PROP_NAME, vmo_name, sizeof(vmo_name));
    *parent_vmo = std::move(result_parent_vmo);
    return ZX_OK;
  }
  void Delete(zx::vmo parent_vmo) override {
    // ~vmo
  }
  // Since this allocator only allocates independent VMOs, it's fine to orphan those VMOs from the
  // allocator since the VMOs independently track what pages they're using.  So this allocator can
  // always claim is_empty() true.
  bool is_empty() override { return true; }

 private:
  Owner* const parent_device_;
  inspect::Node node_;
  inspect::ValueList properties_;
};

}  // namespace

Sysmem::Sysmem(async_dispatcher_t* client_dispatcher)
    : client_dispatcher_(client_dispatcher), loop_(&kAsyncLoopConfigNeverAttachToThread) {
  LOG(DEBUG, "Sysmem::Sysmem");
  std::lock_guard checker(client_checker_);

  zx_status_t start_thread_status = loop_.StartThread("sysmem-loop", &loop_thrd_);
  ZX_ASSERT_MSG(start_thread_status == ZX_OK, "loop start_thread_status: %s",
                zx_status_get_string(start_thread_status));
  RunSyncOnLoop([this] { loop_checker_.emplace(); });
}

Sysmem::~Sysmem() {
  std::lock_guard checker(client_checker_);
  Shutdown();
  LOG(DEBUG, "Finished Shutdown");
}

zx_status_t Sysmem::GetContiguousGuardParameters(const std::optional<sysmem_config::Config>& config,
                                                 uint64_t* guard_bytes_out,
                                                 bool* unused_pages_guarded,
                                                 int64_t* unused_guard_pattern_period_bytes,
                                                 zx::duration* unused_page_check_cycle_period,
                                                 bool* internal_guard_pages_out,
                                                 bool* crash_on_fail_out) {
  const uint64_t kDefaultGuardBytes = zx_system_get_page_size();
  *guard_bytes_out = kDefaultGuardBytes;
  *unused_page_check_cycle_period =
      ContiguousPooledMemoryAllocator::kDefaultUnusedPageCheckCyclePeriod;

  if (!config.has_value()) {
    *unused_pages_guarded = true;
    *internal_guard_pages_out = false;
    *crash_on_fail_out = false;
    *unused_guard_pattern_period_bytes = -1;
    return ZX_OK;
  }

  *crash_on_fail_out = config->contiguous_guard_pages_fatal();
  *internal_guard_pages_out = config->contiguous_guard_pages_internal();
  *unused_pages_guarded = config->contiguous_guard_pages_unused();
  // if this value is <= 0 it'll be ignored and the default of 1/128 will stay in effect
  *unused_guard_pattern_period_bytes =
      config->contiguous_guard_pages_unused_fraction_denominator() * zx_system_get_page_size();

  int64_t unused_page_check_cycle_seconds = config->contiguous_guard_pages_unused_cycle_seconds();
  if (unused_page_check_cycle_seconds > 0) {
    LOG(INFO, "Overriding unused page check period to %ld seconds",
        unused_page_check_cycle_seconds);
    *unused_page_check_cycle_period = zx::sec(unused_page_check_cycle_seconds);
  }

  int64_t guard_page_count = config->contiguous_guard_page_count();
  if (guard_page_count > 0) {
    LOG(INFO, "Overriding guard page count to %ld", guard_page_count);
    *guard_bytes_out = zx_system_get_page_size() * guard_page_count;
  }

  return ZX_OK;
}

void Sysmem::Shutdown() {
  // disconnect existing connections from drivers
  bindings_.RemoveAll();

  // Try to ensure there are no outstanding VMOS before shutting down the loop.
  PostTask([this]() mutable {
    std::lock_guard checker(*loop_checker_);

    // stop serving outgoing_ to prevent new connections
    outgoing_.reset();

    // disconnect existing allocators
    v1_allocators_.RemoveAll();
    v2_allocators_.RemoveAll();

    if (snapshot_annotation_register_.has_value()) {
      snapshot_annotation_register_->UnsetServiceDirectory();
      snapshot_annotation_register_.reset();
    }

    // Force-fail all LogicalBufferCollection(s) - this doesn't force them to all immediately
    // delete, but does get them headed toward deleting. The Fail() call may or may not delete a
    // LogicalBufferCollection synchronously, so copy the list before iterating.
    //
    // This disconnects all Node(s) (all BufferCollectionToken(s), BufferCollection(s),
    // BufferCollectionTokenGroup(s)).
    LogicalBufferCollections local_collections_list = logical_buffer_collections_;
    for (auto& collection : logical_buffer_collections_) {
      collection->Fail();
    }

    // Async notice when all still-existing LogicalBufferCollection(s) go away then shut down loop_.
    // This requires sysmem clients to close their remaining sysmem VMO handles to proceed async.
    //
    // If we decide in future that we want to be able to stop/delete the Sysmem without requiring
    // client VMOs to close first, we'll want to ensure that the securemem protocol and securemem
    // drivers can avoid orphaning protected_memory_size, while also leaving buffers HW-protected
    // until clients are done with the buffers - currently not a real issue since both sysmem and
    // securemem are effectively start-only per boot (outside of unit tests).
    waiting_for_unbind_ = true;
    CheckForUnbind();
  });

  // If a test is stuck waiting here, ensure that the test is dropping its sysmem VMO handles before
  // ~Sysmem.
  loop_.JoinThreads();
  loop_.Shutdown();

  LOG(DEBUG, "Finished Shutdown");
}

void Sysmem::CheckForUnbind() {
  std::lock_guard checker(*loop_checker_);
  if (!waiting_for_unbind_) {
    return;
  }
  if (!logical_buffer_collections().empty()) {
    LOG(INFO, "Not unbinding because there are logical buffer collections count %ld",
        logical_buffer_collections().size());
    return;
  }
  if (!!contiguous_system_ram_allocator_ && !contiguous_system_ram_allocator_->is_empty()) {
    LOG(INFO, "Not unbinding because contiguous system ram allocator is not empty");
    return;
  }
  for (auto& [heap, allocator] : allocators_) {
    if (!allocator->is_empty()) {
      LOG(INFO, "Not unbinding because allocator %s is not empty",
          heap.heap_type().value().c_str());

      return;
    }
  }

  // This will cause the loop thread to exit and will allow ~Sysmem to continue.
  loop_.Quit();
}

std::optional<SnapshotAnnotationRegister>& Sysmem::snapshot_annotation_register() {
  return snapshot_annotation_register_;
}

SysmemMetrics& Sysmem::metrics() { return metrics_; }

protected_ranges::ProtectedRangesCoreControl& Sysmem::protected_ranges_core_control(
    const fuchsia_sysmem2::Heap& heap) {
  std::lock_guard checker(*loop_checker_);
  auto iter = secure_mem_controls_.find(heap);
  ZX_DEBUG_ASSERT(iter != secure_mem_controls_.end());
  return iter->second;
}

bool Sysmem::SecureMemControl::IsDynamic() { return is_dynamic; }

uint64_t Sysmem::SecureMemControl::GetRangeGranularity() { return range_granularity; }

uint64_t Sysmem::SecureMemControl::MaxRangeCount() { return max_range_count; }

bool Sysmem::SecureMemControl::HasModProtectedRange() { return has_mod_protected_range; }

void Sysmem::SecureMemControl::AddProtectedRange(const protected_ranges::Range& range) {
  std::lock_guard checker(*parent->loop_checker_);
  ZX_DEBUG_ASSERT(parent->secure_mem_);
  fuchsia_sysmem2::SecureHeapAndRange secure_heap_and_range;
  secure_heap_and_range.heap() = heap;
  fuchsia_sysmem2::SecureHeapRange secure_heap_range;
  secure_heap_range.physical_address() = range.begin();
  secure_heap_range.size_bytes() = range.length();
  secure_heap_and_range.range() = std::move(secure_heap_range);
  fuchsia_sysmem2::SecureMemAddSecureHeapPhysicalRangeRequest add_request;
  add_request.heap_range() = std::move(secure_heap_and_range);
  auto result = parent->secure_mem_->channel()->AddSecureHeapPhysicalRange(std::move(add_request));
  // If we lose the ability to control protected memory ranges ... reboot.
  ZX_ASSERT_MSG(result.is_ok(), "AddSecureHeapPhysicalRange() failed: %s",
                result.error_value().FormatDescription().c_str());
}

void Sysmem::SecureMemControl::DelProtectedRange(const protected_ranges::Range& range) {
  std::lock_guard checker(*parent->loop_checker_);
  ZX_DEBUG_ASSERT(parent->secure_mem_);
  fuchsia_sysmem2::SecureHeapAndRange secure_heap_and_range;
  secure_heap_and_range.heap() = heap;
  fuchsia_sysmem2::SecureHeapRange secure_heap_range;
  secure_heap_range.physical_address() = range.begin();
  secure_heap_range.size_bytes() = range.length();
  secure_heap_and_range.range() = std::move(secure_heap_range);
  fuchsia_sysmem2::SecureMemDeleteSecureHeapPhysicalRangeRequest delete_request;
  delete_request.heap_range() = std::move(secure_heap_and_range);
  auto result =
      parent->secure_mem_->channel()->DeleteSecureHeapPhysicalRange(std::move(delete_request));
  // If we lose the ability to control protected memory ranges ... reboot.
  ZX_ASSERT_MSG(result.is_ok(), "DeleteSecureHeapPhysicalRange() failed: %s",
                result.error_value().FormatDescription().c_str());
}

void Sysmem::SecureMemControl::ModProtectedRange(const protected_ranges::Range& old_range,
                                                 const protected_ranges::Range& new_range) {
  if (new_range.end() != old_range.end() && new_range.begin() != old_range.begin()) {
    LOG(INFO,
        "new_range.end(): %" PRIx64 " old_range.end(): %" PRIx64 " new_range.begin(): %" PRIx64
        " old_range.begin(): %" PRIx64,
        new_range.end(), old_range.end(), new_range.begin(), old_range.begin());
    ZX_PANIC("INVALID RANGE MODIFICATION");
  }

  std::lock_guard checker(*parent->loop_checker_);
  ZX_DEBUG_ASSERT(parent->secure_mem_);
  fuchsia_sysmem2::SecureHeapAndRangeModification modification;
  modification.heap() = heap;
  fuchsia_sysmem2::SecureHeapRange range_old;
  range_old.physical_address() = old_range.begin();
  range_old.size_bytes() = old_range.length();
  fuchsia_sysmem2::SecureHeapRange range_new;
  range_new.physical_address() = new_range.begin();
  range_new.size_bytes() = new_range.length();
  modification.old_range() = std::move(range_old);
  modification.new_range() = std::move(range_new);
  fuchsia_sysmem2::SecureMemModifySecureHeapPhysicalRangeRequest mod_request;
  mod_request.range_modification() = std::move(modification);
  auto result =
      parent->secure_mem_->channel()->ModifySecureHeapPhysicalRange(std::move(mod_request));
  // If we lose the ability to control protected memory ranges ... reboot.
  ZX_ASSERT_MSG(result.is_ok(), "ModifySecureHeapPhysicalRange() failed: %s",
                result.error_value().FormatDescription().c_str());
}

void Sysmem::SecureMemControl::ZeroProtectedSubRange(bool is_covering_range_explicit,
                                                     const protected_ranges::Range& range) {
  std::lock_guard checker(*parent->loop_checker_);
  ZX_DEBUG_ASSERT(parent->secure_mem_);
  fuchsia_sysmem2::SecureHeapAndRange secure_heap_and_range;
  secure_heap_and_range.heap() = heap;
  fuchsia_sysmem2::SecureHeapRange secure_heap_range;
  secure_heap_range.physical_address() = range.begin();
  secure_heap_range.size_bytes() = range.length();
  secure_heap_and_range.range() = std::move(secure_heap_range);
  fuchsia_sysmem2::SecureMemZeroSubRangeRequest zero_request;
  zero_request.is_covering_range_explicit() = is_covering_range_explicit;
  zero_request.heap_range() = std::move(secure_heap_and_range);
  auto result = parent->secure_mem_->channel()->ZeroSubRange(std::move(zero_request));
  // If we lose the ability to control protected memory ranges ... reboot.
  ZX_ASSERT_MSG(result.is_ok(), "ZeroSubRange() failed: %s",
                result.error_value().FormatDescription().c_str());
}

zx::result<std::unique_ptr<Sysmem>> Sysmem::Create(async_dispatcher_t* client_dispatcher,
                                                   const Sysmem::CreateArgs& create_args) {
  LOG(DEBUG, "Create()");
  auto device = std::make_unique<Sysmem>(client_dispatcher);

  std::lock_guard checker(device->client_checker_);

  auto init_result = device->Initialize(create_args);
  if (!init_result.is_ok()) {
    LOG(ERROR, "Initialize() failed: %s\n", init_result.status_string());
    return init_result.take_error();
  }

  return zx::ok(std::move(device));
}

zx::result<> Sysmem::Initialize(const CreateArgs& create_args) {
  // always true outside unit tests
  if (create_args.set_scheduler_profile) {
    auto role_manager_result = component::Connect<fuchsia_scheduler::RoleManager>();
    if (!role_manager_result.is_ok()) {
      LOG(ERROR, "failed to connect to fuchsia_scheduler::RoleManager");
      return role_manager_result.take_error();
    }
    auto role_manager = fidl::SyncClient(std::move(*role_manager_result));
    ZX_DEBUG_ASSERT(role_manager.is_valid());
    auto set_thread_role = [&role_manager](const char* role_name,
                                           const zx::unowned_thread& thr) -> zx::result<> {
      zx::thread thr_dup;
      zx_status_t dup_status = thr->duplicate(ZX_RIGHT_SAME_RIGHTS, &thr_dup);
      if (dup_status != ZX_OK) {
        LOG(ERROR, "failed to duplicate thread handle for %s: %s", role_name,
            zx_status_get_string(dup_status));
        return zx::error(dup_status);
      }
      fuchsia_scheduler::RoleManagerSetRoleRequest set_role_request;
      set_role_request.target() = fuchsia_scheduler::RoleTarget::WithThread(std::move(thr_dup));
      set_role_request.role().emplace().role() = role_name;
      auto set_role_result = role_manager->SetRole(std::move(set_role_request));
      if (!set_role_result.is_ok()) {
        LOG(ERROR, "role_manager->SetRole() failed for thread %s: %s", role_name,
            set_role_result.error_value().FormatDescription().c_str());
        return zx::error(ZX_ERR_INTERNAL);
      }
      return zx::ok();
    };
    if (auto status = set_thread_role("fuchsia.sysmem", zx::thread::self()); status.is_error()) {
      return status.take_error();
    }
    if (auto status = set_thread_role("fuchsia.sysmem.loop",
                                      zx::unowned_thread(thrd_get_zx_handle(loop_thrd_)));
        status.is_error()) {
      return status.take_error();
    }
  }

  // Put everything under a node called "sysmem" because there's currently there's not a simple way
  // to distinguish (using a selector) which driver inspect information is coming from.
  sysmem_root_ = inspector_.inspector().GetRoot().CreateChild("sysmem");
  heaps_ = sysmem_root_.CreateChild("heaps");
  collections_node_ = sysmem_root_.CreateChild("collections");

  // this is always true outside of unit tests
  if (create_args.create_bti) {
    auto create_bti_result = CreateBti();
    if (!create_bti_result.is_ok()) {
      LOG(ERROR, "CreateBti() failed: %s", create_bti_result.status_string());
      return create_bti_result.take_error();
    }
    bti_ = std::move(create_bti_result.value());
    ZX_DEBUG_ASSERT(bti_.is_valid());
  } else {
    ZX_DEBUG_ASSERT(!bti_.is_valid());
  }

  auto config_from_file_result = GetConfigFromFile();
  if (!config_from_file_result.is_ok()) {
    LOG(WARNING, "sysmem-config - GetConfigFromFile() failed: %s",
        config_from_file_result.status_string());
    // fall back to default-initialized config
    config_from_file_result = zx::ok(fuchsia_sysmem2::Config{});
  }
  auto config_from_file = std::move(config_from_file_result.value());
  if (!config_from_file.format_costs().has_value()) {
    LOG(WARNING, "sysmem-config - missing format_costs");
    config_from_file.format_costs().emplace();
  } else {
    LOG(INFO, "sysmem-config - format_costs.size(): %zu", config_from_file.format_costs()->size());
  }
  usage_pixel_format_cost_.emplace(
      UsagePixelFormatCost(std::move(*config_from_file.format_costs())));

  int64_t protected_memory_size = kDefaultProtectedMemorySize;
  int64_t contiguous_memory_size = kDefaultContiguousMemorySize;

  if (!create_args.create_bti) {
    protected_memory_size = 0;
    contiguous_memory_size = 0;
  }

  std::optional<sysmem_config::Config> maybe_config;
  // this is always true outside of unit tests
  if (create_args.expect_structured_config) {
    sysmem_config::Config config = sysmem_config::Config::TakeFromStartupHandle();
    if (config.contiguous_memory_size() >= 0) {
      contiguous_memory_size = config.contiguous_memory_size();
    } else if (config.contiguous_memory_size_percent() >= 0 &&
               config.contiguous_memory_size_percent() <= 99) {
      // the negation is un-done below
      contiguous_memory_size = -config.contiguous_memory_size_percent();
    }
    if (config.protected_memory_size() >= 0) {
      protected_memory_size = config.protected_memory_size();
    } else if (config.protected_memory_size_percent() >= 0 &&
               config.protected_memory_size_percent() <= 99) {
      // the negation is un-done below
      protected_memory_size = -config.protected_memory_size_percent();
    }
    RunSyncOnLoop([this, &config] {
      std::lock_guard thread_checker(*loop_checker_);
      protected_ranges_disable_dynamic_ = config.protected_ranges_disable_dynamic();
    });
    maybe_config = std::move(config);
  }

  // Negative values are interpreted as a percentage of physical RAM.
  if (contiguous_memory_size < 0) {
    contiguous_memory_size = -contiguous_memory_size;
    ZX_DEBUG_ASSERT(contiguous_memory_size >= 1 && contiguous_memory_size <= 99);
    contiguous_memory_size = zx_system_get_physmem() * contiguous_memory_size / 100;
  }
  if (protected_memory_size < 0) {
    protected_memory_size = -protected_memory_size;
    ZX_DEBUG_ASSERT(protected_memory_size >= 1 && protected_memory_size <= 99);
    protected_memory_size = zx_system_get_physmem() * protected_memory_size / 100;
  }

  constexpr int64_t kMinProtectedAlignment = 64 * 1024;
  assert(kMinProtectedAlignment % zx_system_get_page_size() == 0);
  contiguous_memory_size =
      AlignUp(contiguous_memory_size, safe_cast<int64_t>(zx_system_get_page_size()));
  protected_memory_size = AlignUp(protected_memory_size, kMinProtectedAlignment);

  auto heap = sysmem::MakeHeap(bind_fuchsia_sysmem_heap::HEAP_TYPE_SYSTEM_RAM, 0);
  RunSyncOnLoop([this, &heap] {
    std::lock_guard thread_checker(*loop_checker_);
    allocators_[std::move(heap)] = std::make_unique<SystemRamMemoryAllocator>(this);
    snapshot_annotation_register_.emplace(loop_dispatcher());
    auto service_directory = sys::ServiceDirectory::CreateFromNamespace();
    snapshot_annotation_register_->SetServiceDirectory(service_directory, loop_dispatcher());
    metrics_.metrics_buffer().SetServiceDirectory(service_directory);
    metrics_.LogUnusedPageCheck(sysmem_metrics::UnusedPageCheckMetricDimensionEvent_Connectivity);
  });

  if (contiguous_memory_size) {
    constexpr bool kIsAlwaysCpuAccessible = true;
    constexpr bool kIsEverCpuAccessible = true;
    constexpr bool kIsEverZirconAccessible = true;
    constexpr bool kIsReady = true;
    constexpr bool kCanBeTornDown = true;
    auto heap = sysmem::MakeHeap(bind_fuchsia_sysmem_heap::HEAP_TYPE_SYSTEM_RAM, 0);
    auto pooled_allocator = std::make_unique<ContiguousPooledMemoryAllocator>(
        this, "SysmemContiguousPool", &heaps_, std::move(heap), contiguous_memory_size,
        kIsAlwaysCpuAccessible, kIsEverCpuAccessible, kIsEverZirconAccessible, kIsReady,
        kCanBeTornDown, loop_dispatcher());
    if (pooled_allocator->Init() != ZX_OK) {
      LOG(ERROR, "Contiguous system ram allocator initialization failed");
      return zx::error(ZX_ERR_NO_MEMORY);
    }
    uint64_t guard_region_size;
    bool unused_pages_guarded;
    int64_t unused_guard_pattern_period_bytes;
    zx::duration unused_page_check_cycle_period;
    bool internal_guard_regions;
    bool crash_on_guard;
    if (GetContiguousGuardParameters(maybe_config, &guard_region_size, &unused_pages_guarded,
                                     &unused_guard_pattern_period_bytes,
                                     &unused_page_check_cycle_period, &internal_guard_regions,
                                     &crash_on_guard) == ZX_OK) {
      pooled_allocator->InitGuardRegion(guard_region_size, unused_pages_guarded,
                                        unused_guard_pattern_period_bytes,
                                        unused_page_check_cycle_period, internal_guard_regions,
                                        crash_on_guard, loop_dispatcher());
    }
    pooled_allocator->SetupUnusedPages();
    RunSyncOnLoop([this, &pooled_allocator] {
      std::lock_guard thread_checker(*loop_checker_);
      contiguous_system_ram_allocator_ = std::move(pooled_allocator);
    });
  } else {
    RunSyncOnLoop([this] {
      std::lock_guard thread_checker(*loop_checker_);
      contiguous_system_ram_allocator_ = std::make_unique<ContiguousSystemRamMemoryAllocator>(this);
    });
  }

  // TODO: Separate protected memory allocator into separate driver or library
  if (protected_memory_size > 0) {
    constexpr bool kIsAlwaysCpuAccessible = false;
    constexpr bool kIsEverCpuAccessible = true;
    constexpr bool kIsEverZirconAccessible = true;
    constexpr bool kIsReady = false;
    // We have no way to tear down secure memory.
    constexpr bool kCanBeTornDown = false;
    // The heap is initially nullopt, but is set via set_heap before set_ready when we hear from
    // the secmem driver.
    auto protected_allocator = std::make_unique<ContiguousPooledMemoryAllocator>(
        this, "SysmemProtectedPool", &heaps_, /*heap=*/std::nullopt, protected_memory_size,
        kIsAlwaysCpuAccessible, kIsEverCpuAccessible, kIsEverZirconAccessible, kIsReady,
        kCanBeTornDown, loop_dispatcher());
    // Request at least 64kB alignment because some relevant hardware can only modify protections
    // along 64kB boundaries.
    //
    // Request 1MiB alignment because some relevant HW requires 1MiB alignment for some buffers.
    uint32_t alignment_log2 = __builtin_ctzl(1ul * 1024ul * 1024ul);
    zx_status_t status = protected_allocator->Init(alignment_log2);
    if (status != ZX_OK) {
      LOG(ERROR, "Failed to init allocator for protected/secure (DRM) memory: %d", status);
      return zx::error(status);
    }

    pending_protected_allocator_ = std::move(protected_allocator);
  }

  if (create_args.serve_outgoing) {
    auto begin_serving_result = SyncCall([this] {
      std::lock_guard lock(*loop_checker_);
      return BeginServing();
    });
    if (!begin_serving_result.is_ok()) {
      LOG(ERROR, "BeginServing() failed: %s", begin_serving_result.status_string());
      return begin_serving_result.take_error();
    }
  }

  if constexpr (kLogAllCollectionsPeriodically) {
    ZX_ASSERT(ZX_OK ==
              log_all_collections_.PostDelayed(loop_dispatcher(), kLogAllCollectionsInterval));
  }

  LOG(INFO, "sysmem finished initialization");

  return zx::ok();
}

zx::result<zx::bti> Sysmem::CreateBti() {
  auto iommu_client_result = component::Connect<fuchsia_kernel::IommuResource>();
  if (!iommu_client_result.is_ok()) {
    LOG(ERROR, "component::Connect<fuchsia_kernel::IommuResource>() failed: %s",
        iommu_client_result.status_string());
    return iommu_client_result.take_error();
  }
  auto iommu_sync_client = fidl::SyncClient(std::move(iommu_client_result.value()));
  auto iommu_result = iommu_sync_client->Get();
  if (!iommu_result.is_ok()) {
    LOG(ERROR, "mmio_sync_client->Get() failed: %s",
        iommu_result.error_value().FormatDescription().c_str());
    return zx::error(iommu_result.error_value().status());
  }
  auto iommu_resource = std::move(iommu_result.value().resource());
  zx_iommu_desc_stub_t stub_iommu_desc{};
  zx::iommu iommu;
  zx_status_t iommu_create_status = zx::iommu::create(
      iommu_resource, ZX_IOMMU_TYPE_STUB, &stub_iommu_desc, sizeof(stub_iommu_desc), &iommu);
  if (iommu_create_status != ZX_OK) {
    LOG(ERROR, "zx::iommu::create() failed: %s", zx_status_get_string(iommu_create_status));
    return zx::error(iommu_create_status);
  }
  zx::bti bti;
  zx_status_t bti_create_status = zx::bti::create(iommu, 0, /*bti_id=*/0, &bti);
  if (bti_create_status != ZX_OK) {
    LOG(ERROR, "zx::bti::create() failed: %s", zx_status_get_string(bti_create_status));
    return zx::error(bti_create_status);
  }
  return zx::ok(std::move(bti));
}

zx::result<> Sysmem::BeginServing() {
  // While outgoing_ is on loop_dispatcher(), the fuchsia_hardware_sysmem::Sysmem protocol is served
  // on client_dispatcher().
  outgoing_ = component::OutgoingDirectory(loop_dispatcher());

  auto add_allocator1_result = outgoing_->AddUnmanagedProtocol<fuchsia_sysmem::Allocator>(
      [this](fidl::ServerEnd<fuchsia_sysmem::Allocator> server_end) {
        Allocator::CreateOwnedV1(std::move(server_end), this, v1_allocators());
      });
  if (!add_allocator1_result.is_ok()) {
    LOG(ERROR, "AddUnmanagedProtocol<fuchsia_sysmem::Allocator>() failed: %s",
        add_allocator1_result.status_string());
    return add_allocator1_result.take_error();
  }

  auto add_allocator2_result = outgoing_->AddUnmanagedProtocol<fuchsia_sysmem2::Allocator>(
      [this](fidl::ServerEnd<fuchsia_sysmem2::Allocator> server_end) {
        Allocator::CreateOwnedV2(std::move(server_end), this, v2_allocators());
      });
  if (!add_allocator2_result.is_ok()) {
    LOG(ERROR, "AddUnmanagedProtocol<fuchsia_sysmem2::Allocator>() failed: %s",
        add_allocator2_result.status_string());
    return add_allocator2_result.take_error();
  }

  auto add_sysmem_result = outgoing_->AddUnmanagedProtocol<fuchsia_hardware_sysmem::Sysmem>(
      [this](fidl::ServerEnd<fuchsia_hardware_sysmem::Sysmem> server_end) {
        PostTaskToClientDispatcher([this, server_end = std::move(server_end)]() mutable {
          bindings_.AddBinding(client_dispatcher(), std::move(server_end), this,
                               fidl::kIgnoreBindingClosure);
        });
      });
  if (!add_sysmem_result.is_ok()) {
    LOG(ERROR, "AddUnmanagedProtocol<fuchsia_hardware_sysmem::Sysmem>() failed: %s",
        add_sysmem_result.status_string());
    return add_sysmem_result.take_error();
  }

  auto serve_result = outgoing_->ServeFromStartupInfo();
  if (!serve_result.is_ok()) {
    LOG(ERROR, "outgoing_->ServeFromStartupInfo() failed: %s", serve_result.status_string());
    return serve_result.take_error();
  }
  return zx::ok();
}

zx_status_t Sysmem::RegisterHeapInternal(
    fuchsia_sysmem2::Heap heap, fidl::ClientEnd<fuchsia_hardware_sysmem::Heap> heap_connection) {
  class EventHandler : public fidl::WireAsyncEventHandler<fuchsia_hardware_sysmem::Heap> {
   public:
    void OnRegister(
        ::fidl::WireEvent<::fuchsia_hardware_sysmem::Heap::OnRegister>* event) override {
      auto properties = fidl::ToNatural(event->properties);
      std::lock_guard checker(*device_->loop_checker_);
      // A heap should not be registered twice.
      ZX_DEBUG_ASSERT(heap_client_.is_valid());
      // This replaces any previously registered allocator for heap. This
      // behavior is preferred as it avoids a potential race-condition during
      // heap restart.
      auto allocator = std::make_shared<ExternalMemoryAllocator>(device_, std::move(heap_client_),
                                                                 std::move(properties));
      weak_associated_allocator_ = allocator;
      device_->allocators_[heap_] = std::move(allocator);
    }

    void on_fidl_error(fidl::UnbindInfo info) override {
      if (!info.is_peer_closed()) {
        LOG(ERROR, "Heap failed: %s\n", info.FormatDescription().c_str());
      }
    }

    // Clean up heap allocator after |heap_client_| tears down, but only if the
    // heap allocator for this |heap_| is still associated with this handler via
    // |weak_associated_allocator_|.
    ~EventHandler() override {
      std::lock_guard checker(*device_->loop_checker_);
      auto existing = device_->allocators_.find(heap_);
      if (existing != device_->allocators_.end() &&
          existing->second == weak_associated_allocator_.lock())
        device_->allocators_.erase(heap_);
    }

    static void Bind(Sysmem* device, fidl::ClientEnd<fuchsia_hardware_sysmem::Heap> heap_client_end,
                     fuchsia_sysmem2::Heap heap) {
      auto event_handler = std::unique_ptr<EventHandler>(new EventHandler(device, heap));
      event_handler->heap_client_.Bind(std::move(heap_client_end), device->loop_dispatcher(),
                                       std::move(event_handler));
    }

   private:
    EventHandler(Sysmem* device, fuchsia_sysmem2::Heap heap)
        : device_(device), heap_(std::move(heap)) {}

    Sysmem* const device_;
    fidl::WireSharedClient<fuchsia_hardware_sysmem::Heap> heap_client_;
    const fuchsia_sysmem2::Heap heap_;
    std::weak_ptr<ExternalMemoryAllocator> weak_associated_allocator_;
  };

  PostTask([this, heap = std::move(heap), heap_connection = std::move(heap_connection)]() mutable {
    std::lock_guard checker(*loop_checker_);
    EventHandler::Bind(this, std::move(heap_connection), std::move(heap));
  });
  return ZX_OK;
}

zx_status_t Sysmem::RegisterSecureMemInternal(
    fidl::ClientEnd<fuchsia_sysmem2::SecureMem> secure_mem_connection) {
  LOG(DEBUG, "sysmem RegisterSecureMem begin");

  current_close_is_abort_ = std::make_shared<std::atomic_bool>(true);

  PostTask([this, secure_mem_connection = std::move(secure_mem_connection),
            close_is_abort = current_close_is_abort_]() mutable {
    std::lock_guard checker(*loop_checker_);
    // This code must run asynchronously for two reasons:
    // 1) It does synchronous IPCs to the secure mem device, so SysmemRegisterSecureMem must
    // have return so the call from the secure mem device is unblocked.
    // 2) It modifies member variables like |secure_mem_| and |heaps_| that should only be
    // touched on |loop_|'s thread.
    auto wait_for_close = std::make_unique<async::Wait>(
        secure_mem_connection.channel().get(), ZX_CHANNEL_PEER_CLOSED, 0,
        async::Wait::Handler([this, close_is_abort](async_dispatcher_t* loop_dispatcher,
                                                    async::Wait* wait, zx_status_t status,
                                                    const zx_packet_signal_t* signal) {
          std::lock_guard checker(*loop_checker_);
          if (*close_is_abort && secure_mem_) {
            // The server end of this channel (the securemem driver) is the driver that
            // listens for suspend(mexec) so that soft reboot can succeed.  If that driver has
            // failed, intentionally force a hard reboot here to get back to a known-good state.
            //
            // TODO(https://fxbug.dev/42180331): When there's any more direct/immediate way to
            // intentionally trigger a hard reboot, switch to that (or just remove this TODO
            // when sysmem terminating directly leads to a hard reboot).
            ZX_PANIC(
                "secure_mem_ connection unexpectedly lost; secure mem in unknown state; hard "
                "reboot");
          }
        }));

    // It is safe to call Begin() here before setting up secure_mem_ because handler will either
    // run on current thread (loop_thrd_), or be run after the current task finishes while the
    // loop is shutting down.
    zx_status_t status = wait_for_close->Begin(loop_dispatcher());
    if (status != ZX_OK) {
      LOG(ERROR, "Sysmem::RegisterSecureMem() failed wait_for_close->Begin()");
      return;
    }

    secure_mem_ = std::make_unique<SecureMemConnection>(std::move(secure_mem_connection),
                                                        std::move(wait_for_close));

    // Else we already ZX_PANIC()ed in wait_for_close.
    ZX_DEBUG_ASSERT(secure_mem_);

    // At this point pending_protected_vmo_ will have the protected_memory_size VMO that was
    // allocated during sysmem driver Start (if protected_memory_size != 0), and secure_allocators_
    // has no heaps yet. The pending_protected_vmo_ is not yet protected, but it is physically
    // contiguous. We get the Heap identity and SecureMem properties and set up the heap in
    // secure_allocators_. This location of this heap is determined by the VMO allocation, not by
    // the TEE.
    if (pending_protected_allocator_) {
      auto dynamic_heaps_result = secure_mem_->channel()->GetDynamicSecureHeaps();
      if (!dynamic_heaps_result.is_ok()) {
        LOG(WARNING, "GetDynamicSecureHeaps failed: %s",
            dynamic_heaps_result.error_value().FormatDescription().c_str());
        return;
      }
      auto dynamic_heaps = std::move(dynamic_heaps_result.value());
      if (!dynamic_heaps.heaps().has_value() || dynamic_heaps.heaps()->empty()) {
        LOG(WARNING, "protected_memory_size was set, but missing dynamic heap");
        return;
      }
      // heap field is required; fatal if not present
      auto heap = std::move(dynamic_heaps.heaps()->at(0).heap().value());
      pending_protected_allocator_->set_heap(heap);
      secure_allocators_[heap] = pending_protected_allocator_.get();
      allocators_[std::move(heap)] = std::move(pending_protected_allocator_);
    }

    for (const auto& [heap, allocator] : secure_allocators_) {
      uint64_t phys_base;
      uint64_t size_bytes;
      zx_status_t get_status = allocator->GetPhysicalMemoryInfo(&phys_base, &size_bytes);
      if (get_status != ZX_OK) {
        LOG(WARNING, "get_status != ZX_OK - get_status: %d", get_status);
        return;
      }
      fuchsia_sysmem2::SecureHeapAndRange whole_heap;
      whole_heap.heap() = heap;
      fuchsia_sysmem2::SecureHeapRange range;
      range.physical_address() = phys_base;
      range.size_bytes() = size_bytes;
      whole_heap.range() = std::move(range);
      fuchsia_sysmem2::SecureMemGetPhysicalSecureHeapPropertiesRequest get_props_request;
      get_props_request.entire_heap() = std::move(whole_heap);
      auto get_properties_result =
          secure_mem_->channel()->GetPhysicalSecureHeapProperties(std::move(get_props_request));
      if (!get_properties_result.is_ok()) {
        if (get_properties_result.error_value().is_framework_error()) {
          // For now this is fatal unless explicitly unregistered, since this case is very
          // unexpected, and in this case rebooting is the most plausible way to get back to a
          // working state anyway.
          ZX_ASSERT(!*close_is_abort);
        }
        LOG(WARNING, "GetPhysicalSecureHeapProperties() failed: %s",
            get_properties_result.error_value().FormatDescription().c_str());
        // Don't call set_ready() on secure_allocators_.  Eg. this can happen if securemem TA is
        // not found.
        return;
      }
      // properties field is required; fatal if not present
      const fuchsia_sysmem2::SecureHeapProperties properties =
          std::move(get_properties_result->properties().value());
      ZX_ASSERT(properties.heap().has_value());
      ZX_ASSERT(properties.heap() == heap);
      ZX_ASSERT(properties.dynamic_protection_ranges().has_value());
      ZX_ASSERT(properties.protected_range_granularity().has_value());
      ZX_ASSERT(properties.max_protected_range_count().has_value());
      ZX_ASSERT(properties.is_mod_protected_range_available().has_value());
      SecureMemControl control;
      control.heap = heap;
      control.parent = this;
      control.is_dynamic = properties.dynamic_protection_ranges().value();
      control.max_range_count = properties.max_protected_range_count().value();
      control.range_granularity = properties.protected_range_granularity().value();
      control.has_mod_protected_range = properties.is_mod_protected_range_available().value();
      secure_mem_controls_.emplace(heap, std::move(control));
    }

    // Now we get the secure heaps that are configured via the TEE.
    auto get_result = secure_mem_->channel()->GetPhysicalSecureHeaps();
    if (!get_result.is_ok()) {
      if (get_result.error_value().is_framework_error()) {
        // For now this is fatal unless explicitly unregistered, since this case is very
        // unexpected, and in this case rebooting is the most plausible way to get back to a
        // working state anyway.
        ZX_ASSERT(!*close_is_abort);
      }
      LOG(WARNING, "GetPhysicalSecureHeaps() failed: %s",
          get_result.error_value().FormatDescription().c_str());
      // Don't call set_ready() on secure_allocators_.  Eg. this can happen if securemem TA is
      // not found.
      return;
    }
    // heaps field is required; fatal if not present
    auto heaps = std::move(get_result->heaps().value());
    ZX_ASSERT(heaps.size() != 0);
    for (const auto& heap : heaps) {
      ZX_ASSERT(heap.heap().has_value());
      ZX_ASSERT(heap.ranges().has_value());
      // A tee-configured heap with multiple ranges can be specified by the protocol but is not
      // currently supported by sysmem.
      ZX_ASSERT(heap.ranges()->size() == 1);
      // For now we assume that all TEE-configured heaps are protected full-time, and that they
      // start fully protected.
      constexpr bool kIsAlwaysCpuAccessible = false;
      constexpr bool kIsEverCpuAccessible = false;
      constexpr bool kIsEverZirconAccessible = false;
      constexpr bool kIsReady = false;
      constexpr bool kCanBeTornDown = true;
      const fuchsia_sysmem2::SecureHeapRange& heap_range = heap.ranges()->at(0);
      const fuchsia_sysmem2::Heap& which_heap = heap.heap().value();
      auto secure_allocator = std::make_unique<ContiguousPooledMemoryAllocator>(
          this, "tee_secure", &heaps_, which_heap, heap_range.size_bytes().value(),
          kIsAlwaysCpuAccessible, kIsEverCpuAccessible, kIsEverZirconAccessible, kIsReady,
          kCanBeTornDown, loop_dispatcher());
      status = secure_allocator->InitPhysical(heap_range.physical_address().value());
      // A failing status is fatal for now.
      ZX_ASSERT_MSG(status == ZX_OK, "%s", zx_status_get_string(status));
      LOG(DEBUG,
          "created secure allocator: heap_type: %s heap_id: %" PRId64 " base: %016" PRIx64
          " size: %016" PRIx64,
          which_heap.heap_type().value().c_str(), which_heap.id().value(),
          heap_range.physical_address().value(), heap_range.size_bytes().value());

      // The only usage of SecureMemControl for a TEE-configured heap is to do ZeroSubRange(),
      // so field values of this SecureMemControl are somewhat degenerate (eg. VDEC).
      SecureMemControl control;
      control.heap = which_heap;
      control.parent = this;
      control.is_dynamic = false;
      control.max_range_count = 0;
      control.range_granularity = 0;
      control.has_mod_protected_range = false;
      secure_mem_controls_.emplace(which_heap, std::move(control));

      ZX_ASSERT(secure_allocators_.find(which_heap) == secure_allocators_.end());
      secure_allocators_[which_heap] = secure_allocator.get();
      ZX_ASSERT(allocators_.find(which_heap) == allocators_.end());
      allocators_[std::move(which_heap)] = std::move(secure_allocator);
    }

    for (const auto& [heap_type, allocator] : secure_allocators_) {
      // The secure_mem_ connection is ready to protect ranges on demand to cover this heap's
      // used ranges.  There are no used ranges yet since the heap wasn't ready until now.
      allocator->set_ready();
    }

    is_secure_mem_ready_ = true;
    was_secure_mem_ready_ = true;

    // At least for now, we just call all the LogicalBufferCollection(s), regardless of which
    // are waiting on secure mem (if any). The extra calls are required (by semantics of
    // OnDependencyReady) to not be harmful from a correctness point of view. If any are waiting
    // on secure mem, those can now proceed, and will do so using the current thread (the loop_
    // thread).
    ForEachLogicalBufferCollection([](LogicalBufferCollection* logical_buffer_collection) {
      logical_buffer_collection->OnDependencyReady();
    });

    LOG(DEBUG, "sysmem RegisterSecureMem() done (async)");
  });
  return ZX_OK;
}

// This call allows us to tell the difference between expected vs. unexpected close of the tee_
// channel.
zx_status_t Sysmem::UnregisterSecureMemInternal() {
  // By this point, the securemem driver's suspend(mexec) has already prepared for mexec.
  //
  // In this path, the server end of the channel hasn't closed yet, but will be closed shortly after
  // return from UnregisterSecureMem().
  //
  // We set a flag here so that a PEER_CLOSED of the channel won't cause the wait handler to crash.
  *current_close_is_abort_ = false;
  current_close_is_abort_.reset();
  PostTask([this]() {
    std::lock_guard checker(*loop_checker_);
    LOG(DEBUG, "begin UnregisterSecureMem()");
    secure_mem_.reset();
    for (const auto& [heap_type, allocator] : secure_allocators_) {
      allocator->clear_ready();
    }
    is_secure_mem_ready_ = false;
    LOG(DEBUG, "end UnregisterSecureMem()");
  });
  return ZX_OK;
}

const zx::bti& Sysmem::bti() { return bti_; }

// Only use this in cases where we really can't use zx::vmo::create_contiguous() because we must
// specify a specific physical range.
zx::result<zx::vmo> Sysmem::CreatePhysicalVmo(uint64_t base, uint64_t size) {
  // This isn't called much, so get the mmio resource each time rather than caching it.
  zx::result resource_result = component::Connect<fuchsia_kernel::MmioResource>();
  if (resource_result.is_error()) {
    LOG(ERROR, "Connect<fuchsia_kernel::MmioResource>() failed: %s",
        resource_result.status_string());
    return resource_result.take_error();
  }
  auto resource_protocol = fidl::SyncClient(std::move(resource_result).value());
  auto get_result = resource_protocol->Get();
  if (!get_result.is_ok()) {
    LOG(ERROR, "resource->Get() failed: %s", get_result.error_value().FormatDescription().c_str());
    return zx::error(get_result.error_value().status());
  }
  auto resource = std::move(std::move(get_result).value().resource());

  zx::vmo result_vmo;
  zx_status_t status = zx::vmo::create_physical(resource, base, size, &result_vmo);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(result_vmo));
}

void Sysmem::TrackToken(BufferCollectionToken* token) {
  std::lock_guard checker(*loop_checker_);
  ZX_DEBUG_ASSERT(token->has_server_koid());
  zx_koid_t server_koid = token->server_koid();
  ZX_DEBUG_ASSERT(server_koid != ZX_KOID_INVALID);
  ZX_DEBUG_ASSERT(tokens_by_koid_.find(server_koid) == tokens_by_koid_.end());
  tokens_by_koid_.insert({server_koid, token});
}

void Sysmem::UntrackToken(BufferCollectionToken* token) {
  std::lock_guard checker(*loop_checker_);
  if (!token->has_server_koid()) {
    // The caller is allowed to un-track a token that never saw
    // OnServerKoid().
    return;
  }
  // This is intentionally idempotent, to allow un-tracking from
  // BufferCollectionToken::CloseChannel() as well as from
  // ~BufferCollectionToken().
  tokens_by_koid_.erase(token->server_koid());
}

bool Sysmem::TryRemoveKoidFromUnfoundTokenList(zx_koid_t token_server_koid) {
  std::lock_guard checker(*loop_checker_);
  // unfound_token_koids_ is limited to kMaxUnfoundTokenCount (and likely empty), so a loop over it
  // should be efficient enough.
  for (auto it = unfound_token_koids_.begin(); it != unfound_token_koids_.end(); ++it) {
    if (*it == token_server_koid) {
      unfound_token_koids_.erase(it);
      return true;
    }
  }
  return false;
}

BufferCollectionToken* Sysmem::FindTokenByServerChannelKoid(zx_koid_t token_server_koid) {
  std::lock_guard checker(*loop_checker_);
  auto iter = tokens_by_koid_.find(token_server_koid);
  if (iter == tokens_by_koid_.end()) {
    if (token_server_koid != 0) {
      unfound_token_koids_.push_back(token_server_koid);
      constexpr uint32_t kMaxUnfoundTokenCount = 8;
      while (unfound_token_koids_.size() > kMaxUnfoundTokenCount) {
        unfound_token_koids_.pop_front();
      }
    }
    return nullptr;
  }
  return iter->second;
}

Sysmem::FindLogicalBufferByVmoKoidResult Sysmem::FindLogicalBufferByVmoKoid(zx_koid_t vmo_koid) {
  auto iter = vmo_koids_.find(vmo_koid);
  if (iter == vmo_koids_.end()) {
    return {nullptr, false};
  }
  return iter->second;
}

MemoryAllocator* Sysmem::GetAllocator(const fuchsia_sysmem2::BufferMemorySettings& settings) {
  std::lock_guard checker(*loop_checker_);
  if (*settings.heap()->heap_type() == bind_fuchsia_sysmem_heap::HEAP_TYPE_SYSTEM_RAM &&
      *settings.is_physically_contiguous()) {
    return contiguous_system_ram_allocator_.get();
  }

  auto iter = allocators_.find(*settings.heap());
  if (iter == allocators_.end()) {
    return nullptr;
  }
  return iter->second.get();
}

const fuchsia_hardware_sysmem::HeapProperties* Sysmem::GetHeapProperties(
    const fuchsia_sysmem2::Heap& heap) const {
  std::lock_guard checker(*loop_checker_);
  auto iter = allocators_.find(heap);
  if (iter == allocators_.end()) {
    return nullptr;
  }
  return &iter->second->heap_properties();
}

void Sysmem::AddVmoKoid(zx_koid_t koid, bool is_weak, LogicalBuffer& logical_buffer) {
  vmo_koids_.insert({koid, {&logical_buffer, is_weak}});
}

void Sysmem::RemoveVmoKoid(zx_koid_t koid) {
  // May not be present if ~TrackedParentVmo called in error path prior to being fully set up.
  vmo_koids_.erase(koid);
}

void Sysmem::LogAllBufferCollections() {
  std::lock_guard checker(*loop_checker_);
  IndentTracker indent_tracker(0);
  auto indent = indent_tracker.Current();
  LOG(INFO, "%*scollections.size: %" PRId64, indent.num_spaces(), "",
      logical_buffer_collections_.size());
  // We sort by create time to make it easier to figure out which VMOs are likely leaks, especially
  // when running with kLogAllCollectionsPeriodically true.
  std::vector<LogicalBufferCollection*> sort_by_create_time;
  ForEachLogicalBufferCollection(
      [&sort_by_create_time](LogicalBufferCollection* logical_buffer_collection) {
        sort_by_create_time.push_back(logical_buffer_collection);
      });
  struct CompareByCreateTime {
    bool operator()(const LogicalBufferCollection* a, const LogicalBufferCollection* b) {
      return a->create_time_monotonic() < b->create_time_monotonic();
    }
  };
  std::sort(sort_by_create_time.begin(), sort_by_create_time.end(), CompareByCreateTime{});
  for (auto* logical_buffer_collection : sort_by_create_time) {
    logical_buffer_collection->LogSummary(indent_tracker);
    // let output catch up so we don't drop log lines, hopefully; if there were a way to flush/wait
    // the logger to avoid dropped lines, we could do that instead
    zx::nanosleep(zx::deadline_after(zx::msec(20)));
  };
}

void Sysmem::ForeachSecureHeap(fit::function<bool(const fuchsia_sysmem2::Heap&)> callback) {
  std::lock_guard checker(*loop_checker_);
  for (auto& allocator : secure_allocators_) {
    bool keep_going = callback(allocator.first);
    if (!keep_going) {
      break;
    }
  }
}

Sysmem::SecureMemConnection::SecureMemConnection(
    fidl::ClientEnd<fuchsia_sysmem2::SecureMem> channel,
    std::unique_ptr<async::Wait> wait_for_close)
    : connection_(std::move(channel)), wait_for_close_(std::move(wait_for_close)) {
  // nothing else to do here
}

const fidl::SyncClient<fuchsia_sysmem2::SecureMem>& Sysmem::SecureMemConnection::channel() const {
  ZX_DEBUG_ASSERT(connection_);
  return connection_;
}

void Sysmem::LogCollectionsTimer(async_dispatcher_t* loop_dispatcher, async::TaskBase* task,
                                 zx_status_t status) {
  std::lock_guard checker(*loop_checker_);
  LogAllBufferCollections();
  ZX_ASSERT(kLogAllCollectionsPeriodically);
  ZX_ASSERT(ZX_OK == log_all_collections_.PostDelayed(loop_dispatcher, kLogAllCollectionsInterval));
}

void Sysmem::RegisterHeap(RegisterHeapRequest& request, RegisterHeapCompleter::Sync& completer) {
  std::lock_guard checker(client_checker_);
  // TODO(b/316646315): Change RegisterHeap to specify fuchsia_sysmem2::Heap, and remove the
  // conversion here.
  auto v2_heap_type_result =
      sysmem::V2CopyFromV1HeapType(static_cast<fuchsia_sysmem::HeapType>(request.heap()));
  if (!v2_heap_type_result.is_ok()) {
    LOG(WARNING, "V2CopyFromV1HeapType failed");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto& v2_heap_type = v2_heap_type_result.value();
  auto heap = sysmem::MakeHeap(std::move(v2_heap_type), 0);
  zx_status_t status = RegisterHeapInternal(std::move(heap), std::move(request.heap_connection()));
  if (status != ZX_OK) {
    LOG(WARNING, "CommonSysmemRegisterHeap failed");
    completer.Close(status);
    return;
  }
}

void Sysmem::RegisterSecureMem(RegisterSecureMemRequest& request,
                               RegisterSecureMemCompleter::Sync& completer) {
  std::lock_guard checker(client_checker_);
  zx_status_t status = RegisterSecureMemInternal(std::move(request.secure_mem_connection()));
  if (status != ZX_OK) {
    completer.Close(status);
    return;
  }
}

void Sysmem::UnregisterSecureMem(UnregisterSecureMemCompleter::Sync& completer) {
  std::lock_guard checker(client_checker_);
  zx_status_t status = UnregisterSecureMemInternal();
  if (status == ZX_OK) {
    completer.Reply(fit::ok());
  } else {
    completer.Reply(fit::error(status));
  }
}

zx::result<fuchsia_sysmem2::Config> Sysmem::GetConfigFromFile() {
  int config_fd = open(kSysmemConfigFilename, O_RDONLY);
  if (config_fd == -1) {
    int local_errno = errno;
    LOG(WARNING, "open(kSysmemConfigFilename) failed: %d", local_errno);
    return zx::error(ZX_ERR_INTERNAL);
  }
  auto close_config_fd = fit::defer([&config_fd] { close(config_fd); });
  struct stat config_stat;
  int fstat_result = fstat(config_fd, &config_stat);
  if (fstat_result != 0) {
    int local_errno = errno;
    LOG(WARNING, "fstat(config_fd) failed: %d", local_errno);
    return zx::error(ZX_ERR_INTERNAL);
  }
  off_t size = config_stat.st_size;
  std::vector<uint8_t> config_bytes(size);
  ssize_t read_result = read(config_fd, config_bytes.data(), config_bytes.size());
  if (read_result < 0) {
    int local_errno = errno;
    LOG(WARNING, "read(config_fd) failed: %d", local_errno);
    return zx::error(ZX_ERR_INTERNAL);
  }
  if (static_cast<size_t>(read_result) != config_bytes.size()) {
    LOG(WARNING, "read(config_fd) didn't read exact size");
    return zx::error(ZX_ERR_INTERNAL);
  }
  // Decode the FIDL struct
  fit::result result = fidl::Unpersist<fuchsia_sysmem2::Config>(config_bytes);
  ZX_ASSERT_MSG(result.is_ok(), "Could not decode fuchsia.sysmem2.Config FIDL structure");
  fuchsia_sysmem2::Config fidl_config = std::move(result.value());
  return zx::ok(fidl_config);
}

}  // namespace sysmem_service
