// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_THREADS_THREAD_STORAGE_H_
#define LIB_C_THREADS_THREAD_STORAGE_H_

#include <lib/elfldltl/tls-layout.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/threads.h>

#include <cassert>
#include <concepts>
#include <cstddef>
#include <span>
#include <string_view>

#include "../asm-linkage.h"  // TODO(https://fxbug.dev/342469121): see below
#include "../zircon/vmar.h"
#include "src/__support/macros/config.h"
#include "stack-abi.h"

struct __pthread;  // Forward declared for legacy "threads_impl.h".

namespace LIBC_NAMESPACE_DECL {

using Thread = ::__pthread;  // Legacy C type.

// ThreadStorage handles the memory allocation and ownership for Thread.  It's
// responsible for allocating all the various kinds of stacks, and the thread
// area that underlies both the implementation's private Thread Control Block
// (TCB) as well as the public Fuchsia Compiler ABI and ELF TLS.  ThreadStorage
// initializes only the parts of the TCB that are part of the public ABI
// (including all of ELF Initial Exec TLS) and then leaves the rest of the TCB
// zero-initialized for later use by libc internals.
//
// Initially it's used at process startup and thread creation.  It owns those
// allocations and cleans them up on destruction e.g. if thread creation fails.
// During the thread's lifetime, the ThreadStorage is moved into its Thread.
// This is a bit tricky, as the Thread object's own memory resides in one of
// the blocks that ThreadStorage owns.  So to destroy Thread, its ThreadStorage
// must be moved back out before explicitly calling ~Thread().
//
// ThreadStorage can only be default-constructed or moved.  Before Allocate()
// has returned successfully (or after it fails), it should only be destroyed
// and no other methods used (except for moves).
class ThreadStorage {
 public:
  constexpr ThreadStorage() = default;
  ThreadStorage(ThreadStorage&& other) { *this = std::move(other); }

  ThreadStorage& operator=(ThreadStorage&& other) {
    auto move_members = [this, &other](auto... m) {
      ((this->*m = std::exchange(other.*m, {})), ...);
    };
    move_members(&ThreadStorage::stack_size_, &ThreadStorage::guard_size_,
                 &ThreadStorage::thread_block_size_, &ThreadStorage::address_,
                 &ThreadStorage::vmar_);
    return *this;
  }

  ~ThreadStorage();

  // This allocates everything and holds ownership in this object.  If it
  // returns an error, some resources may now be owned but nothing more should
  // be done with the object but to destroy it (and thus reclaim them).
  //
  // When this returns success, the remaining methods return useful values.
  // The machine stack, unsafe stack (for the SafeStack ABI, enabled on all
  // machines), and shadow call stack (enabled on some machines), are all
  // allocated with guard pages and all-zeroes contents.  In the thread area:
  //
  //  * The unsafe stack pointer at $tp + ZX_TLS_UNSAFE_SP_OFFSET is set to
  //    the top of the unsafe stack.
  //
  //  * If the machine's TLS ABI has *$tp = $tp (like x86), that is set.
  //
  //  * The ZX_TLS_STACK_GUARD_OFFSET word still must be set.  For the initial
  //    thread, the caller will choose random bits; for new threads, the caller
  //    will copy the value found via its own $tp.
  //
  //  * All the ELF Initial Exec TLS data (static TLS) is initialized.
  //
  //  * The DTV for the dynamic TLS implementation is allocated and filled in.
  //    TODO(https://fxbug.dev/397084454): The new //sdk/lib/dl TLS runtime
  //    does not require a bespoke ABI contract for a DTV.
  //
  // The name string will become the ZX_PROP_NAME used for the VMO, and must
  // not be empty.  The VMAR handles are saved and used for destruction, so
  // they must remain valid for the lifetime of the object.
  //
  // The returned Thread* points somewhere inside the thread area block owned
  // by this ThreadStorage object, which also contains the static TLS area
  // already initialized with all the Initial Exec TLS data.  The ABI rules
  // govern where that is in relation to the thread pointer ($tp) and thereby,
  // indirectly, where the TCB lies relative to $tp.  (Note that here we
  // consider the Fuchsia Compiler ABI <zircon/tls.h> fixed slots, as well as
  // the $tp->self pointer on x86, to be part of the TCB--at one end of it or
  // the other--though nothing else about the TCB is part of any public ABI.)
  //
  // Returns ZX_ERR_INVALID_ARGS if the stack size is less than PTHREAD_STACK_MIN.
  zx::result<Thread*> Allocate(thrd_zx_create_handles_t allocate_from, std::string_view vmo_name,
                               PageRoundedSize stack, PageRoundedSize guard);

