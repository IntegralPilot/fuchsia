// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SECUREMEM_DRIVERS_AML_SECUREMEM_SYSMEM_SECURE_MEM_SERVER_H_
#define SRC_DEVICES_SECUREMEM_DRIVERS_AML_SECUREMEM_SYSMEM_SECURE_MEM_SERVER_H_

#include <fidl/fuchsia.sysmem/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/sequence_checker.h>
#include <lib/fdf/dispatcher.h>

#include <optional>
#include <set>

#include <tee-client-api/tee_client_api.h>

#include "secmem-session.h"

// This class is thread-unsafe and must be used from a synchronized dispatcher.
//
// The dispatcher thread for this class must be separate from the ddk_dispatcher_thread_ so that
// TEEC_* calls made by the |SysmemSecureMemServer| can be served by the fdf dispatcher without
// deadlock.
class SysmemSecureMemServer : public fidl::WireServer<fuchsia_sysmem2::SecureMem> {
 public:
  using SecureMemServerOnUnbound = fit::callback<void(bool is_success)>;

  // |dispatcher| must be the current dispatcher of the thread creating this object.
  SysmemSecureMemServer(async_dispatcher_t* dispatcher, zx::channel tee_client_channel);
  ~SysmemSecureMemServer() override;

  void Bind(fidl::ServerEnd<fuchsia_sysmem2::SecureMem> sysmem_secure_mem_server,
            SecureMemServerOnUnbound secure_mem_server_on_unbound);
  void Unbind();

