// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sysmem-secure-mem-server.h"

#include <lib/fdf/dispatcher.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/fidl/cpp/wire/vector_view.h>
#include <lib/fit/defer.h>
#include <zircon/errors.h>

#include <cinttypes>
#include <limits>

#include <bind/fuchsia/amlogic/platform/sysmem/heap/cpp/bind.h>
#include <fbl/algorithm.h>
#include <safemath/safe_math.h>

#include "log.h"

namespace {

constexpr uint32_t kProtectedRangeGranularity = 64u * 1024;

bool VerifyRange(uint64_t physical_address, size_t size_bytes, uint32_t required_alignment) {
  if (physical_address % required_alignment != 0) {
    LOG(ERROR, "physical_address not divisible by required_alignment");
    return false;
  }

  if (size_bytes % required_alignment != 0) {
    LOG(ERROR, "size_bytes not divisible by required_alignment");
    return false;
  }

  if (size_bytes == 0) {
    LOG(ERROR, "heap.size_bytes == 0");
    return false;
  }

  if (!safemath::IsValueInRangeForNumericType<uint32_t>(physical_address)) {
    LOG(ERROR, "heap.physical_address > 0xFFFFFFFF");
    return false;
  }

  if (!safemath::IsValueInRangeForNumericType<uint32_t>(size_bytes)) {
    LOG(ERROR, "heap.size_bytes > 0xFFFFFFFF");
    return false;
  }

  if (!safemath::CheckAdd(physical_address, size_bytes).IsValid<uint32_t>()) {
    // At least For now, we are rejecting any range whose last page is the page
    // that contains 0xFFFFFFFF.   It's probably best to continue rejecting any
    // such range at least until we've covered that case in focused testing.
    // If we wanted to allow such a range we'd subtract 1 from size_bytes for
    // the argument passed to CheckAdd() just above.
    LOG(ERROR, "start + size overflow");
    return false;
  }

  return true;
}

bool ValidateSecureHeapRange(const fuchsia_sysmem2::wire::SecureHeapRange& range) {
  if (!range.has_physical_address()) {
    LOG(INFO, "!range.has_physical_address()");
    return false;
  }

  if (!range.has_size_bytes()) {
    LOG(INFO, "!range.has_size_bytes()");
    return false;
  }

  if (range.physical_address() > (1ull << 32) - kProtectedRangeGranularity) {
    LOG(INFO, "physical_address() > (1ull << 32) - kProtectedRangeGranularity");
    return false;
  }

  if (range.size_bytes() > std::numeric_limits<uint32_t>::max()) {
    LOG(INFO, "size_bytes() > 0xFFFFFFFF");
    return false;
  }

  if (range.physical_address() + range.size_bytes() > (1ull << 32)) {
    LOG(INFO, "physical_address() + size_bytes() > (1ull << 32)");
    return false;
  }

  return true;
}

bool ValidateSecureHeapAndRange(const fuchsia_sysmem2::wire::SecureHeapAndRange& heap_range,
                                bool is_zeroing) {
  if (!heap_range.has_heap()) {
    LOG(INFO, "!heap_range.has_heap()");
    return false;
  }

  if (heap_range.heap().has_id() && heap_range.heap().id() != 0) {
    LOG(INFO, "heap id not 0");
    return false;
  }

  std::string heap_type(heap_range.heap().heap_type().data(), heap_range.heap().heap_type().size());

  if (is_zeroing) {
    if (heap_type != bind_fuchsia_amlogic_platform_sysmem_heap::HEAP_TYPE_SECURE &&
        heap_type != bind_fuchsia_amlogic_platform_sysmem_heap::HEAP_TYPE_SECURE_VDEC) {
      LOG(INFO, "heap_range.heap() != AMLogic secure && heap_range.heap() != AMLogic secure VDEC");
      return false;
    }
  } else {
    if (heap_type != bind_fuchsia_amlogic_platform_sysmem_heap::HEAP_TYPE_SECURE) {
      LOG(INFO, "heap_range.heap() != kAmlogicSecure");
      return false;
    }
  }

  if (!heap_range.has_range()) {
    LOG(INFO, "!heap_range.has_range()");
    return false;
  }

  if (!ValidateSecureHeapRange(heap_range.range())) {
    return false;
  }

  return true;
}

bool ValidateSecureHeapAndRangeModification(
    const fuchsia_sysmem2::wire::SecureHeapAndRangeModification& range_modification) {
  if (!range_modification.has_heap()) {
    LOG(INFO, "!range_modification.has_heap()");
    return false;
  }

  std::string heap_type(range_modification.heap().heap_type().data(),
                        range_modification.heap().heap_type().size());

  if (heap_type != bind_fuchsia_amlogic_platform_sysmem_heap::HEAP_TYPE_SECURE) {
    LOG(INFO, "heap_range.heap() != kAmlogicSecure");
    return false;
  }

  if (!range_modification.has_old_range()) {
    LOG(INFO, "!range_modification.has_old_range()");
    return false;
  }

  if (!range_modification.has_new_range()) {
    LOG(INFO, "!range_modification.has_new_range()");
    return false;
  }

  const auto& old_range = range_modification.old_range();
  const auto& new_range = range_modification.new_range();

  if (!ValidateSecureHeapRange(old_range)) {
    LOG(INFO, "!ValidateSecureHeapRange(old_range)");
    return false;
  }

  if (!ValidateSecureHeapRange(new_range)) {
    LOG(INFO, "!ValidateSecureHeapRange(new_range)");
    return false;
  }

  if (new_range.physical_address() != old_range.physical_address() &&
      new_range.physical_address() + new_range.size_bytes() !=
          old_range.physical_address() + old_range.size_bytes()) {
    LOG(INFO, "old_range and new_range do not match in start or end");
    return false;
  }

  if (old_range.physical_address() == new_range.physical_address() &&
      old_range.size_bytes() == new_range.size_bytes()) {
    LOG(INFO, "old_range and new_range are the same");
    return false;
  }

  if (old_range.size_bytes() == 0) {
    LOG(INFO, "old_range is empty");
    return false;
  }

  // new range is allowed to be empty, which effectively becomes a delete

  return true;
}

constexpr char kSysmemSecureMemServerThreadSafetyDescription[] =
    "|SysmemSecureMemServer| is thread-unsafe.";

}  // namespace

