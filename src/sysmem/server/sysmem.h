// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_SYSMEM_H_
#define SRC_SYSMEM_SERVER_SYSMEM_H_

#include <fidl/fuchsia.hardware.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/wait.h>
#include <lib/closure-queue/closure_queue.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/fit/thread_checker.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/bti.h>
#include <lib/zx/channel.h>

#include <limits>
#include <map>
#include <memory>
#include <unordered_set>

#include <fbl/vector.h>
#include <region-alloc/region-alloc.h>

#include "src/sysmem/server/memory_allocator.h"
#include "src/sysmem/server/snapshot_annotation_register.h"
#include "src/sysmem/server/sysmem_config.h"
#include "src/sysmem/server/sysmem_metrics.h"
#include "src/sysmem/server/usage_pixel_format_cost.h"

namespace sys {
class ServiceDirectory;
}  // namespace sys

namespace sysmem_service {

class Sysmem;
class BufferCollectionToken;
class LogicalBuffer;
class LogicalBufferCollection;
class Node;
class ContiguousPooledMemoryAllocator;

struct Settings {
  // Maximum size of a single allocation. Mainly useful for unit tests.
  uint64_t max_allocation_size = UINT64_MAX;
};

// The fuchsia_hardware_sysmem::Sysmem protocol is used by the securemem driver and by external
// heaps such as goldfish.
class Sysmem final : public MemoryAllocator::Owner,
                     public fidl::Server<fuchsia_hardware_sysmem::Sysmem> {
 public:
  struct CreateArgs {
    bool create_bti = false;
    bool expect_structured_config = false;
    bool serve_outgoing = false;
    bool set_scheduler_profile = false;
  };
  static zx::result<std::unique_ptr<Sysmem>> Create(async_dispatcher_t* dispatcher,
                                                    const CreateArgs& create_args);

  // Use Create() instead.
  Sysmem(async_dispatcher_t* dispatcher);

  ~Sysmem() __TA_REQUIRES(client_checker_);

  // currently public only for tests
  [[nodiscard]] zx::result<> Initialize(const CreateArgs& create_args)
      __TA_REQUIRES(client_checker_);

  zx::result<zx::resource> GetInfoResource();

  [[nodiscard]] zx_status_t GetContiguousGuardParameters(
      const std::optional<sysmem_config::Config>& config, uint64_t* guard_bytes_out,
      bool* unused_pages_guarded, int64_t* unused_guard_pattern_period_bytes,
      zx::duration* unused_page_check_cycle_period, bool* internal_guard_pages_out,
      bool* crash_on_fail_out);

  //
  // The rest of the methods are only valid to call after Bind().
  //

  [[nodiscard]] zx_status_t RegisterHeapInternal(
      fuchsia_sysmem2::Heap heap, fidl::ClientEnd<fuchsia_hardware_sysmem::Heap> heap_connection)
      __TA_REQUIRES(client_checker_);
  // Also called directly by a test.
  [[nodiscard]] zx_status_t RegisterSecureMemInternal(
      fidl::ClientEnd<fuchsia_sysmem2::SecureMem> secure_mem_connection)
      __TA_REQUIRES(client_checker_);
  // Also called directly by a test.
  [[nodiscard]] zx_status_t UnregisterSecureMemInternal() __TA_REQUIRES(client_checker_);

  // MemoryAllocator::Owner implementation.
  [[nodiscard]] const zx::bti& bti() override;
  [[nodiscard]] zx::result<zx::vmo> CreatePhysicalVmo(uint64_t base, uint64_t size) override;
  void CheckForUnbind() override;
  std::optional<SnapshotAnnotationRegister>& snapshot_annotation_register() override;
  SysmemMetrics& metrics() override;
  protected_ranges::ProtectedRangesCoreControl& protected_ranges_core_control(
      const fuchsia_sysmem2::Heap& heap) override;

  inspect::Node* heap_node() override { return &heaps_; }

  zx::result<> BeginServing() __TA_REQUIRES(*loop_checker_);

  // fuchsia_hardware_sysmem::Sysmem impl; these run on client_dispatcher_.
  void RegisterHeap(RegisterHeapRequest& request, RegisterHeapCompleter::Sync& completer) override;
  void RegisterSecureMem(RegisterSecureMemRequest& request,
                         RegisterSecureMemCompleter::Sync& completer) override;
  void UnregisterSecureMem(UnregisterSecureMemCompleter::Sync& completer) override;

  // Track/untrack the token by the koid of the server end of its FIDL channel. TrackToken() is only
  // allowed after/during token->OnServerKoid(). UntrackToken() is allowed even if there was never a
  // token->OnServerKoid() (in which case it's a nop).
  //
  // While tracked, a token can be found with FindTokenByServerChannelKoid().
  void TrackToken(BufferCollectionToken* token);
  void UntrackToken(BufferCollectionToken* token);

  // Finds and removes token_server_koid from unfound_token_koids_.
  [[nodiscard]] bool TryRemoveKoidFromUnfoundTokenList(zx_koid_t token_server_koid);

  // Find the BufferCollectionToken (if any) by the koid of the server end of its FIDL channel.
  [[nodiscard]] BufferCollectionToken* FindTokenByServerChannelKoid(zx_koid_t token_server_koid);

  struct FindLogicalBufferByVmoKoidResult {
    LogicalBuffer* logical_buffer;
    bool is_koid_of_weak_vmo;
  };
  [[nodiscard]] FindLogicalBufferByVmoKoidResult FindLogicalBufferByVmoKoid(zx_koid_t vmo_koid);

  // Get allocator for |settings|. Returns NULL if allocator is not registered for settings.
  [[nodiscard]] MemoryAllocator* GetAllocator(
      const fuchsia_sysmem2::BufferMemorySettings& settings);

  // Get heap properties of a specific memory heap allocator.
  //
  // If the heap is not valid or not registered to sysmem driver, nullptr is returned.
  [[nodiscard]] const fuchsia_hardware_sysmem::HeapProperties* GetHeapProperties(
      const fuchsia_sysmem2::Heap& heap) const;

  // This is the dispatcher we run most of sysmem on (at least for now).
  [[nodiscard]] async_dispatcher_t* loop_dispatcher() { return loop_.dispatcher(); }
  [[nodiscard]] async_dispatcher_t* client_dispatcher() { return client_dispatcher_; }

  [[nodiscard]] std::unordered_set<LogicalBufferCollection*>& logical_buffer_collections() {
    std::lock_guard checker(*loop_checker_);
    return logical_buffer_collections_;
  }

  void AddLogicalBufferCollection(LogicalBufferCollection* collection) {
    std::lock_guard checker(*loop_checker_);
    logical_buffer_collections_.insert(collection);
  }

  void RemoveLogicalBufferCollection(LogicalBufferCollection* collection) {
    std::lock_guard checker(*loop_checker_);
    logical_buffer_collections_.erase(collection);
    CheckForUnbind();
  }

  void AddVmoKoid(zx_koid_t koid, bool is_weak, LogicalBuffer& logical_buffer);
  void RemoveVmoKoid(zx_koid_t koid);

  [[nodiscard]] inspect::Node& collections_node() { return collections_node_; }

  void set_settings(const Settings& settings) { settings_ = settings; }

  [[nodiscard]] const Settings& settings() const { return settings_; }

  void ResetThreadCheckerForTesting() { loop_checker_.emplace(); }

  bool protected_ranges_disable_dynamic() const override {
    std::lock_guard checker(*loop_checker_);
    return protected_ranges_disable_dynamic_;
  }

  // false - no secure heaps are expected to exist
  // true - secure heaps are expected to exist (regardless of whether any of them currently exist)
  bool is_secure_mem_expected() const {
    std::lock_guard checker(*loop_checker_);
    // We can base this on pending_protected_allocator_ || secure_allocators_ non-empty() since
    // there will be at least one secure allocator created before any clients can connect iff there
    // will be any secure heaps available. A true result here does not imply that all secure heaps
    // are already present and ready. For that, use is_secure_mem_ready().
    return pending_protected_allocator_ || !secure_allocators_.empty();
  }

  // false - secure mem is expected, but is not yet ready
  //
  // true - secure mem is not expected (and is therefore as ready as it will ever be / ready in the
  // "secure mem system is ready for allocation requests" sense), or secure mem is expected and
  // ready.
  bool is_secure_mem_ready() const {
    std::lock_guard checker(*loop_checker_);
    if (!is_secure_mem_expected()) {
      // attempts to use secure mem can go ahead and try to allocate and fail to allocate, so this
      // means "as ready
      return true;
    }
    return is_secure_mem_ready_;
  }

  bool was_secure_mem_ready() const {
    std::lock_guard checker(*loop_checker_);
    if (!is_secure_mem_expected()) {
      return true;
    }
    return was_secure_mem_ready_;
  }

  template <typename F>
  void PostTask(F to_run) {
    // This either succeeds, or fails because we're shutting down a test. We never actually shut
    // down the real sysmem.
    std::ignore = async::PostTask(loop_dispatcher(), std::move(to_run));
  }

  template <typename F>
  void PostTaskToClientDispatcher(F to_run) {
    // This either succeeds, or fails because we're shutting down a test. We never actually shut
    // down the real sysmem.
    std::ignore = async::PostTask(client_dispatcher(), std::move(to_run));
  }

  // Runs `to_run` on the sysmem `loop_` dispatcher and waits for `to_run` to finish.
  //
  // Must not be called from the `loop_` dispatcher, and `to_run` must not call `RunSyncOnLoop()` or
  // `SyncCall()`, otherwise it will hang forever.
  template <typename F>
  void RunSyncOnLoop(F to_run) {
    libsync::Completion done;
    PostTask([&done, to_run = std::move(to_run)]() mutable {
      std::move(to_run)();
      done.Signal();
    });
    done.Wait();
  }

  template <typename F>
  void RunSyncOnClientDispatcher(F to_run) {
    libsync::Completion done;
    PostTaskToClientDispatcher([&done, to_run = std::move(to_run)]() mutable {
      std::move(to_run)();
      done.Signal();
    });
    done.Wait();
  }

  // Runs callable on the sysmem `loop_` dispatcher and waits for `callable` to finish, and returns
  // whatever callable returned.
  //
  // Must not be called from the `loop_` dispatcher, and `to_run` must not call `RunSyncOnLoop()` or
  // `SyncCall()`, otherwise it will hang forever.
  template <typename Callable, typename... Args>
  auto SyncCall(Callable&& callable, Args&&... args) {
    constexpr bool kIsInvocable = std::is_invocable_v<Callable, Args...>;
    static_assert(kIsInvocable,
                  "|Callable| must be callable with the provided |Args|. "
                  "Check that you specified each argument correctly to the |callable| function.");
    if constexpr (kIsInvocable) {
      using Result = std::invoke_result_t<Callable, Args...>;
      // When we're on C++20 we can switch this to capture ...args directly.
      auto args_tuple = std::make_tuple(std::forward<Args>(args)...);
      if constexpr (std::is_void_v<Result>) {
        RunSyncOnLoop(
            [callable = std::move(callable), args_tuple = std::move(args_tuple)]() mutable {
              std::apply([callable = std::move(callable)](
                             auto&&... args) mutable { std::invoke(callable, args...); },
                         std::move(args_tuple));
            });
      } else {
        std::optional<Result> result;
        RunSyncOnLoop(
            [&, callable = std::move(callable), args_tuple = std::move(args_tuple)]() mutable {
              result.emplace(std::apply(
                  [callable = std::move(callable)](auto&&... args) mutable {
                    return std::invoke(callable, args...);
                  },
                  std::move(args_tuple)));
            });
        return *result;
      }
    }
  }

  virtual void OnAllocationFailure() override { LogAllBufferCollections(); }
  void LogAllBufferCollections();

  fidl::ServerBindingGroup<fuchsia_sysmem::Allocator>& v1_allocators() { return v1_allocators_; }
  fidl::ServerBindingGroup<fuchsia_sysmem2::Allocator>& v2_allocators() { return v2_allocators_; }

  // public for tests
  //
  // The client_dispatcher_ has a single thread. This checks we're on that thread.
  mutable fit::thread_checker client_checker_;
  // Checks we're on the one loop_ thread.
  mutable std::optional<fit::thread_checker> loop_checker_;
  fidl::ServerBindingGroup<fuchsia_hardware_sysmem::Sysmem>& BindingsForTest() { return bindings_; }

  const UsagePixelFormatCost& usage_pixel_format_cost() {
    ZX_DEBUG_ASSERT(usage_pixel_format_cost_.has_value());
    return *usage_pixel_format_cost_;
  }

  // Iff callback returns false, stop iterating and return.
  void ForeachSecureHeap(fit::function<bool(const fuchsia_sysmem2::Heap&)> callback);

  zx::result<zx::bti> CreateBti() __TA_REQUIRES(client_checker_);
  zx::bti& mutable_bti_for_testing() { return bti_; }

 private:
  class SecureMemConnection {
   public:
    SecureMemConnection(fidl::ClientEnd<fuchsia_sysmem2::SecureMem> channel,
                        std::unique_ptr<async::Wait> wait_for_close);
    const fidl::SyncClient<fuchsia_sysmem2::SecureMem>& channel() const;

   private:
    fidl::SyncClient<fuchsia_sysmem2::SecureMem> connection_;
    std::unique_ptr<async::Wait> wait_for_close_;
  };

  // to_run must not cause creation or deletion of any LogicalBufferCollection(s), with the one
  // exception of causing deletion of the passed-in LogicalBufferCollection, which is allowed
  void ForEachLogicalBufferCollection(fit::function<void(LogicalBufferCollection*)> to_run) {
    std::lock_guard checker(*loop_checker_);
    // to_run can erase the current item, but std::unordered_set only invalidates iterators pointing
    // at the erased item, so we can just save the pointer and advance iter before calling to_run
    //
    // to_run must not cause any other iterator invalidation
    LogicalBufferCollections::iterator next;
    for (auto iter = logical_buffer_collections_.begin(); iter != logical_buffer_collections_.end();
         /* iter already advanced in the loop */) {
      auto* item = *iter;
      ++iter;
      to_run(item);
    }
  }

  void LogCollectionsTimer(async_dispatcher_t* dispatcher, async::TaskBase* task,
                           zx_status_t status);

  void Shutdown() __TA_REQUIRES(client_checker_);

  zx::result<fuchsia_sysmem2::Config> GetConfigFromFile();

  // We use this dispatcher to serve fuchsia_hardware_sysmem::Sysmem, since that needs to be on a
  // separate dispatcher from loop_.dispatcher() (at least for now).
  async_dispatcher_t* client_dispatcher_ = nullptr;

  // Other than client call-ins and the fuchsia.hardware.Sysmem protocol, everything runs on the
  // loop_ thread.
  async::Loop loop_;
  thrd_t loop_thrd_{};

  inspect::ComponentInspector inspector_{loop_.dispatcher(), {}};

  // Currently located at bootstrap/driver_manager:root/sysmem.
  inspect::Node sysmem_root_;
  inspect::Node heaps_;

  inspect::Node collections_node_;

  // In some unit tests, !bti_.is_valid() or bti_ is a fake BTI.
  zx::bti bti_;

  // This map allows us to look up the BufferCollectionToken by the koid of
  // the server end of a BufferCollectionToken channel.
  std::map<zx_koid_t, BufferCollectionToken*> tokens_by_koid_ __TA_GUARDED(*loop_checker_);

  std::deque<zx_koid_t> unfound_token_koids_ __TA_GUARDED(*loop_checker_);

  struct HashHeap {
    size_t operator()(const fuchsia_sysmem2::Heap& heap) const {
      const static auto hash_heap_type = std::hash<std::string>{};
      const static auto hash_id = std::hash<uint64_t>{};
      size_t hash = 0;
      if (heap.heap_type().has_value()) {
        hash = hash ^ hash_heap_type(heap.heap_type().value());
      }
      if (heap.id().has_value()) {
        hash = hash ^ hash_id(heap.id().value());
      }
      return hash;
    }
  };

  // This map contains all registered memory allocators.
  std::unordered_map<fuchsia_sysmem2::Heap, std::shared_ptr<MemoryAllocator>, HashHeap> allocators_
      __TA_GUARDED(*loop_checker_);

  // This map contains only the secure allocators, if any.  The pointers are owned by allocators_.
  std::unordered_map<fuchsia_sysmem2::Heap, MemoryAllocator*, HashHeap> secure_allocators_
      __TA_GUARDED(*loop_checker_);

  struct SecureMemControl : public protected_ranges::ProtectedRangesCoreControl {
    // ProtectedRangesCoreControl implementation.  These are essentially backed by
    // parent->secure_mem_.
    //
    // cached
    bool IsDynamic() override;

    // cached
    uint64_t MaxRangeCount() override;

    // cached
    uint64_t GetRangeGranularity() override;

    // cached
    bool HasModProtectedRange() override;

    // calls SecureMem driver
    void AddProtectedRange(const protected_ranges::Range& range) override;

    // calls SecureMem driver
    void DelProtectedRange(const protected_ranges::Range& range) override;

    // calls SecureMem driver
    void ModProtectedRange(const protected_ranges::Range& old_range,
                           const protected_ranges::Range& new_range) override;
    // calls SecureMem driver
    void ZeroProtectedSubRange(bool is_covering_range_explicit,
                               const protected_ranges::Range& range) override;

    fuchsia_sysmem2::Heap heap{};
    Sysmem* parent{};
    bool is_dynamic{};
    uint64_t range_granularity{};
    uint64_t max_range_count{};
    bool has_mod_protected_range{};
  };

  // This map has the secure_mem_ properties for each fuchsia_sysmem2::Heap in secure_allocators_.
  std::unordered_map<fuchsia_sysmem2::Heap, SecureMemControl, HashHeap> secure_mem_controls_
      __TA_GUARDED(*loop_checker_);

  // This flag is used to determine if the closing of the current secure mem
  // connection is an error (true), or expected (false).
  std::shared_ptr<std::atomic_bool> current_close_is_abort_;

  // This has the connection to the securemem driver, if any.  Once allocated this is supposed to
  // stay allocated unless mexec is about to happen.  The server end takes care of handling
  // DdkSuspend() to allow mexec to work.  For example, by calling secmem TA.  This channel will
  // close from the server end when DdkSuspend(mexec) happens, but only after
  // UnregisterSecureMem().
  std::unique_ptr<SecureMemConnection> secure_mem_ __TA_GUARDED(*loop_checker_);

  std::unique_ptr<MemoryAllocator> contiguous_system_ram_allocator_ __TA_GUARDED(*loop_checker_);

  using LogicalBufferCollections = std::unordered_set<LogicalBufferCollection*>;
  LogicalBufferCollections logical_buffer_collections_ __TA_GUARDED(*loop_checker_);

  // A single LogicalBuffer can be in this map multiple times, once per VMO koid that has been
  // handed out by sysmem. Entries are removed when the TrackedParentVmo parent of the handed-out
  // VMO sees ZX_VMO_ZERO_CHILDREN, which occurs before LogicalBuffer is deleted.
  using VmoKoids = std::unordered_map<zx_koid_t, FindLogicalBufferByVmoKoidResult>;
  VmoKoids vmo_koids_;

  Settings settings_;

  std::atomic<bool> waiting_for_unbind_ = false;

  std::optional<SnapshotAnnotationRegister> snapshot_annotation_register_;
  SysmemMetrics metrics_;

  bool protected_ranges_disable_dynamic_ __TA_GUARDED(*loop_checker_) = false;

  bool is_secure_mem_ready_ __TA_GUARDED(*loop_checker_) = false;
  bool was_secure_mem_ready_ __TA_GUARDED(*loop_checker_) = false;

  async::TaskMethod<Sysmem, &Sysmem::LogCollectionsTimer> log_all_collections_{this};

  fidl::ServerBindingGroup<fuchsia_hardware_sysmem::Sysmem> bindings_;
  fidl::ServerBindingGroup<fuchsia_sysmem::Allocator> v1_allocators_;
  fidl::ServerBindingGroup<fuchsia_sysmem2::Allocator> v2_allocators_;

  std::optional<const UsagePixelFormatCost> usage_pixel_format_cost_;

  // We allocate protected_memory_size during sysmem driver Start, and then when the securemem
  // driver calls sysmem, that triggers the rest of setting up this ContiguousPooledMemoryAllocator
  // in secure_allocators_. At that point secure_allocators_ owns the allocator and we reset() this
  // field.
  std::unique_ptr<ContiguousPooledMemoryAllocator> pending_protected_allocator_;

  std::optional<component::OutgoingDirectory> outgoing_ __TA_GUARDED(*loop_checker_);
};

}  // namespace sysmem_service

#endif  // SRC_SYSMEM_SERVER_SYSMEM_H_
