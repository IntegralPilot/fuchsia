// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DL_TLSDESC_RUNTIME_DYNAMIC_H_
#define LIB_C_DLFCN_DL_TLSDESC_RUNTIME_DYNAMIC_H_

#include <lib/elfldltl/layout.h>
#include <lib/ld/tls.h>
#include <lib/ld/tlsdesc.h>

#include <algorithm>
#include <cstddef>
#include <memory>
#include <span>
#include <type_traits>

#include <fbl/alloc_checker.h>
#include <fbl/array.h>
#include <fbl/intrusive_double_list.h>

#include "diagnostics.h"

namespace [[gnu::visibility("hidden")]] dl {

using Elf = elfldltl::Elf<>;
using TlsModule = ld::abi::Abi<>::TlsModule;

// The argument to TLSDESC hooks is `const TlsDescGot&`.
using TlsDescGot = Elf::TlsDescGot<>;

// For dynamic TLS modules, each thread's copy of each dynamic PT_TLS segment
// is found in by index into an array of pointers.  That array itself is found
// as a normal thread_local variable dl::_dl_tlsdesc_runtime_dynamic_blocks
// (owned by libdl) using a normal IE access.  These TLSDESC hooks take two
// values: an index into that array, and an offset within that PT_TLS segment.
// They compute `_dl_tlsdesc_runtime_dynamic_blocks[index] + offset - $tp`.
//
// The TLSDESC ABI provides for one address-sized word to encode the argument
// to the TLSDESC hook (TlsDescGot::value).  There are two TLSDESC hooks that
// encode those two values in different ways:
//
//  * The "split" version encodes both values directly in the word by splitting
//    it in half bitwise.  The index is found in the high bits.  The offset is
//    found in the low bits.  This version is used whenever each value fits
//    into half the bits of the word.
//
//  * The "indirect" version uses the word as a pointer to an allocated data
//    structure containing the index and offset (TlsdescIndirect).
//
// **NOTE:** There is no special provision for synchronization so far.  These
// entry points assume that the current thread's blocks vector pointer is valid
// for any index they can be passed.

struct TlsdescIndirect {
  size_t index, offset;
};

// DynamicTlsPtr is a smart pointer type that's interoperable with assembly
// code accessing it as if it were a plain pointer type in memory.
class DynamicTlsPtr;

// For interacting with assembly code, a raw pointer to the first element of a
// contiguous array of DynamicTlsPtr is used.
using RawDynamicTlsArray = DynamicTlsPtr*;

// These are used or defined in assembly (see tlsdesc-runtime-dynamic.S), so
// they need unmangled linkage names.  From C++, they're still namespaced.
extern "C" {

// The runtime hooks access `_dl_tlsdesc_runtime_dynamic_blocks[index]`.
// _dl_tlsdesc_runtime_dynamic_blocks itself must stay constinit (preferably
// zero so it goes into .tbss) and trivially destructible to prevent ordering
// issues that C++ thread_local constructor/destructor semantics would have.
extern constinit thread_local RawDynamicTlsArray _dl_tlsdesc_runtime_dynamic_blocks
    [[gnu::tls_model("initial-exec")]];

// This hook splits the `value` field in half, with index in the high bits.
extern ld::TlsdescCallback _dl_tlsdesc_runtime_dynamic_split;

// This hook makes the `value` field a `const TlsdescIndirect*`.
extern ld::TlsdescCallback _dl_tlsdesc_runtime_dynamic_indirect;

}  // extern "C"

// Given the thread pointer of any thread, including one just being allocated
// and not actually started yet, access its _dl_tlsdesc_runtime_dynamic_blocks
// variable as an lvalue reference.  Just accessing the thread_local variable
// directly is the same as `TpToDynamicTlsBlocks(__builtin_thread_pointer())`.
inline RawDynamicTlsArray& TpToDynamicTlsBlocks(void* tp) {
  // Since all TLS accesses are IE model, there is a fixed offset from every
  // thread pointer.  The compiler would compute that with a GOT load and add
  // that to the thread pointer to take the address, but it will see that then
  // being subtracted from the current thread pointer and optimize away the
  // whole thread pointer part, so this is just the trivial GOT load.
  RawDynamicTlsArray* const blocks = &_dl_tlsdesc_runtime_dynamic_blocks;
  return *ld::TpRelative<RawDynamicTlsArray>(ld::TpRelativeToOffset(blocks), tp);
}

// DynamicTlsPtr is the type of elements in _dl_tlsdesc_runtime_dynamic_blocks.
// It's a standard-layout type that's nothing but a plain pointer, so that
// assembly code can use it with a known precise memory layout.  Otherwise it
// acts precisely like DynamicTlsPtr::UniquePtr.  (The only reason this type
// needs to exist is to ensure assembly-compatible implementation internals;
// std::unique_ptr doesn't formally guarantee that.)
class DynamicTlsPtr {
 public:
  // The std::unique_ptr to own a TLS block needs a custom deleter to use the
  // `operator delete[]` *function* directly, rather than the `delete[]`
  // *operator*.  This is to match the precise means of allocation, which has
  // to use the `operator new[]` function directly (not `new std::byte[n]`) so
  // as to use the overload that indicates (dynamic) alignment as well as size.
  // Since std::byte is both trivially-destructible and usable uninitialized,
  // there is no semantic difference between using the proper `new[]` and
  // `delete[]` operators (which in the general case ensure constructors and
  // destructors and formal C++ object lifetime rules) and using the underlying
  // allocator functions those operators call, which are called `operator
  // new[]` and `operator delete[]` to keep it confusing since they're neither
  // operators nor functions that take the same arguments as those operators.
  // But there is an important low-level difference, since the `new[]` and
  // `delete[]` operators implicitly use a hidden element count that's stored
  // as a size_t before the pointer (to allow `delete[]` to run the right
  // number of destructors); the underlying allocation includes space for this
  // hidden pointer, not just for the elements.  So it always matters to
  // manually pair the precise allocator and deallocator functions being used.
  //
  // Furthermore, which `operator delete[]` function signature is used to
  // deallocate a particular array should match which `operator new[]` function
  // signature was used to allocare it.  The compiler would generate the
  // `operator new[]` taking the dynamic alignment argument for `new T[n]` when
  // the static alignof(T) is > __STDCPP_DEFAULT_NEW_ALIGNMENT__; in that case,
  // its `delete[]` on T* would use the `operator delete[]` function that takes
  // the alignment (and it could choose or not to use the one that also takes
  // the size).  So this Deleter needs to recover the size and alignment of the
  // original allocation to call the correct `operator delete[]` signature.
  // The private BlockSizes helper class handles all this.
  struct Deleter {
    void operator()(std::byte* ptr) const {
      const TlsModule& module = BlockSizes::GetModule(ptr);
      BlockSizes{module}.Delete(ptr);
    }
  };
  using UniquePtr = std::unique_ptr<std::byte[], Deleter>;