  // fidl::WireServer<fuchsia_sysmem2::SecureMem> impl
  void GetPhysicalSecureHeaps(GetPhysicalSecureHeapsCompleter::Sync& completer) override;
  void GetDynamicSecureHeaps(GetDynamicSecureHeapsCompleter::Sync& completer) override;
  void GetPhysicalSecureHeapProperties(
      GetPhysicalSecureHeapPropertiesRequestView request,
      GetPhysicalSecureHeapPropertiesCompleter::Sync& completer) override;
  void AddSecureHeapPhysicalRange(AddSecureHeapPhysicalRangeRequestView request,
                                  AddSecureHeapPhysicalRangeCompleter::Sync& completer) override;
  void DeleteSecureHeapPhysicalRange(
      DeleteSecureHeapPhysicalRangeRequestView request,
      DeleteSecureHeapPhysicalRangeCompleter::Sync& completer) override;
  void ModifySecureHeapPhysicalRange(
      ModifySecureHeapPhysicalRangeRequestView request,
      ModifySecureHeapPhysicalRangeCompleter::Sync& completer) override;
  void ZeroSubRange(ZeroSubRangeRequestView request,
                    ZeroSubRangeCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_sysmem2::SecureMem> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  // We might want to extract the Range class into a lib used by aml-securemem and sysmem instead of
  // using very similar code.
  class Range {
   public:
    Range(const Range& to_copy) = default;
    Range& operator=(const Range& to_copy) = default;

    static Range BeginLength(uint64_t begin, uint64_t length) { return Range{begin, length}; }
    static Range BeginEnd(uint64_t begin, uint64_t end) { return Range{begin, end - begin}; }
    uint64_t begin() const { return begin_; }
    uint64_t end() const { return begin_ + length_; }
    uint64_t length() const { return length_; }
    bool empty() const { return end() <= begin(); }

   private:
    explicit Range(uint64_t begin, uint64_t length) : begin_(begin), length_(length) {}
    uint64_t begin_;
    uint64_t length_;
  };

  class CompareRangeByBegin {
   public:
    bool operator()(const Range& left, const Range& right) const {
      // Non-empty is required.
      ZX_DEBUG_ASSERT(left.begin() != left.end());
      ZX_DEBUG_ASSERT(right.begin() != right.end());

      if (left.begin() < right.begin()) {
        // <
        return true;
      }
      if (left.begin() > right.begin()) {
        // >
        return false;
      }

      if (left.end() < right.end()) {
        // <
        return true;
      }
      if (left.end() > right.end()) {
        // >
        return false;
      }

      // ==
      return false;
    }
  };

  using Ranges = std::set<Range, CompareRangeByBegin>;

  bool TrySetupSecmemSession();
  void OnUnbound(bool is_success);

  fit::result<fuchsia_sysmem2::Error> GetPhysicalSecureHeapsInternal(
      fidl::AnyArena* allocator,
      fuchsia_sysmem2::wire::SecureMemGetPhysicalSecureHeapsResponse* response);
  fit::result<fuchsia_sysmem2::Error> GetDynamicSecureHeapsInternal(
      fidl::AnyArena* allocator,
      fuchsia_sysmem2::wire::SecureMemGetDynamicSecureHeapsResponse* response);
  fit::result<fuchsia_sysmem2::Error> GetPhysicalSecureHeapPropertiesInternal(
      const fuchsia_sysmem2::wire::SecureHeapAndRange& entire_heap, fidl::AnyArena& allocator,
      fuchsia_sysmem2::wire::SecureMemGetPhysicalSecureHeapPropertiesResponse* reponse);
  fit::result<fuchsia_sysmem2::Error> AddSecureHeapPhysicalRangeInternal(
      fuchsia_sysmem2::wire::SecureHeapAndRange heap_range);
  fit::result<fuchsia_sysmem2::Error> DeleteSecureHeapPhysicalRangeInternal(
      fuchsia_sysmem2::wire::SecureHeapAndRange heap_range);
  fit::result<fuchsia_sysmem2::Error> ModifySecureHeapPhysicalRangeInternal(
      fuchsia_sysmem2::wire::SecureHeapAndRangeModification range_modification);
  fit::result<fuchsia_sysmem2::Error> ZeroSubRangeInternal(
      bool is_covering_range_explicit, fuchsia_sysmem2::wire::SecureHeapAndRange heap_range);

  // Call secmem TA to setup the one physical secure heap that's configured by the TEE Controller.
  zx_status_t SetupVdec(uint64_t* physical_address, uint64_t* size_bytes);

  // Call secmem TA to setup the one physical secure heap that's configured by sysmem.
  zx_status_t ProtectMemoryRange(uint64_t physical_address, uint64_t size_bytes, bool enable);
  zx_status_t AdjustMemoryRange(uint64_t physical_address, size_t size_bytes,
                                uint32_t adjustment_magnitude, bool at_start, bool longer);
  zx_status_t ZeroSubRangeIncrementally(bool is_covering_range_explicit, uint64_t physical_address,
                                        size_t size_bytes);

  static bool IsOverlap(const Range& a, const Range& b);
  static std::pair<Range, Range> SubtractRanges(const Range& a, const Range& b);

  async_dispatcher_t* const dispatcher_;
  fuchsia::tee::ApplicationSyncPtr tee_connection_ = {};
  SecureMemServerOnUnbound secure_mem_server_on_unbound_;

  bool is_get_physical_secure_heaps_called_ = {};
  bool is_get_dynamic_secure_heaps_called_ = {};

  bool is_dynamic_checked_ = {};
  bool is_dynamic_ = {};
  uint32_t max_range_count_ = 0;

  // We try to open a SecmemSession once.  If that fails, we remember the status and
  // EnsureSecmemSession() will return that status without trying Init() again.
  bool has_attempted_secmem_session_connection_ __TA_GUARDED(checker_) = false;
  std::optional<SecmemSession> secmem_session_ __TA_GUARDED(checker_) = std::nullopt;

  Ranges ranges_ __TA_GUARDED(checker_);

  std::optional<fidl::ServerBinding<fuchsia_sysmem2::SecureMem>> binding_;
  async::synchronization_checker checker_;
};

#endif  // SRC_DEVICES_SECUREMEM_DRIVERS_AML_SECUREMEM_SYSMEM_SECURE_MEM_SERVER_H_
