// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "thread-storage.h"

#include <zircon/assert.h>

#include <cinttypes>
#include <utility>

#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

void Unmap(zx::unowned_vmar& vmar, uintptr_t& base, PageRoundedSize size) {
  if (base != 0) {
    zx::result result = zx::make_result(vmar->unmap(base, size.get()));
    ZX_ASSERT_MSG(result.is_ok(), "zx_vmar_unmap(%#" PRIx32 ", %#" PRIxPTR ", %#zx: %s",
                  vmar->get(), base, size.get(), result.status_string());
    vmar = {};
    base = 0;
  }
}

}  // namespace

void ThreadStorage::FreeStacks() {
  std::optional<PageRoundedSize> size = stack_size_ + guard_size_;
  ZX_DEBUG_ASSERT_MSG(
      size.has_value(),
      "Overflow in FreeStacks: stack_size=%zu, guard_size=%zu. This should never overflow since this should be a valid combination on a successful ThreadStorage::Allocate.",
      stack_size_.get(), guard_size_.get());

  auto unmap = [size = *size](zx::unowned_vmar& vmar, uintptr_t& base) { Unmap(vmar, base, size); };
  OnStacks(unmap, vmar_, address_);
}

ThreadStorage::~ThreadStorage() {
  FreeStacks();

  // The thread block is destroyed last.  The address and size stored cover the
  // whole VMAR that contains the guard regions; the unmap implicitly destroys
  // that VMAR.  **NOTE:** The Thread object itself resides inside the thread
  // block, so the block must not be reclaimed until the ThreadStorage has been
  // moved out of the Thread!  FreeStacks() can be called first to free up most
  // of the storage while the Thread object needs to stay alive (until join or
  // detached final-exit).
  Unmap(vmar_.thread_block, address_.thread_block, thread_block_size_);
}

// Translate from the legacy C struct representation for ownership.
ThreadStorage ThreadStorage::FromThread(Thread& thread, bool take_thread_block) {
  constexpr auto take_vmar = [](zx_handle_t& storage_vmar) {
    zx::unowned_vmar vmar{std::exchange(storage_vmar, ZX_HANDLE_INVALID)};
    return vmar;
  };

  constexpr auto take_size = [](size_t& storage_size) {
    PageRoundedSize size;
    size.rounded_size_ = storage_size;
    assert(PageRoundedSize{storage_size} == size);
    storage_size = 0;
    return size;
  };

  ThreadStorage result;
  result.stack_size_ = take_size(thread.storage_stack_size);
  result.guard_size_ = take_size(thread.storage_guard_size);

  result.vmar_.machine_stack = take_vmar(thread.storage_handles.machine_stack_vmar);
  result.address_.machine_stack = std::exchange(thread.storage_machine_stack_address, 0);

#if HAVE_UNSAFE_STACK
  result.vmar_.unsafe_stack = take_vmar(thread.storage_handles.security_stack_vmar);
  result.address_.unsafe_stack = std::exchange(thread.storage_unsafe_stack_address, 0);
#endif

#if HAVE_SHADOW_CALL_STACK
  result.vmar_.shadow_call_stack = take_vmar(thread.storage_handles.security_stack_vmar);
  result.address_.shadow_call_stack = std::exchange(thread.storage_shadow_call_stack_address, 0);
#endif

  if (take_thread_block) {
    result.thread_block_size_ = take_size(thread.storage_thread_block_size);
    result.vmar_.thread_block = take_vmar(thread.storage_handles.thread_block_vmar);
    result.address_.thread_block = std::exchange(thread.storage_thread_block_address, 0);
  }

  return result;
}

void ThreadStorage::ToThread(Thread& thread) && {
  thread.storage_thread_block_size = std::exchange(thread_block_size_, {}).get();
  thread.storage_thread_block_address = std::exchange(address_.thread_block, {});
  thread.storage_handles.thread_block_vmar = std::exchange(vmar_.thread_block, {})->get();

  thread.storage_stack_size = std::exchange(stack_size_, {}).get();
  thread.storage_guard_size = std::exchange(guard_size_, {}).get();
  thread.storage_machine_stack_address = std::exchange(address_.machine_stack, {}).value;
  thread.storage_handles.machine_stack_vmar = std::exchange(vmar_.machine_stack, {}).value->get();

#if HAVE_UNSAFE_STACK
  thread.storage_unsafe_stack_address = std::exchange(address_.unsafe_stack, {}).value;
  thread.storage_handles.security_stack_vmar = std::exchange(vmar_.unsafe_stack, {}).value->get();
#endif

#if HAVE_SHADOW_CALL_STACK
  thread.storage_shadow_call_stack_address = std::exchange(address_.shadow_call_stack, {}).value;
  thread.storage_handles.security_stack_vmar =
      std::exchange(vmar_.shadow_call_stack, {}).value->get();
#endif
}

std::span<uint64_t> ThreadStorage::ThreadMachineStack(const Thread& thread) {
  return GrowsDownSpan(MachineStack<uintptr_t>{thread.storage_machine_stack_address},
                       thread.storage_stack_size, thread.storage_guard_size);
}

std::span<uint64_t> ThreadStorage::ThreadUnsafeStack(const Thread& thread) {
#if HAVE_UNSAFE_STACK
  return GrowsDownSpan(IfSafeStack<uintptr_t>{thread.storage_unsafe_stack_address},
                       thread.storage_stack_size, thread.storage_guard_size);
#endif
  return {};
}

std::span<uint64_t> ThreadStorage::ThreadShadowCallstack(const Thread& thread) {
#if HAVE_SHADOW_CALL_STACK
  return GrowsUpSpan(IfShadowCallStack<uintptr_t>{thread.storage_shadow_call_stack_address},
                     thread.storage_stack_size);
#endif
  return {};
}

std::span<std::byte> ThreadStorage::ThreadThreadBlock(const Thread& thread) {
  const PageRoundedSize page_size = PageRoundedSize::Page();
  std::optional<PageRoundedSize> size_opt =
      PageRoundedSize::From(thread.storage_thread_block_size) - PageRoundedSize::Pages(2);
  ZX_DEBUG_ASSERT_MSG(size_opt.has_value(),
                      "Overflow in ThreadThreadBlock: storage_thread_block_size=%zu, Pages(2)=%zu",
                      thread.storage_thread_block_size, PageRoundedSize::Pages(2)->get());
  PageRoundedSize size = *size_opt;
  return {
      reinterpret_cast<std::byte*>(  //
          thread.storage_thread_block_address + page_size.get()),
      size.get(),
  };
}

}  // namespace LIBC_NAMESPACE_DECL