  // Allocate a new, initialized block for the TlsModule.  This is the only way
  // a new pointer goes into a DynamicTlsPtr; otherwise only moves happen.
  // Hence, a DynamicTlsPtr always points to a block that already contains its
  // properly constinit-initialized values for some thread to start using (or,
  // later, that it is already using).
  [[nodiscard]] static DynamicTlsPtr New(fbl::AllocChecker& ac, const TlsModule& module) {
    const BlockSizes sizes{module};
    return sizes.New(ac, module);
  }

  constexpr DynamicTlsPtr() = default;
  DynamicTlsPtr(const DynamicTlsPtr&) = delete;

  constexpr DynamicTlsPtr(DynamicTlsPtr&& other) noexcept : ptr_{other.release()} {}

  DynamicTlsPtr& operator=(const DynamicTlsPtr&) = delete;
  DynamicTlsPtr& operator=(DynamicTlsPtr&& other) noexcept {
    reset();
    ptr_ = other.release();
    return *this;
  }

  void reset() { UniquePtr{release()}.reset(); }

  ~DynamicTlsPtr() { UniquePtr{ptr_}.reset(); }

  explicit constexpr operator bool() const { return ptr_; }

  // There are no get(), operator*(), or operator->() methods.  Once a TLS
  // block has been allocated, the only way to see a pointer inside it is to
  // acquire the valid span with knowledge of the TlsModule::tls_size() value
  // used to allocate this block.
  std::span<std::byte> contents(size_t tls_size) { return std::span{ptr_, tls_size}; }
  std::span<std::byte> contents(const TlsModule& module) { return contents(module.tls_size()); }