SysmemSecureMemServer::SysmemSecureMemServer(async_dispatcher_t* dispatcher,
                                             zx::channel tee_client_channel)
    : dispatcher_(dispatcher), checker_(dispatcher, kSysmemSecureMemServerThreadSafetyDescription) {
  ZX_DEBUG_ASSERT(tee_client_channel);
  tee_connection_.Bind(std::move(tee_client_channel));
}

SysmemSecureMemServer::~SysmemSecureMemServer() {
  std::scoped_lock lock{checker_};
  ZX_DEBUG_ASSERT(ranges_.empty());
}

void SysmemSecureMemServer::Bind(
    fidl::ServerEnd<fuchsia_sysmem2::SecureMem> sysmem_secure_mem_server,
    SecureMemServerOnUnbound secure_mem_server_on_unbound) {
  ZX_DEBUG_ASSERT(sysmem_secure_mem_server);
  ZX_DEBUG_ASSERT(secure_mem_server_on_unbound);
  ZX_DEBUG_ASSERT(!secure_mem_server_on_unbound_);
  ZX_DEBUG_ASSERT(!binding_);

  secure_mem_server_on_unbound_ = std::move(secure_mem_server_on_unbound);
  binding_.emplace(
      dispatcher_, std::move(sysmem_secure_mem_server), this,
      [](SysmemSecureMemServer* self, fidl::UnbindInfo info) { self->OnUnbound(false); });
}

void SysmemSecureMemServer::Unbind() {
  std::scoped_lock lock{checker_};
  binding_.reset();
  OnUnbound(true);
}

void SysmemSecureMemServer::GetPhysicalSecureHeaps(
    GetPhysicalSecureHeapsCompleter::Sync& completer) {
  std::scoped_lock lock{checker_};
  fidl::Arena allocator;
  fuchsia_sysmem2::wire::SecureMemGetPhysicalSecureHeapsResponse response;
  auto get_result = GetPhysicalSecureHeapsInternal(&allocator, &response);
  if (!get_result.is_ok()) {
    LOG(ERROR, "GetPhysicalSecureHeapsInternal() failed: %u",
        fidl::ToUnderlying(get_result.error_value()));
    completer.ReplyError(get_result.error_value());
    return;
  }

  completer.ReplySuccess(std::move(response));
}

void SysmemSecureMemServer::GetDynamicSecureHeaps(GetDynamicSecureHeapsCompleter::Sync& completer) {
  std::scoped_lock lock{checker_};
  fidl::Arena allocator;
  fuchsia_sysmem2::wire::SecureMemGetDynamicSecureHeapsResponse response;
  auto get_result = GetDynamicSecureHeapsInternal(&allocator, &response);
  if (!get_result.is_ok()) {
    LOG(ERROR, "GetDynamicSecureHeaps() failed: %u", fidl::ToUnderlying(get_result.error_value()));
    completer.ReplyError(get_result.error_value());
    return;
  }
  completer.ReplySuccess(std::move(response));
}

