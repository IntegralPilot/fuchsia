// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/machine.h>
#include <lib/fit/defer.h>
#include <lib/fit/result.h>

#include <cassert>
#include <concepts>
#include <utility>

#include "stack-abi.h"
#include "thread-storage.h"
#include "threads_impl.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

using TlsLayout = elfldltl::TlsLayout<>;

// Each of the blocks owned by ThreadStorage is represented by a Block
// specialization for some &ThreadStorage::member_.  Each class below meets
// this same API contract.
template <typename T>
concept BlockType = requires(T& t, ThreadStorage& storage,  //
                             PageRoundedSize tls_size, AllocationVmo& vmo) {
  // Returns the size needed in the AllocationVmo.
  { std::as_const(t).VmoSize(std::as_const(storage), tls_size) } -> std::same_as<PageRoundedSize>;

  // Maps the block into the VMAR from the AllocationVmo.
  {
    t.Map(std::as_const(storage), tls_size, thrd_zx_create_handles_t{}, vmo).is_ok()
  } -> std::same_as<bool>;

  // Commits the block to the successfully-allocated ThreadStorage object.
  { storage.CommitBlock(t) };
};

template <BlockType T>
using BlockTypeCheck = T;

template <auto Vmar, auto Address, auto CreateHandle>
class Block;

template <auto Vmar, auto Address, auto CreateHandle>
using BlockFor = BlockTypeCheck<Block<Vmar, Address, CreateHandle>>;

template <typename T>
concept ForThreadBlock = !SomeStack<T> && !NoStack<T>;

template <auto Vmar, auto Address, auto CreateHandle>
class BlockBase {
 public:
  void Commit(auto& vmars, auto& addresses) {
    vmars.*Vmar = block_.vmar().borrow();
    addresses.*Address = block_.release();
  }

 protected:
  template <typename T = std::byte>
  auto Allocate(thrd_zx_create_handles_t handles, AllocationVmo& vmo, PageRoundedSize size,
                PageRoundedSize guard_below, PageRoundedSize guard_above) {
    zx::unowned_vmar vmar{handles.*CreateHandle};
    return block_.Allocate<T>(vmar->borrow(), vmo, size, guard_below, guard_above);
  }

 private:
  GuardedPageBlock block_;
};

// This handles each of the stack blocks.
template <auto CreateHandle, class VS, class AS, SomeStack V, SomeStack A, V VS::* Vmar,
          A AS::* Address>
class Block<Vmar, Address, CreateHandle> : public BlockBase<Vmar, Address, CreateHandle> {
 public:
  PageRoundedSize VmoSize(const ThreadStorage& storage, PageRoundedSize tls_size) const {
    return storage.stack_size();
  }

  auto Map(const ThreadStorage& storage, PageRoundedSize tls_size, thrd_zx_create_handles_t handles,
           AllocationVmo& vmo) {
    PageRoundedSize guard_below, guard_above;
    if constexpr (kStackGrowsUp<A>) {
      guard_above = storage.guard_size();
    } else {
      guard_below = storage.guard_size();
    }
    return this->template Allocate<uint64_t>(  //
        handles, vmo, storage.stack_size(), guard_below, guard_above);
  }
};

// This handles shadow_call_stack_ or unsafe_stack_ when it's a no-op.
template <auto CreateHandle, class VS, class AS, NoStack V, NoStack A, V VS::* Vmar,
          A AS::* Address>
class Block<Vmar, Address, CreateHandle> {
 public:
  PageRoundedSize VmoSize(const ThreadStorage& storage, PageRoundedSize tls_size) const {
    return {};
  }

  zx::result<std::span<uint64_t>> Map(const ThreadStorage& storage, PageRoundedSize tls_size,
                                      thrd_zx_create_handles_t handles, AllocationVmo& vmo) {
    return zx::ok(std::span<uint64_t>{});
  }

  void Commit(auto& vmars, auto& addresses) {}
};

// This handles thread_block_, which includes both the TCB and the
// (runtime-dynamic) static TLS segments.  It always gets one-page guards both
// above and below, regardless of the configured guard size for the stacks.
template <auto CreateHandle, class VS, class AS, ForThreadBlock V, ForThreadBlock A, V VS::* Vmar,
          A AS::* Address>