 private:
  // This helper class encapsulates all the arithmetic.  It's created by
  // extract size details from a TlsModule.  It then has enough information to
  // allocate (and initialize) or deallocate that module's TLS blocks.
  //
  // Each TLS block is allocated with extra space (before the returned pointer)
  // for its bookkeeping.  This is how the compiler's `new T[n]` and `delete[]`
  // usually work: storing the element count at `((size_t*)ptr)[-1]` by
  // allocating a slightly larger block from the underlying allocator, and
  // actually returning a pointer just inside that block.  This does the same,
  // with the same overhead: one word plus alignment padding.  But it instead
  // stores the TlsModule pointer whence both the size and the alignment of the
  // block allocated can be recomputed.  These blocks must be cleared out of
  // every thread and freed before the module can be unloaded and its TlsModule
  // pointer made invalid.
  class BlockSizes {
   public:
    using ModulePtr = const TlsModule*;

    BlockSizes() = delete;
    BlockSizes(const BlockSizes&) = default;

    // Compute sizes to allocate for this TlsModule.  Every block must be
    // aligned well enough to store the TlsModule pointer, in case that's more
    // than the requested alignment.  Its size must leave space for that
    // pointer to be before the aligned block of the requested size, so it
    // needs as much extra space as the total alignment to store the pointer.
    explicit BlockSizes(const TlsModule& module)
        : align_(std::max(sizeof(ModulePtr), module.tls_alignment())),
          size_{module.tls_size() + align_} {}

    // Do the actual allocation and initialization.  The module must be the
    // same one used in the constructor.  The returned pointer can be passed to
    // GetModule and Delete.
    DynamicTlsPtr New(fbl::AllocChecker& ac, const TlsModule& module) const {
      assert(module.tls_alignment() <= align_);
      assert(module.tls_size() <= size_ - align_);
      void* ptr = operator new[](size_, std::align_val_t{align_}, ac);
      if (!ptr) [[unlikely]] {
        return {};
      }
      DynamicTlsPtr block;
      block.ptr_ = static_cast<std::byte*>(ptr) + align_;
      ModulePointer(block.ptr_) = &module;
      ld::TlsModuleInit(module, {block.ptr_, module.tls_size()});
      return block;
    }

    // Recover the module pointer saved by New() from the DynamicTlsPtr::ptr_.
    static const TlsModule& GetModule(std::byte* ptr) {
      assert(ptr);
      return *ModulePointer(ptr);
    }

    // Given the DynamicTlsPtr::ptr_ value, recover the original pointer from
    // operator new[] and pass that to operator delete[].  The sizes recovered
    // via the saved TlsModule pointer match the operator new[] call exactly.
    void Delete(std::byte* ptr) const {
      operator delete[](ptr - align_, size_, std::align_val_t{align_});
    }

   private:
    static ModulePtr& ModulePointer(std::byte* ptr) {
      return reinterpret_cast<ModulePtr*>(ptr)[-1];
    }

    size_t align_, size_;
  };

  std::byte* release() { return std::exchange(ptr_, nullptr); }