  // This frees just the blocks for all the stacks, leaving the thread block
  // (where the Thread object itself resides) intact until destruction.
  void FreeStacks();

  // Assert that this ThreadStorage looks like the result of a successful
  // Allocate(), possibly after FreeStacks() has been called.
  void AssertLive() const {
    assert(*vmar_.thread_block);
    assert(address_.thread_block != 0);
  }

  // This moves ownership of the ThreadStorage out of the Thread, making it
  // possible to destroy the Thread before destroying the ThreadStorage makes
  // its memory inaccessible.  (When Thread becomes a true C++ type, this will
  // be replaced with plain move-construction from its ThreadStorage member.)
  // If take_thread_block is false, leave the thread block intact.
  static ThreadStorage FromThread(Thread& thread, bool take_thread_block);

  // This moves ownership from this ThreadStorage into the Thread.  (When
  // Thread becomes a true C++ type, this will be replaced with plain
  // move-assignment to its ThreadStorage member.)
  void ToThread(Thread& thread) &&;

  // This returns the initial value for the machine SP.  This is always the
  // limit of the stack, where a push (`*--sp = ...`) will be the first thing
  // done.  This makes it appropriate for a call site, and on most machines for
  // the entry to a C function.  But on x86 it needs a return address pushed
  // before it can be used at a C function's entry point.
  uint64_t* machine_sp() const { return GrowsDown(address_.machine_stack); }

  // This returns the initial value for the unsafe SP.  This is always the
  // limit of the stack, which is always the protocol for function entry.
  // (This is already stored at $tp + ZX_TLS_UNSAFE_SP_OFFSET, too.)
  uint64_t* unsafe_sp() const { return GrowsDown(address_.unsafe_stack); }

  // This returns the initial value for the shadow call stack pointer.
  // That stack grows up, so the next operation will be `*sp++ = ...`.
  uint64_t* shadow_call_sp() const { return GrowsUp(address_.shadow_call_stack); }

  // These return each entire stack as a span.

  std::span<uint64_t> machine_stack() const {
    return GrowsDownSpan(address_.machine_stack, stack_size_.get(), guard_size_.get());
  }

  std::span<uint64_t> unsafe_stack() const {
    return GrowsDownSpan(address_.unsafe_stack, stack_size_.get(), guard_size_.get());
  }

  std::span<uint64_t> shadow_call_stack() const {
    return GrowsUpSpan(address_.shadow_call_stack, stack_size_.get());
  }

  std::span<std::byte> thread_block() const {
    const PageRoundedSize page_size = PageRoundedSize::Page();
    std::optional<PageRoundedSize> size_opt =
        std::optional{thread_block_size_} - PageRoundedSize::Pages(2);
    ZX_DEBUG_ASSERT_MSG(
        size_opt.has_value(),
        "Overflow in thread_block: thread_block_size=%zu, Pages(2)=%zu. This should never overflow since the thread_block_size_ should be large enough to contain the guard pages which is handled by Allocate.",
        thread_block_size_.get(), PageRoundedSize::Pages(2)->get());
    PageRoundedSize size = *size_opt;
    return {
        reinterpret_cast<std::byte*>(address_.thread_block + page_size.get()),
        size.get(),
    };
  }

  // These recover those spans from a Thread without modifying it.  (When
  // Thread becomes a true C++ type, these can be removed in favor of just
  // using the methods above on the ThreadStorage member in Thread.)
  static std::span<uint64_t> ThreadMachineStack(const Thread& thread);
  static std::span<uint64_t> ThreadUnsafeStack(const Thread& thread);
  static std::span<uint64_t> ThreadShadowCallstack(const Thread& thread);
  static std::span<std::byte> ThreadThreadBlock(const Thread& thread);

  PageRoundedSize stack_size() const { return stack_size_; }
  PageRoundedSize guard_size() const { return guard_size_; }

  // This is only used inside the Allocate() implementation, but it's public so
  // it can be used in a concept.
  void CommitBlock(auto&& block) { block.Commit(vmar_, address_); }