class Block<Vmar, Address, CreateHandle> : public BlockBase<Vmar, Address, CreateHandle> {
 public:
  PageRoundedSize VmoSize(const ThreadStorage& storage, PageRoundedSize tls_size) const {
    return tls_size;
  }

  auto Map(const ThreadStorage& storage, PageRoundedSize tls_size, thrd_zx_create_handles_t handles,
           AllocationVmo& vmo) {
    const PageRoundedSize page_size = PageRoundedSize::Page();
    return this->Allocate(handles, vmo, tls_size, page_size, page_size);
  }
};

// This is the result of computations for allocating the thread block.  The
// PT_TLS p_align fields affect the computations within, but regardless the
// whole block is always page-aligned (so any p_align larger than a page is
// effectively treated as only a page).  This is allocated via GuardedPageBlock
// with guards both above and below to minimize chances of overruns out of TLS
// into something else or out of something else into TLS or the TCB.  (In the
// x86 kTlsNegative layout, overruns from TLS into the TCB are unguarded while
// elsewhere only underruns from TLS back into the TCB are what's unguarded.)
struct ThreadBlockSize {
  PageRoundedSize size;  // Total size to allocate for TLS + TCB.
  size_t tp_offset;      // Point $tp this far inside the block allocated.
};

// These abbreviations are used in the layout descriptions below:
//  * $tp: the thread pointer, as __builtin_thread_pointer() returns
//  * TLSn: the PT_TLS segment for static TLS module with ID n (n > 0)
//    - When the main executable has a PT_TLS of its own, then it has ID 1
//      and its $tp offset is fixed by an ABI calculation at static link time.
//  * Sn: the runtime address in a given thread where the TLSn block starts
//    - This must lie at an address 0 mod p_align, occupying p_memsz bytes.
//    - (Sn - $tp) is the value that appears in GOT slots for IE accesses.
//    - (S1 - $tp) for LE accesses is fixed by the ABI given TLS1.p_align.
//  * DTV: the Dynamic Thread Vector of traditional dynamic TLS implementations
//    - Not part of any public ABI contract, but sometimes described as if so.
//  * TCB: the Thread object
//    - This is a private implementation detail of libc (mostly).
//    - The <zircon/tls.h> Fuchsia Compiler ABI slots lie within this object,
//      though they are expressed as byte offsets from $tp.
//    - When kTlsLocalExecOffset is nonzero (ARM), it describes the final bytes
//      of the Thread object (psABI), so the Thread object in memory will
//      straddle $tp.  The ABI specifies that this much space above $tp is
//      reserved for the implementation and says nothing about what goes there.
//    - When kTpSelfPointer is nonzero (x86), the void* directly at $tp must be
//      set to the $tp address itself.  Only x86 has this and also only x86 has
//      kTlsNegative, so this first word above $tp is not part of the layout
//      calculations imposed by the ABI for TLS; instead, it's just a runtime
//      requirement and forms the first part of the Thread object.
//    - In traditional implementations, the second word above $tp (either in
//      the private TCB or in the ABI-specified reserved area) holds the DTV.
//      This has never been part of any ABI contract, except one private to
//      some particular dynamic linker, its __tls_get_addr implementation, and
//      its TLSDESC implementation hooks.
//  * T: the runtime address of the Thread object
//  * FC: the <zircon/tls.h> Fuchsia Compiler ABI slots
//    - <zircon/tls.h> defines two per-machine offsets from $tp for ABI use
//    - These are used by --target=*-fuchsia compilers by default, but are not
//      part of the basic machine ABI or the Fuchsia System ABI.  They are not
//      used by the startup dynamic linker or vDSO, nor provided by the system
//      program loader.  They are only provided here in libc.
//    - This implementation includes these in the TCB.
//  * psABI: ELF processor-specific ABI-mandated kTlsLocalExecOffset space
//    - The psABI for ARM and AArch64 specifies this, but not its use.
//    - Traditional implementations have used it just like the first two words
//      past $tp on x86: a $tp self-pointer (though nothing uses that); and the
//      DTV (a private implementation detail).
//  * align-pad: Padding to ensure $tp or any other field in the TCB is aligned
//    - This is for ensuring fields are maximally aligned to the largest TLSn
//      p_align.
//
// TCB is used to refer to the whole Thread object and also sometimes to
// distinguish FC and psABI from the rest of the Thread object.  A future
// implementation might have clearer distinctions in the data structures.