void SysmemSecureMemServer::GetPhysicalSecureHeapProperties(
    GetPhysicalSecureHeapPropertiesRequestView request,
    GetPhysicalSecureHeapPropertiesCompleter::Sync& completer) {
  std::scoped_lock lock{checker_};
  // must out-live Reply()
  fidl::Arena allocator;
  fuchsia_sysmem2::wire::SecureMemGetPhysicalSecureHeapPropertiesResponse response;
  auto get_result =
      GetPhysicalSecureHeapPropertiesInternal(request->entire_heap(), allocator, &response);
  if (!get_result.is_ok()) {
    LOG(INFO, "GetPhysicalSecureHeapPropertiesInternal() failed: %u",
        fidl::ToUnderlying(get_result.error_value()));
    completer.ReplyError(get_result.error_value());
    return;
  }
  completer.ReplySuccess(std::move(response));
}

void SysmemSecureMemServer::AddSecureHeapPhysicalRange(
    AddSecureHeapPhysicalRangeRequestView request,
    AddSecureHeapPhysicalRangeCompleter::Sync& completer) {
  std::scoped_lock lock{checker_};
  // must out-live Reply()
  fidl::Arena allocator;
  auto add_result = AddSecureHeapPhysicalRangeInternal(request->heap_range());
  if (!add_result.is_ok()) {
    LOG(INFO, "AddSecureHeapPhysicalRangeInternal() failed: %u",
        fidl::ToUnderlying(add_result.error_value()));
    completer.ReplyError(add_result.error_value());
    return;
  }
  completer.ReplySuccess();
}

void SysmemSecureMemServer::DeleteSecureHeapPhysicalRange(
    DeleteSecureHeapPhysicalRangeRequestView request,
    DeleteSecureHeapPhysicalRangeCompleter::Sync& completer) {
  std::scoped_lock lock{checker_};
  // must out-live Reply()
  fidl::Arena allocator;
  auto delete_result = DeleteSecureHeapPhysicalRangeInternal(request->heap_range());
  if (!delete_result.is_ok()) {
    LOG(INFO, "DeleteSecureHeapPhysicalRangesInternal() failed: %u",
        fidl::ToUnderlying(delete_result.error_value()));
    completer.ReplyError(delete_result.error_value());
    return;
  }
  completer.ReplySuccess();
}

void SysmemSecureMemServer::ModifySecureHeapPhysicalRange(
    ModifySecureHeapPhysicalRangeRequestView request,
    ModifySecureHeapPhysicalRangeCompleter::Sync& completer) {
  std::scoped_lock lock{checker_};
  // must out-live Reply()
  fidl::Arena allocator;
  auto modify_result = ModifySecureHeapPhysicalRangeInternal(request->range_modification());
  if (!modify_result.is_ok()) {
    LOG(INFO, "ModifySecureHeapPhysicalRangesInternal() failed: %u",
        fidl::ToUnderlying(modify_result.error_value()));
    completer.ReplyError(modify_result.error_value());
    return;
  }
  completer.ReplySuccess();
}

void SysmemSecureMemServer::ZeroSubRange(ZeroSubRangeRequestView request,
                                         ZeroSubRangeCompleter::Sync& completer) {
  std::scoped_lock lock{checker_};
  // must out-live Reply()
  fidl::Arena allocator;
  auto zero_result =
      ZeroSubRangeInternal(request->is_covering_range_explicit(), request->heap_range());
  if (!zero_result.is_ok()) {
    LOG(INFO, "ZeroSubRangeInternal() failed: %u", fidl::ToUnderlying(zero_result.error_value()));
    completer.ReplyError(zero_result.error_value());
    return;
  }
  completer.ReplySuccess();
}

void SysmemSecureMemServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_sysmem2::SecureMem> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  LOG(WARNING, "unknown method: %" PRId64, metadata.method_ordinal);
}

bool SysmemSecureMemServer::TrySetupSecmemSession() {
  std::scoped_lock lock{checker_};
  // We only try this once; if it doesn't work the first time, it's very
  // unlikely to work on retry anyway, and this avoids some retry complexity.
  if (!has_attempted_secmem_session_connection_) {
    ZX_DEBUG_ASSERT(tee_connection_.is_bound());
    ZX_DEBUG_ASSERT(!secmem_session_.has_value());

    has_attempted_secmem_session_connection_ = true;

    auto session_result = SecmemSession::TryOpen(std::move(tee_connection_));
    if (!session_result.is_ok()) {
      // Logging handled in `SecmemSession::TryOpen`
      tee_connection_ = session_result.take_error();
      return false;
    }

    secmem_session_.emplace(session_result.take_value());
    LOG(DEBUG, "Successfully connected to secmem session");
    return true;
  }

  return secmem_session_.has_value();
}

void SysmemSecureMemServer::OnUnbound(bool is_success) {
  std::scoped_lock lock{checker_};

  if (has_attempted_secmem_session_connection_ && secmem_session_.has_value()) {
    for (auto& range : ranges_) {
      TEEC_Result tee_status = secmem_session_->ProtectMemoryRange(
          static_cast<uint32_t>(range.begin()), static_cast<uint32_t>(range.length()), false);
      if (tee_status != TEEC_SUCCESS) {
        LOG(ERROR, "SecmemSession::ProtectMemoryRange(false) failed - TEEC_Result %d", tee_status);
        ZX_PANIC("SecmemSession::ProtectMemoryRange(false) failed - TEEC_Result %d", tee_status);
      }
    }
    ranges_.clear();
    secmem_session_.reset();
  } else {
    ZX_DEBUG_ASSERT(!secmem_session_);
  }
  if (secure_mem_server_on_unbound_) {
    secure_mem_server_on_unbound_(is_success);
  }
}

