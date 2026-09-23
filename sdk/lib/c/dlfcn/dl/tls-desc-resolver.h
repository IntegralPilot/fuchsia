// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DL_TLS_DESC_RESOLVER_H_
#define LIB_C_DLFCN_DL_TLS_DESC_RESOLVER_H_

#include <lib/ld/tlsdesc.h>

#include <cassert>
#include <cstddef>
#include <memory>
#include <type_traits>

#include <fbl/intrusive_single_list.h>

#include "tlsdesc-runtime-dynamic.h"

namespace dl {

class Diagnostics;

// TlsDescResolver is used with elfldltl::MakeSymbolResolver, which specifies
// its API.  For static TLS, it uses the <lib/ld/tlsdesc.h> implementation and
// its relocation protocol (based on the <lib/ld/tls.h> passive ABI).  For
// dynamic TLS, it uses the tlsdesc-runtime-dynamic.h implementation with the
// relocation protocol described there.
class TlsDescResolver;

// When _dl_tlsdesc_runtime_dynamic_indirect is chosen for the runtime, it
// needs a TlsdescIndirect object to point to from the GOT.  Those objects'
// lifetimes must be bound to the lifetime of the module containing that GOT.
// TlsdescIndirectList is a std::unique_ptr-owning list of address-stable
// objects; relocation has stored pointers into those objects in the GOT.
class TlsdescIndirectStorage;
using TlsdescIndirectList = fbl::SinglyLinkedList<std::unique_ptr<TlsdescIndirectStorage>>;

// TlsDescResolver is a tiny, trivial object meant to be ephemeral.  It can be
// constructed directly in an rvalue argument to elfldltl::MakeSymbolResolver.
// The constructor requires two arguments: the current threshold between static
// TLS and dynamic TLS module IDs; and a reference to the TlsdescIndirectList
// owned by the RuntimeModule being relocated.
//
// The argument is the maximum TLS module ID handled by the <lib/ld/tlsdesc.h>
// runtime for static TLS cases.  The first dynamic PT_TLS module, stored in
// _dl_tlsdesc_runtime_dynamic_blocks[0], has ID max_static_tls_modid + 1,
//
// The TlsdescIndirectList only needs to be default-constructed, and then
// eventually destroyed when the module's GOT is definitely no longer in use
// (or never was).  It will never be touched again after this module's
// relocaton is complete; only elements' TlsdescIndirect contents will be read.
class TlsDescResolver : public ld::LocalRuntimeTlsDescResolver {
 public:
  using Base = ld::LocalRuntimeTlsDescResolver;

  TlsDescResolver(size_type max_static_tls_modid, TlsdescIndirectList& indirect_list)
      : max_static_tls_modid_(max_static_tls_modid), indirect_list_(indirect_list) {}

  // The overload for undefined weak references can be used as is.
  using Base::operator();

  // For defined TLS symbols, the base class handles the static TLS modules.
  // The dynamic TLS modules are handled in the Dynamic() method.
  fit::result<bool, TlsDescGot> operator()(auto& diag, const auto& defn) const {
    // The catch-all signature is needed to always override the base class, but
    // this can only be used with dl::Diagnostics.
    static_assert(std::is_same_v<Diagnostics, std::decay_t<decltype(diag)>>);
    if (defn.tls_module_id() <= max_static_tls_modid_) {
      return Base::operator()(diag, defn);
    }
    return Dynamic(diag,
                   ld::abi::Abi<>::dynamic_tls_index(defn.tls_module_id(), max_static_tls_modid_),
                   defn.symbol().value);
  }

 private:
  fit::result<bool, TlsDescGot> Dynamic(Diagnostics& diag, size_type index, size_type offset) const;

  size_type max_static_tls_modid_;
  TlsdescIndirectList& indirect_list_;
};

class TlsdescIndirectStorage final
    : public fbl::SinglyLinkedListable<std::unique_ptr<TlsdescIndirectStorage>>,
      private TlsdescIndirect {
 public:
  // These are only constructed once, and never moved or copied.
  TlsdescIndirectStorage() = delete;
  TlsdescIndirectStorage(const TlsdescIndirectStorage&) = delete;
  TlsdescIndirectStorage(TlsdescIndirectStorage&&) = delete;

  explicit TlsdescIndirectStorage(const TlsdescIndirect& values) noexcept
      : TlsdescIndirect{values} {}

  elfldltl::Elf<>::GotEntry<> got_value() const {
    return reinterpret_cast<uintptr_t>(static_cast<const TlsdescIndirect*>(this));
  }
};

}  // namespace dl

#endif  // LIB_C_DLFCN_DL_TLS_DESC_RESOLVER_H_