template <class TlsTraits = elfldltl::TlsTraits<>>
  requires(TlsTraits::kTlsNegative)
fit::result<fit::failed, ThreadBlockSize> ComputeThreadBlockSize(TlsLayout static_tls_layout) {
  // This layout is used only on x86 (both EM_386 and EM_X86_64).  To be
  // pedantic, the ABI requirement kTpSelfPointer indicates is orthogonal;
  // but it's related, and also unique to x86.  kTlsLocalExecOffset is also
  // zero on some machines other than x86, but it's especially helpful to
  // ignore a possible nonzero value in explaining the kTlsNegative layout.
  static_assert(TlsTraits::kTpSelfPointer);
  static_assert(TlsTraits::kTlsLocalExecOffset == 0);

  // *--------------------------------------------------------------------------*
  // |     unused    | TLSn | ... | TLS1 | $tp . (DTV) . FC . TCB | (align-pad) |
  // *---------------^------^-----^------^-----^-------^----^-----*-------------*
  // |               Sn     S2    S1    $tp=T  +8      +16  +32                 |
  // *--------------------------------------------------------------------------*
  // Note: T == $tp
  //
  // TLS offsets are negative, but the layout size is computed ascending from
  // zero with PT_TLS p_memsz and p_align requirements; then each segment's
  // offset is just negated at the end.  So the layout size is how many bytes
  // below $tp will be used.  The start address of each TLSn (Sn) must meet
  // TLSn's p_align requirement (capped at one OS page, as in PT_LOAD), but
  // TLS1 is always assigned first (closest to $tp).  The whole block must be
  // aligned to the maximum of the alignment requirements of each TLSn and the
  // TCB.  Then the offsets will be "aligned up" before being negated, such
  // that each address Sn is correctly aligned (the first TLSn block in memory,
  // for the largest n, will have the same alignment as $tp--the maximum of any
  // block).  Once the offsets have been assigned in this way, each TLSn block
  // starts at $tp + (negative) offset.  That may be followed by unused padding
  // space as required to make S(n+1) be 0 mod the TLS(n+1) p_align.  When TLS1
  // is the LE segment from the executable, that padding will be included in
  // its (negative offset) such that $tp itself will have at least the same
  // alignment as TLS1.  Layout mechanics require that $tp be thus aligned to
  // the maximum needed by any TLSn or the TCB.  If this is larger than the
  // TCB's own size, then there can be some unused space wasted at the very end
  // of the allocation: `alignof($tp) - sizeof(Thread)` bytes.
  //
  // The Fuchsia Compiler ABI <zircon/tls.h> slots are early in the TCB,
  // appearing in Thread just after the $tp slot and the second slot
  // traditionally used for the DTV (but formally just reserved for private
  // implementation use).
  //
  // Since the allocation will be in whole pages, there will often be some
  // unused space beyond any (usual small) amount wasted for alignment.  That
  // (usually larger) unused portion is placed at the beginning of the first
  // page, such that the end of the TCB is at (or close to) the end of the
  // whole allocation and the space "off the end" of TLSn (in the negative
  // direction from $tp) is available.  Reusing that space opportunistically
  // for PT_TLS segments of modules loaded later (e.g. dlopen) is fairly
  // straightforward, though notably a new PT_TLS segment with p_align greater
  // than static_tls_layout.alignment() can never be placed there (even if it
  // fits), as each thread's separate allocation can only be presumed to be
  // aligned to the original layout's requirement.
  const size_t tls_size = static_tls_layout.Align(  //
      static_tls_layout.size_bytes(), alignof(Thread));
  const size_t aligned_thread_size = static_tls_layout.Align(sizeof(Thread), alignof(Thread));

  std::optional<PageRoundedSize> allocation_size_opt =
      PageRoundedSize::From(tls_size) + aligned_thread_size;
  if (!allocation_size_opt.has_value()) [[unlikely]] {
    return fit::error(fit::failed());
  }

  const PageRoundedSize allocation_size = *allocation_size_opt;
  return fit::ok(ThreadBlockSize{
      .size = allocation_size,
      .tp_offset = allocation_size.get() - aligned_thread_size,
  });
}