fit::result<fuchsia_sysmem2::wire::Error> SysmemSecureMemServer::GetPhysicalSecureHeapsInternal(
    fidl::AnyArena* allocator,
    fuchsia_sysmem2::wire::SecureMemGetPhysicalSecureHeapsResponse* response) {
  std::scoped_lock lock{checker_};

  if (is_get_physical_secure_heaps_called_) {
    LOG(ERROR, "GetPhysicalSecureHeaps may only be called at most once - reply status: %d",
        ZX_ERR_BAD_STATE);
    return fit::error(fuchsia_sysmem2::wire::Error::kProtocolDeviation);
  }
  is_get_physical_secure_heaps_called_ = true;

  if (!TrySetupSecmemSession()) {
    // Logging handled in `TrySetupSecmemSession`
    return fit::error(fuchsia_sysmem2::wire::Error::kUnspecified);
  }

  uint64_t vdec_phys_base;
  size_t vdec_size;
  zx_status_t status = SetupVdec(&vdec_phys_base, &vdec_size);
  if (status != ZX_OK) {
    LOG(ERROR, "SetupVdec failed: %s", zx_status_get_string(status));
    return fit::error(fuchsia_sysmem2::wire::Error::kUnspecified);
  }

  auto heaps = fidl::VectorView<fuchsia_sysmem2::wire::SecureHeapAndRanges>(*allocator, 1);
  auto heap_and_ranges_builder = fuchsia_sysmem2::wire::SecureHeapAndRanges::Builder(*allocator);

  auto heap_builder = fuchsia_sysmem2::wire::Heap::Builder(*allocator);
  heap_builder.heap_type(bind_fuchsia_amlogic_platform_sysmem_heap::HEAP_TYPE_SECURE_VDEC);
  heap_builder.id(0);

  heap_and_ranges_builder.heap(heap_builder.Build());

  auto ranges = fidl::VectorView<fuchsia_sysmem2::wire::SecureHeapRange>(*allocator, 1);
  auto range_builder = fuchsia_sysmem2::wire::SecureHeapRange::Builder(*allocator);
  range_builder.physical_address(vdec_phys_base);
  range_builder.size_bytes(vdec_size);

  ranges[0] = range_builder.Build();
  heap_and_ranges_builder.ranges(ranges);
  heaps[0] = heap_and_ranges_builder.Build();

  *response = fuchsia_sysmem2::wire::SecureMemGetPhysicalSecureHeapsResponse::Builder(*allocator)
                  .heaps(std::move(heaps))
                  .Build();

  return fit::ok();
}

fit::result<fuchsia_sysmem2::Error> SysmemSecureMemServer::GetDynamicSecureHeapsInternal(
    fidl::AnyArena* allocator,
    fuchsia_sysmem2::wire::SecureMemGetDynamicSecureHeapsResponse* response) {
  std::scoped_lock lock(checker_);

  if (is_get_dynamic_secure_heaps_called_) {
    LOG(ERROR, "GetDynamicSecureHeaps may only be called at most once.");
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }
  is_get_dynamic_secure_heaps_called_ = true;

  if (!TrySetupSecmemSession()) {
    // Logging handled in `TrySetupSecmemSession`
    return fit::error(fuchsia_sysmem2::Error::kUnspecified);
  }

  auto response_builder =
      fuchsia_sysmem2::wire::SecureMemGetDynamicSecureHeapsResponse::Builder(*allocator);
  auto heaps = fidl::VectorView<fuchsia_sysmem2::wire::DynamicSecureHeap>(*allocator, 1);
  auto dynamic_heap = fuchsia_sysmem2::wire::DynamicSecureHeap::Builder(*allocator);

  auto heap = fuchsia_sysmem2::wire::Heap::Builder(*allocator);
  heap.heap_type(bind_fuchsia_amlogic_platform_sysmem_heap::HEAP_TYPE_SECURE);
  heap.id(0);

  dynamic_heap.heap(heap.Build());

  heaps[0] = dynamic_heap.Build();
  response_builder.heaps(std::move(heaps));

  *response = response_builder.Build();

  return fit::ok();
}