 private:
  // Each of the different blocks is allocated as a GuardedPageBlock.  That
  // object has nice destructor semantics and it's used during Allocate().  But
  // for storage, it's suboptimal to just use separate GuardedPageBlock objects
  // here.  GuardedPageBlock records the base address and size of the sub-VMAR,
  // and the (parent) VMAR handle through which to unmap it; struct alignment
  // means that's three whole words though half of one is unused.  Moreover,
  // the size is actually redundant between the different stack blocks since
  // they are all controlled by the ThreadAttributes stack and guard sizes.  On
  // the other hand, GuardedPageBlock just records the address and size of the
  // whole region (including guards), while ThreadStorage needs to record the
  // stack and guard sizes separately for pthread_getattr_np to recover.  So
  // what's most compact is to store the sizes separately, and then store the
  // address and VMAR handle for each block in parallel structs that don't
  // waste any space for alignment.  So a Perblock<T> struct is used in the
  // separate address_ and vmar_ members to cover all the blocks.  The sizes
  // are stored separately since the thread block has a single size while the
  // others are all computed from the same one pair of stack and guard sizes.
  template <typename T>
  struct PerBlock {
    [[no_unique_address]] T thread_block{};
    [[no_unique_address]] MachineStack<T> machine_stack{};
    [[no_unique_address]] IfShadowCallStack<T> shadow_call_stack{};
    [[no_unique_address]] IfSafeStack<T> unsafe_stack{};
  };

  template <typename... T>
  static void OnStacks(std::invocable<T&...> auto&& f, PerBlock<T>&... x) {
    f(x.machine_stack.value...);
    if constexpr (kShadowCallStackAbi) {
      f(x.shadow_call_stack.value...);
    }
    if constexpr (kSafeStackAbi) {
      f(x.unsafe_stack.value...);
    }
  }

  // The shadow call stack grows up with guard above, so the initial pointer is
  // just the base of the mapping.
  static uint64_t* GrowsUp(NoStack auto&&) { return nullptr; }
  static uint64_t* GrowsUp(SomeStack auto&& base) {
    return reinterpret_cast<uint64_t*>(base.value);
  }

  static std::span<uint64_t> GrowsUpSpan(NoStack auto&&, size_t) { return {}; }
  static std::span<uint64_t> GrowsUpSpan(SomeStack auto&& base, size_t size) {
    return {GrowsUp(base), size / sizeof(uint64_t)};
  }

  // The other stacks grow down with guard below, so the initial pointer is at
  // the end of the whole mapping.
  static uint64_t* GrowsDown(NoStack auto&&) { return nullptr; }
  uint64_t* GrowsDown(SomeStack auto&& base) const {
    return reinterpret_cast<uint64_t*>(  //
        base.value + guard_size_.get() + stack_size_.get());
  }

  static std::span<uint64_t> GrowsDownSpan(NoStack auto&&, size_t, size_t) { return {}; }
  static std::span<uint64_t> GrowsDownSpan(  //
      SomeStack auto&& base, size_t stack_size, size_t guard_size) {
    return {
        reinterpret_cast<uint64_t*>(base.value + guard_size),
        stack_size / sizeof(uint64_t),
    };
  }

  // This acquires the information from the dynamic linker or from a static
  // PIE's own PT_TLS segment.
  static elfldltl::TlsLayout<> GetTlsLayout()
      // TODO(https://fxbug.dev/342469121): This and InitializeTls only need
      // asm-linkage to be defined in a separate hermetic_source_set() that
      // allows the startup code to be shared between legacy and new
      // implementations.  When the legacy implementation is retired, these can
      // drop special linkage.
      LIBC_ASM_LINKAGE_DECLARE(GetTlsLayout);

  // Given that `thread_block.data() + tp_offset` will become $tp for the new
  // thread and that the space set aside for static TLS is all zero bytes now,
  // this will initialize all its PT_TLS segments properly.
  static void InitializeTls(std::span<std::byte> thread_block, size_t tp_offset)
      // TODO(https://fxbug.dev/342469121): see above
      LIBC_ASM_LINKAGE_DECLARE(InitializeTls);

  PageRoundedSize stack_size_, guard_size_;
  PageRoundedSize thread_block_size_;  // Includes two one-page guards.
  PerBlock<uintptr_t> address_;
  PerBlock<zx::unowned_vmar> vmar_;
};
static_assert(std::default_initializable<ThreadStorage>);
static_assert(std::movable<ThreadStorage>);

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_THREADS_THREAD_STORAGE_H_