template <class TlsTraits = elfldltl::TlsTraits<>>
  requires(!TlsTraits::kTlsNegative)
fit::result<fit::failed, ThreadBlockSize> ComputeThreadBlockSize(TlsLayout static_tls_layout) {
  // This style of layout is used on all machines other than x86.
  // The kTlsLocalExecOffset value differs by machine.
  //
  // *--------------------------------------------------------------------------*
  // |  (align-pad) | TCB, FC | [psABI] | TLS1 | ... | TLSn |     unused        |
  // *--------------^---------^---------^------^-----^------^-------------------*
  // |              T        $tp        S1     S2    Sn     (Sn+p_memsz)        |
  // *--------------------------------------------------------------------------*
  // Note: $tp - T == sizeof(Thread) - kTlsLocalExecOffset
  //         and
  //       $tp == AlignUp(sizeof(Thread) - kTlsLocalExecOffset, max_p_align)
  //
  // TLS offsets are positive, so the TCB (Thread) will sit just below $tp.
  // Any ABI-specified reserved area (ABI) is the tail of the layout of
  // Thread.  This means that Thread straddles $tp, which points to the ABI
  // reserved area that forms the last kTlsLocalExecOffset bytes of the
  // Thread object.  When kTlsLocalExecOffset is zero, $tp points exactly
  // just past Thread, which is also exactly the S1 address where TLS1 starts
  // (the LE segment assigned per ABI at static link time if there is one) .
  //
  // **Note:** It is always $tp _itself_ that must be aligned to the maximum
  // TLSn p_align!  When kTlsLocalExecOffset is nonzero, the static linker
  // starts its LE offset assignments there and then rounds up if the p_align
  // in the LE (executable) TLS1 is larger than that ABI-specified offset.
  //
  // kTpSelfPointer is not required for any non-x86 machine yet, but if it
  // were then the ABI's kTlsLocalExecOffset value would account for it.
  //
  // Note also that the 16 bytes just _below_ $tp are reserved by the Fuchsia
  // Compiler ABI for the two <zircon/tls.h> slots.
  //
  // Since the allocation will be in whole pages, there will often be some
  // unused space still available off the end of the last TLSn.  This layout
  // makes it easy to just iteratively do another static_tls_layout.  Assign to
  // see if a correctly-placed new block fits in the space left over.  There
  // may also be space at the beginning of the block if the TLS alignment is
  // greater than alignof(Thread), which will not be recovered for reuse.

  // The actual size of the ThreadBlockSize must fit two components.
  //
  // First, the TCB, which is just the implementation-specific Thread object.
  //
  // Recall the psABI reserved space is counted as the beginning of the overall
  // TLS area for size and alignment purposes, though it's not part of any TLSn
  // segment. So the TCB size can be reduced by this amount.
  constexpr size_t tcb_allocation_size = sizeof(Thread) - TlsTraits::kTlsLocalExecOffset;
  static_assert(offsetof(Thread, abi.stack_guard) - tcb_allocation_size ==
                ZX_TLS_STACK_GUARD_OFFSET);

  // To guarantee $tp remains maximally aligned to the largest TLSn alignment,
  // round up the TCB allocation size to this alignment such that $tp can
  // always remain maximally aligned. Due to rounding, it's possible that size
  // taken out of the `tcb_allocation_size` could be readded, potentially causing
  // `tcb_allocation_size` to either be `sizeof(Thread)` or larger than
  // `sizeof(Thread)`. If it's larger, then alignment padding will appear before
  // the TCB portion (shown in the diagram above).
  //
  // Thus, it's $tp that is aligned to the maximum alignment of any TLS segment,
  // not the TLS1 segment. (When TLS1 is from the executable, the psABI + alignment
  // gap between $tp and TLS1 is counted in the static link-time LE offset
  // calculations for TLS1 even though it's not part of TLS1's own size.)
  //
  // If the TLS layout's required alignment is less than `alignof(Thread)`, the
  // allocation will be aligned for Thread but this still meets alignment
  // requirements.
  const size_t aligned_tcb_allocation_size =
      static_tls_layout.Align(tcb_allocation_size, alignof(Thread));

  // Second, the TLS, which contains all the actual thread-specific data as well as any
  // required ABI offsets. When kTlsLocalExecOffset is non-zero, it straddles the
  // TCB, but this was accounted for earlier by removing it from the `tcb_allocation_size`.
  const size_t tls_size = static_tls_layout.size_bytes();

  // The TLS layout size is missing the TLS local exec offset. If there is no TLS
  // at all, then the `tls_size` will be zero. Otherwise, kTlsLocalExecOffset
  // is included in the size.
  assert(tls_size == 0 || tls_size > TlsTraits::kTlsLocalExecOffset);

  size_t block_size;
  if (add_overflow(aligned_tcb_allocation_size, tls_size, block_size)) [[unlikely]] {
    return fit::error(fit::failed());
  }

  // Check fundamental invariants: whatever block size we use, it must always
  // minimally be able to fit the whole Thread object (excluding the straddle),
  // and the TLS.
  assert(block_size >= sizeof(Thread) - TlsTraits::kTlsLocalExecOffset + tls_size);

  std::optional<PageRoundedSize> allocation_size_opt = PageRoundedSize::From(block_size);
  if (!allocation_size_opt.has_value()) [[unlikely]] {
    return fit::error(fit::failed());
  }
  return fit::ok(ThreadBlockSize{
      .size = *allocation_size_opt,

      // The location of $tp relative to the start of this thread block is simply
      // the TCB allocation size, which we ensured as maximally aligned earlier.
      .tp_offset = aligned_tcb_allocation_size,
  });
}

}  // namespace