  std::byte* ptr_ = nullptr;
};
static_assert(!std::is_copy_constructible_v<DynamicTlsPtr>);
static_assert(!std::is_copy_assignable_v<DynamicTlsPtr>);
static_assert(std::is_nothrow_move_constructible_v<DynamicTlsPtr>);
static_assert(std::is_move_assignable_v<DynamicTlsPtr>);
static_assert(std::is_standard_layout_v<DynamicTlsPtr>);
static_assert(sizeof(DynamicTlsPtr) == sizeof(std::byte*));

// An array to be installed in some thread's _dl_tlsdesc_runtime_dynamic_blocks
// should start as a managed pointer until its elements are all fully
// initialized with DynamicTlsPtr::New.
using SizedDynamicTlsArray = fbl::Array<DynamicTlsPtr>;
[[nodiscard]] inline SizedDynamicTlsArray MakeDynamicTlsArray(fbl::AllocChecker& ac, size_t n) {
  return fbl::MakeArray<DynamicTlsPtr>(&ac, n);
}

// When it's ready to be installed in a thread, it loses track of its size.
// (That is, the size is no longer accessible to us; however, delete[] will
// find the hidden size so it can run each element's destructor.)  This should
// be the only way to modify _dl_tlsdesc_runtime_dynamic_blocks for any thread.
// It returns an owned, but unsized, pointer to the previous array.  With the
// thread pointer of any live thread, TLSDESC callbacks can still be accessing
// the old array itself and/or any of the blocks it points to.  So the returned
// old array should only be destroyed in cases where it's well-understood to be
// safe.  Note that even moving-from (i.e. clearing) any of the old array's
// elements could let any racing thread to see a null pointer, even if the
// array itself is kept accessible.  So great care should be taken in deciding
// when to destroy this old array and how.  The straightforward case of just
// letting the returned array destroy its elements is correct for thread
// teardown (or unwinding an abortive thread creation).  At thread setup, it's
// reasonable to use this and just assert the returned pointer is null.  There
// should be no other uses of changing the installed pointer for a thread
// (aside from simulated thread setup and teardown in tests) not governed by a
// set of synchronization constraints around dangling pointer accesses.
using UnsizedDynamicTlsArray = std::unique_ptr<DynamicTlsPtr[]>;
[[nodiscard]] inline UnsizedDynamicTlsArray ExchangeRuntimeDynamicBlocks(  //
    SizedDynamicTlsArray blocks, void* tp = __builtin_thread_pointer()) {
  return UnsizedDynamicTlsArray{
      std::exchange(TpToDynamicTlsBlocks(tp), blocks.release()),
  };
}

// In testing cases, when an old array is recovered and its size is known, turn
// it into a SizedDynamicTlsArray again.
[[nodiscard]] inline SizedDynamicTlsArray AdoptDynamicTlsArray(  //
    UnsizedDynamicTlsArray blocks, size_t n) {
  return SizedDynamicTlsArray{blocks.release(), n};
}

// In testing cases, an old array of known size can be expanded by moving its
// existing blocks without deleting them but then deleting the old array
// itself.  This is only safe when it's known that no thread could be reading
// the old array (for example, it's the current thread's own array) and then
// it's fine if the thread does continue accessing the TLS blocks themselves
// either through previously-acquired pointers or through the new array that
// now owns those TLS blocks.  The old array is passed by lvalue reference so
// it can be left untouched if the allocation of the new array fails.
[[nodiscard]] inline SizedDynamicTlsArray EnlargeDynamicTlsArray(  //
    fbl::AllocChecker& ac, SizedDynamicTlsArray& old_array, size_t n) {
  assert(n > old_array.size());
  SizedDynamicTlsArray new_array = MakeDynamicTlsArray(ac, n);
  if (new_array) [[likely]] {
    std::ranges::move(old_array, new_array.begin());
    old_array.reset();
  }
  return new_array;
}

// Given the startup ABI, a global TLS module ID for a dynamically-loaded
// module, and its module descriptor, return a pointer to the beginning of the
// dynamic TLS block on the calling thread.
inline void* DynamicTlsData(const ld::abi::Abi<>& abi, Elf::size_type tls_module_id,
                            const TlsModule& tls_module) {
  DynamicTlsPtr& module_tls =
      _dl_tlsdesc_runtime_dynamic_blocks[abi.dynamic_tls_index(tls_module_id)];
  return module_tls.contents(tls_module).data();
}

// Allocate and initialize dynamic TLS blocks for a newly-created thread given
// a list of dynamic TLS module descriptors.
fit::result<Error> PrepareTlsBlocksForThread(std::span<const TlsModule> dynamic_tls_modules,
                                             void* tp = __builtin_thread_pointer());

}  // namespace dl

#endif  // LIB_C_DLFCN_DL_TLSDESC_RUNTIME_DYNAMIC_H_