fit::result<fuchsia_sysmem2::Error> SysmemSecureMemServer::GetPhysicalSecureHeapPropertiesInternal(
    const fuchsia_sysmem2::wire::SecureHeapAndRange& entire_heap, fidl::AnyArena& allocator,
    fuchsia_sysmem2::wire::SecureMemGetPhysicalSecureHeapPropertiesResponse* response) {
  std::scoped_lock lock{checker_};

  if (!entire_heap.has_heap()) {
    LOG(INFO, "!entire_heap.has_heap()");
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  if (!entire_heap.has_range()) {
    LOG(INFO, "!entire_heap.has_range()");
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  if (!entire_heap.range().has_physical_address()) {
    LOG(INFO, "!entire_heap.range().has_physical_address()");
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  if (!entire_heap.range().has_size_bytes()) {
    LOG(INFO, "!entire_heap.range().has_size_bytes()");
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  if (!TrySetupSecmemSession()) {
    // Logging in TrySetupSecmemSession
    return fit::error(fuchsia_sysmem2::Error::kUnspecified);
  }

  std::string heap_type(entire_heap.heap().heap_type().data(),
                        entire_heap.heap().heap_type().size());

  if (heap_type != bind_fuchsia_amlogic_platform_sysmem_heap::HEAP_TYPE_SECURE) {
    LOG(INFO, "heap_type != AMLogic secure");
    return fit::error(fuchsia_sysmem2::Error::kNotFound);
  }
  if (entire_heap.heap().id() != 0) {
    LOG(INFO, "heap id != 0");
    return fit::error(fuchsia_sysmem2::Error::kNotFound);
  }

  is_dynamic_ = secmem_session_->DetectIsAdjustAndSkipDeviceSecureModeUpdateAvailable();
  is_dynamic_checked_ = true;
  max_range_count_ = secmem_session_->GetMaxClientUsableProtectedRangeCount(
      entire_heap.range().physical_address(), entire_heap.range().size_bytes());

  auto builder = fuchsia_sysmem2::wire::SecureHeapProperties::Builder(allocator);
  auto heap = fuchsia_sysmem2::wire::Heap::Builder(allocator);

  heap.heap_type(bind_fuchsia_amlogic_platform_sysmem_heap::HEAP_TYPE_SECURE);
  heap.id(0);

  builder.heap(heap.Build());

  builder.dynamic_protection_ranges(is_dynamic_);
  builder.protected_range_granularity(kProtectedRangeGranularity);
  builder.max_protected_range_count(max_range_count_);
  builder.is_mod_protected_range_available(is_dynamic_);

  *response =
      fuchsia_sysmem2::wire::SecureMemGetPhysicalSecureHeapPropertiesResponse::Builder(allocator)
          .properties(builder.Build())
          .Build();

  return fit::ok();
}

fit::result<fuchsia_sysmem2::Error> SysmemSecureMemServer::AddSecureHeapPhysicalRangeInternal(
    fuchsia_sysmem2::wire::SecureHeapAndRange heap_range) {
  std::scoped_lock lock{checker_};
  ZX_DEBUG_ASSERT(ranges_.size() <= max_range_count_);

  if (!TrySetupSecmemSession()) {
    // Logging in TrySetupSecmemSession
    return fit::error(fuchsia_sysmem2::Error::kUnspecified);
  }

  if (!is_dynamic_checked_) {
    LOG(INFO, "!is_dynamic_checked_");
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  if (!ValidateSecureHeapAndRange(heap_range, false)) {
    // Logging in ValidateSecureHeapAndRange
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  ZX_DEBUG_ASSERT(ranges_.size() <= max_range_count_);
  if (ranges_.size() == max_range_count_) {
    LOG(INFO, "range_count_ == max_range_count_");
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  const auto range = heap_range.range();
  zx_status_t status = ProtectMemoryRange(range.physical_address(), range.size_bytes(), true);
  if (status != ZX_OK) {
    LOG(ERROR, "ProtectMemoryRange(true) failed - status: %d", status);
    return fit::error(fuchsia_sysmem2::Error::kUnspecified);
  }

  const Range new_range = Range::BeginLength(range.physical_address(), range.size_bytes());
  ranges_.insert(new_range);

  return fit::ok();
}

fit::result<fuchsia_sysmem2::Error> SysmemSecureMemServer::DeleteSecureHeapPhysicalRangeInternal(
    fuchsia_sysmem2::wire::SecureHeapAndRange heap_range) {
  std::scoped_lock lock{checker_};
  ZX_DEBUG_ASSERT(ranges_.size() <= max_range_count_);

  if (!TrySetupSecmemSession()) {
    // Logging in TrySetupSecmemSession
    return fit::error(fuchsia_sysmem2::Error::kUnspecified);
  }

  if (!is_dynamic_checked_) {
    LOG(INFO, "!is_dynamic_checked_");
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  if (!is_dynamic_) {
    LOG(ERROR,
        "DeleteSecureHeapPhysicalRangesInternal() can't be called when !dynamic - reply "
        "status: %d",
        ZX_ERR_BAD_STATE);
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  if (!ValidateSecureHeapAndRange(heap_range, false)) {
    // Logging in ValidateSecureHeapAndRange
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  const Range to_delete =
      Range::BeginLength(heap_range.range().physical_address(), heap_range.range().size_bytes());
  auto to_delete_iter = ranges_.find(to_delete);
  if (to_delete_iter == ranges_.end()) {
    LOG(INFO, "ranges_.find(to_delete) == ranges_.end()");
    return fit::error(fuchsia_sysmem2::Error::kNotFound);
  }

  // We could optimize this to only search a portion of ranges_, but the size limit is quite small
  // so this will be fine.
  //
  // This checks if to_delete is fully covered by other ranges.  If fully covered, we don't need to
  // zero incrementally.  If not fully covered, we do need to zero incrementally.
  Range uncovered = to_delete;
  for (auto iter = ranges_.begin(); iter != ranges_.end(); ++iter) {
    if (iter == to_delete_iter) {
      continue;
    }
    auto& range = *iter;
    if (range.end() <= uncovered.begin()) {
      continue;
    }
    if (range.begin() >= uncovered.end()) {
      break;
    }
    auto [left_remaining, right_remaining] = SubtractRanges(uncovered, range);
    if (!left_remaining.empty()) {
      // This range didn't fully cover uncovered, and no later range will either, since later ranges
      // will have begin >= range.begin, so we know uncovered isn't empty overall.
      break;
    }
    ZX_DEBUG_ASSERT(left_remaining.empty());
    if (!right_remaining.empty()) {
      // Later ranges might cover the rest.
      uncovered = right_remaining;
      continue;
    }
    ZX_DEBUG_ASSERT(right_remaining.empty());
    uncovered = right_remaining;
    break;
  }

  if (!uncovered.empty()) {
    // Shorten the range to nothing incrementally so that page zeroing doesn't happen all in one
    // call to the TEE.
    const auto range = heap_range.range();
    zx_status_t status = AdjustMemoryRange(range.physical_address(), range.size_bytes(),
                                           static_cast<uint32_t>(range.size_bytes()), false, false);
    if (status != ZX_OK) {
      LOG(INFO, "AdjustMemoryRange() failed - status: %d", status);
      return fit::error(fuchsia_sysmem2::Error::kUnspecified);
    }
  } else {
    // No need for incremental zeroing since no zeroing will occur.  We can just make one call to
    // the TEE.
    const auto range = heap_range.range();
    zx_status_t status = ProtectMemoryRange(range.physical_address(), range.size_bytes(), false);
    if (status != ZX_OK) {
      LOG(ERROR, "ProtectMemoryRange(false) failed - status: %d", status);
      return fit::error(fuchsia_sysmem2::Error::kUnspecified);
    }
  }

  ranges_.erase(to_delete);

  return fit::ok();
}

fit::result<fuchsia_sysmem2::Error> SysmemSecureMemServer::ZeroSubRangeInternal(
    bool is_covering_range_explicit, fuchsia_sysmem2::wire::SecureHeapAndRange heap_range) {
  std::scoped_lock lock{checker_};
  ZX_DEBUG_ASSERT(ranges_.size() <= max_range_count_);

  if (!TrySetupSecmemSession()) {
    // Logging in TrySetupSecmemSession
    return fit::error(fuchsia_sysmem2::Error::kUnspecified);
  }

  if (!is_dynamic_checked_) {
    LOG(INFO, "!is_dynamic_checked_");
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  if (!is_dynamic_) {
    LOG(ERROR,
        "ZeroSubRangeInternal() can't be called when !dynamic - reply "
        "status: %d",
        ZX_ERR_BAD_STATE);
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  if (!ValidateSecureHeapAndRange(heap_range, true)) {
    // Logging in ValidateSecureHeapAndRange
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  const Range to_zero =
      Range::BeginLength(heap_range.range().physical_address(), heap_range.range().size_bytes());

  if (is_covering_range_explicit) {
    // We don't strictly need to check for a covering range here since the TEE will do an equivalent
    // check, but doing this check here is helpful for debugging reasons.  Also, it's nice to avoid
    // situations where the first fiew chunks could zero successfully and then we could fail if the
    // next chunk isn't covered by any range.  This could occur because we zero incrementally to
    // avoid zeroing too much per call to the TEE.
    const Range to_find = Range::BeginLength(to_zero.begin(), std::numeric_limits<uint64_t>::max());
    auto covering = ranges_.upper_bound(to_find);
    if (covering != ranges_.begin()) {
      --covering;
    }
    if (covering == ranges_.end() || covering->begin() > to_zero.begin() ||
        covering->end() < to_zero.end()) {
      LOG(ERROR, "to_zero not entirely covered by a single range in ranges_");
      LOG(ERROR, "to_zero: begin: 0x%" PRIx64 " length: 0x%" PRIx64 " end: 0x%" PRIx64,
          to_zero.begin(), to_zero.length(), to_zero.end());
      for (auto& range : ranges_) {
        LOG(ERROR, "range: begin: 0x%" PRIx64 " length: 0x%" PRIx64 " end: 0x%" PRIx64,
            range.begin(), range.length(), range.end());
      }
      return fit::error(fuchsia_sysmem2::Error::kNotFound);
    }

    // Similar for validating that there's no other range that overlaps - checking here is useful
    // for debugging and it's nice to avoid partial success then failure.
    bool found_another_overlapping = false;
    for (auto iter = ranges_.begin(); iter != ranges_.end(); ++iter) {
      if (iter == covering) {
        continue;
      }
      if (iter->end() <= to_zero.begin()) {
        continue;
      }
      if (iter->begin() >= to_zero.end()) {
        continue;
      }
      found_another_overlapping = true;
      break;
    }
    if (found_another_overlapping) {
      LOG(ERROR, "ZeroSubRangeInternal() found a second range that overlaps; this isn't allowed");
      return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
    }
  }

  // Zero incrementally to avoid too much zeroing in a single call to the TEE.
  const auto& range = heap_range.range();
  zx_status_t status = ZeroSubRangeIncrementally(is_covering_range_explicit,
                                                 range.physical_address(), range.size_bytes());
  if (status != ZX_OK) {
    LOG(ERROR, "ZeroSubRangeIncermentally() failed - status: %d", status);
    return fit::error(fuchsia_sysmem2::Error::kUnspecified);
  }

  return fit::ok();
}

fit::result<fuchsia_sysmem2::Error> SysmemSecureMemServer::ModifySecureHeapPhysicalRangeInternal(
    fuchsia_sysmem2::wire::SecureHeapAndRangeModification range_modification) {
  std::scoped_lock lock{checker_};
  ZX_DEBUG_ASSERT(ranges_.size() <= max_range_count_);

  if (!TrySetupSecmemSession()) {
    // Logging in TrySetupSecmemSession
    return fit::error(fuchsia_sysmem2::Error::kUnspecified);
  }

  if (!is_dynamic_checked_) {
    LOG(INFO, "!is_dynamic_checked_");
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  if (!is_dynamic_) {
    LOG(ERROR,
        "ModifySecureHeapPhysicalRangesInternal() can't be called when !dynamic - reply "
        "status: %d",
        ZX_ERR_BAD_STATE);
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  if (!ValidateSecureHeapAndRangeModification(range_modification)) {
    // Logging in ValidateSecureHeapAndRangeModification
    return fit::error(fuchsia_sysmem2::Error::kProtocolDeviation);
  }

  const auto& old_range_in = range_modification.old_range();
  const Range old_range =
      Range::BeginLength(old_range_in.physical_address(), old_range_in.size_bytes());
  if (ranges_.find(old_range) == ranges_.end()) {
    LOG(INFO, "ranges_.find(old_range) == ranges_.end()");
    return fit::error(fuchsia_sysmem2::Error::kNotFound);
  }
  const auto& new_range_in = range_modification.new_range();
  const Range new_range =
      Range::BeginLength(new_range_in.physical_address(), new_range_in.size_bytes());

  bool at_start;
  bool longer;
  uint64_t adjustment_magnitude;
  if (old_range.begin() == new_range.begin()) {
    ZX_DEBUG_ASSERT(old_range.end() != new_range.end());
    at_start = false;
    longer = new_range.end() > old_range.end();
    if (longer) {
      adjustment_magnitude = new_range.end() - old_range.end();
    } else {
      adjustment_magnitude = old_range.end() - new_range.end();
    }
  } else {
    ZX_DEBUG_ASSERT(old_range.begin() != new_range.begin());
    at_start = true;
    longer = new_range.begin() < old_range.begin();
    if (longer) {
      adjustment_magnitude = old_range.begin() - new_range.begin();
    } else {
      adjustment_magnitude = new_range.begin() - old_range.begin();
    }
  }

  zx_status_t status =
      AdjustMemoryRange(old_range.begin(), old_range.length(),
                        static_cast<uint32_t>(adjustment_magnitude), at_start, longer);
  if (status != ZX_OK) {
    LOG(INFO, "AdjustMemoryRange() failed - status: %d", status);
    return fit::error(fuchsia_sysmem2::Error::kUnspecified);
  }

  ranges_.erase(old_range);
  if (!new_range.empty()) {
    ranges_.insert(new_range);
  }

  return fit::ok();
}

zx_status_t SysmemSecureMemServer::SetupVdec(uint64_t* physical_address, size_t* size_bytes) {
  std::scoped_lock lock{checker_};
  ZX_DEBUG_ASSERT(has_attempted_secmem_session_connection_);
  ZX_DEBUG_ASSERT(secmem_session_.has_value());
  uint32_t start;
  uint32_t length;
  TEEC_Result tee_status = secmem_session_->AllocateSecureMemory(&start, &length);
  if (tee_status != TEEC_SUCCESS) {
    LOG(ERROR, "SecmemSession::AllocateSecureMemory() failed - TEEC_Result %" PRIu32, tee_status);
    return ZX_ERR_INTERNAL;
  }
  *physical_address = start;
  *size_bytes = length;
  return ZX_OK;
}

zx_status_t SysmemSecureMemServer::ProtectMemoryRange(uint64_t physical_address, size_t size_bytes,
                                                      bool enable) {
  std::scoped_lock lock{checker_};
  ZX_DEBUG_ASSERT(has_attempted_secmem_session_connection_);
  ZX_DEBUG_ASSERT(secmem_session_.has_value());
  if (!VerifyRange(physical_address, size_bytes, kProtectedRangeGranularity)) {
    return ZX_ERR_INVALID_ARGS;
  }
  auto start = static_cast<uint32_t>(physical_address);
  auto length = static_cast<uint32_t>(size_bytes);
  TEEC_Result tee_status = secmem_session_->ProtectMemoryRange(start, length, enable);
  if (tee_status != TEEC_SUCCESS) {
    LOG(ERROR, "SecmemSession::ProtectMemoryRange() failed - TEEC_Result %d returning status: %d",
        tee_status, ZX_ERR_INTERNAL);
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

zx_status_t SysmemSecureMemServer::AdjustMemoryRange(uint64_t physical_address, size_t size_bytes,
                                                     uint32_t adjustment_magnitude, bool at_start,
                                                     bool longer) {
  std::scoped_lock lock{checker_};
  ZX_DEBUG_ASSERT(has_attempted_secmem_session_connection_);
  ZX_DEBUG_ASSERT(secmem_session_.has_value());
  if (!VerifyRange(physical_address, size_bytes, kProtectedRangeGranularity)) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (!longer && adjustment_magnitude > size_bytes) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (longer) {
    if (at_start) {
      if (!safemath::CheckSub(physical_address, adjustment_magnitude).IsValid<uint32_t>()) {
        return ZX_ERR_INVALID_ARGS;
      }
    } else {
      // physical_address + size_bytes was already checked by calling VerifyRange() above.
      if (!safemath::CheckAdd(physical_address + size_bytes, adjustment_magnitude)
               .IsValid<uint32_t>()) {
        return ZX_ERR_INVALID_ARGS;
      }
    }
  }
  auto start = static_cast<uint32_t>(physical_address);
  auto length = static_cast<uint32_t>(size_bytes);
  TEEC_Result tee_status =
      secmem_session_->AdjustMemoryRange(start, length, adjustment_magnitude, at_start, longer);
  if (tee_status != TEEC_SUCCESS) {
    LOG(ERROR, "SecmemSession::AdjustMemoryRange() failed - TEEC_Result %d returning status: %d",
        tee_status, ZX_ERR_INTERNAL);
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

zx_status_t SysmemSecureMemServer::ZeroSubRangeIncrementally(bool is_covering_range_explicit,
                                                             uint64_t physical_address,
                                                             size_t size_bytes) {
  std::scoped_lock lock{checker_};
  ZX_DEBUG_ASSERT(has_attempted_secmem_session_connection_);
  ZX_DEBUG_ASSERT(secmem_session_.has_value());
  if (!VerifyRange(physical_address, size_bytes, zx_system_get_page_size())) {
    return ZX_ERR_INVALID_ARGS;
  }
  auto start = static_cast<uint32_t>(physical_address);
  auto length = static_cast<uint32_t>(size_bytes);
  // This zeroes incrementally.
  TEEC_Result tee_status = secmem_session_->ZeroSubRange(is_covering_range_explicit, start, length);
  if (tee_status != TEEC_SUCCESS) {
    LOG(ERROR, "SecmemSession::ZeroSubRange() failed - TEEC_Result %d returning status: %d",
        tee_status, ZX_ERR_INTERNAL);
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}

bool SysmemSecureMemServer::IsOverlap(const Range& a, const Range& b) {
  if (a.end() <= b.begin()) {
    return false;
  }
  if (b.end() <= a.begin()) {
    return false;
  }
  return true;
}

std::pair<SysmemSecureMemServer::Range, SysmemSecureMemServer::Range>
SysmemSecureMemServer::SubtractRanges(const SysmemSecureMemServer::Range& a,
                                      const SysmemSecureMemServer::Range& b) {
  // Caller must ensure this.
  ZX_DEBUG_ASSERT(IsOverlap(a, b));
  Range leftover_left = Range::BeginLength(a.begin(), 0);
  Range leftover_right = Range::BeginLength(a.end(), 0);
  if (b.begin() > a.begin()) {
    leftover_left = Range::BeginEnd(a.begin(), b.begin());
  }
  if (b.end() < a.end()) {
    leftover_right = Range::BeginEnd(b.end(), a.end());
  }
  return std::make_pair(leftover_left, leftover_right);
}