zx::result<Thread*> ThreadStorage::Allocate(thrd_zx_create_handles_t allocate_from,
                                            std::string_view vmo_name, PageRoundedSize stack,
                                            PageRoundedSize guard) {
  using ThreadBlock = Block<  //
      &decltype(vmar_)::thread_block, &decltype(address_)::thread_block,
      &thrd_zx_create_handles_t::thread_block_vmar>;
  using MachineStackBlock = Block<  //
      &decltype(vmar_)::machine_stack, &decltype(address_)::machine_stack,
      &thrd_zx_create_handles_t::machine_stack_vmar>;
  using UnsafeStackBlock = Block<  //
      &decltype(vmar_)::unsafe_stack, &decltype(address_)::unsafe_stack,
      &thrd_zx_create_handles_t::security_stack_vmar>;
  using ShadowCallStackBlock = Block<  //
      &decltype(vmar_)::shadow_call_stack, &decltype(address_)::shadow_call_stack,
      &thrd_zx_create_handles_t::security_stack_vmar>;

  assert(allocate_from.machine_stack_vmar != ZX_HANDLE_INVALID);
  assert(allocate_from.security_stack_vmar != ZX_HANDLE_INVALID);
  assert(allocate_from.thread_block_vmar != ZX_HANDLE_INVALID);
  assert(!vmo_name.empty());

  if (stack.get() < PTHREAD_STACK_MIN) [[unlikely]] {
    return zx::error{ZX_ERR_INVALID_ARGS};
  }

  const auto tls_layout = GetTlsLayout();

  // We cannot guarantee larger alignments without over-allocating or using
  // special VMAR flags.
  assert(tls_layout.alignment() <= PageRoundedSize::Page().get());

  // The thread block size is a complex calculation, while the others depend
  // only on the stack and guard sizes.
  auto block_size_result = ComputeThreadBlockSize(tls_layout);
  if (block_size_result.is_error()) [[unlikely]] {
    return zx::error(ZX_ERR_NO_RESOURCES);
  }
  auto [thread_block_size, tp_offset] = *block_size_result;

  stack_size_ = stack;
  guard_size_ = guard;

  // Reset the stack and guard if Allocate ever fails.
  auto reset_stack_guard = fit::defer([&stack_size = stack_size_, &guard_size = guard_size_]() {
    stack_size = PageRoundedSize{};
    guard_size = PageRoundedSize{};
  });

  const std::optional<PageRoundedSize> thread_block_size_opt =
      PageRoundedSize::Pages(2) + thread_block_size;
  if (!thread_block_size_opt) {
    return zx::error{ZX_ERR_NO_RESOURCES};
  }
  thread_block_size_ = *thread_block_size_opt;

  std::span<std::byte> thread_block;

  // The VMO space and mapping is handled the same for each block.
  auto allocate_blocks = [&](ThreadBlock tcb, BlockType auto... stacks) -> zx::result<> {
    // Allocate a single VMO for all the blocks.
    std::optional<PageRoundedSize> vmo_size_opt =
        (tcb.VmoSize(*this, thread_block_size) + ... + stacks.VmoSize(*this, thread_block_size));
    if (!vmo_size_opt) {
      return zx::error{ZX_ERR_NO_RESOURCES};
    }
    PageRoundedSize vmo_size = *vmo_size_opt;

    zx::result vmo = AllocationVmo::New(vmo_size);
    if (vmo.is_error()) [[unlikely]] {
      return vmo.take_error();
    }

    auto map_one_block = [&]<BlockType B>(B& block) -> zx::result<> {
      zx::result result = block.Map(*this, thread_block_size, allocate_from, *vmo);
      if (result.is_error()) [[unlikely]] {
        return result.take_error();
      }
      if constexpr (std::is_same_v<B, ThreadBlock>) {
        thread_block = result.value();
      }
      return fit::ok();
    };

    // Map in each block's portion of that VMO.  After this, the mappings
    // (including guards) cannot be modified, only unmapped whole from above.
    auto map_blocks = [&](BlockType auto&&... blocks) {
      zx::result<> result = zx::ok();
      ((result = map_one_block(blocks)).is_ok() && ...);
      return result;
    };

    // Allocate the largest blocks first to minimize fragmentation in the VMAR.
    // The stacks all have the same size.
    if (zx::result<> result = tcb.VmoSize(*this, thread_block_size) >= stack
                                  ? map_blocks(tcb, stacks...)
                                  : map_blocks(stacks..., tcb);
        result.is_error()) [[unlikely]] {
      return result;
    }

    // Now that everything is mapped in, the ownership can move into this
    // ThreadStorage object.  Everything will be cleaned up on destruction.
    CommitBlock(tcb);
    (CommitBlock(stacks), ...);

    // Set the VMO's name to ease debugging.  The (only) VMO handle is dropped
    // after on return, so there won't be any way to change the name later.
    return zx::make_result(vmo->vmo.set_property(ZX_PROP_NAME, vmo_name.data(), vmo_name.size()));
  };

  // Allocate all the blocks together in a single VMO and map each separately.
  if (zx::result<> result = allocate_blocks(  //
          ThreadBlock{}, MachineStackBlock{}, UnsafeStackBlock{}, ShadowCallStackBlock{});
      result.is_error()) [[unlikely]] {
    // ZX_ERR_OUT_OF_RANGE can be yielded while attempting to map individual blocks
    // at different vmo_offsets. Large enough stacks or TCBs mapped in succession
    // can vause the vmo_offset + len to overflow yielding this error when mapping.
    if (result.error_value() == ZX_ERR_OUT_OF_RANGE) {
      return zx::error(ZX_ERR_NO_RESOURCES);
    }
    return result.take_error();
  }

  // Initialize the static TLS data from PT_TLS segments.
  InitializeTls(thread_block, tp_offset);

  // The location of the Thread object inside the thread block is part of the
  // complex sizing calculation, while all the stack pointers are just at one
  // end of their block or the other.
  Thread* thread = tp_to_pthread(thread_block.data() + tp_offset);
  if constexpr (elfldltl::TlsTraits<>::kTpSelfPointer) {
    void* tp = pthread_to_tp(thread);
    assert(&thread->head.tp == tp);
    thread->head.tp = reinterpret_cast<uintptr_t>(tp);
  }

  // The unsafe stack pointer is always part of the Thread rather than using a
  // machine register, so it can be initialized right here.
  thread->abi.unsafe_sp = reinterpret_cast<uintptr_t>(unsafe_sp());

  reset_stack_guard.cancel();
  return zx::ok(thread);
}

}  // namespace LIBC_NAMESPACE_DECL
