// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_MODULE_H_
#define LIB_LD_MODULE_H_

#include <lib/elfldltl/init-fini.h>
#include <lib/elfldltl/link-map-list.h>
#include <lib/elfldltl/phdr.h>
#include <lib/elfldltl/svr4-abi.h>
#include <lib/elfldltl/symbol.h>

#include <algorithm>
#include <array>
#include <concepts>
#include <cstddef>
#include <cstdint>
#include <iterator>
#include <ranges>
#include <span>
#include <type_traits>

#include "abi.h"

namespace ld {
namespace abi {

// ld::abi::Abi::Module holds all the information about an ELF module that's
// still relevant at runtime after it's been loaded and dynamically linked.
// This is enough for basic dl_iterate_phdr and dlsym implementations and the
// like to interrogate the initial-exec set of modules.  A runtime loading
// implementation can also provide data about runtime modules in this format.
//
// This type is defined in the ld::abi namespace because these layouts form
// part of the quasi-public, quasi-stable "passive" data ABI between startup or
// OOP dynamic linking and other runtime dynamic linking support code that
// might live in libc.so or libdl.so.  Note that this brings a few toolkit API
// types, which are not otherwise presumed ABI stable nor even public outside a
// single module, into the ld::abi regime for quasi-public, quasi-stable ABI.
//
// The leading portion of ld::abi::Abi::Module matches the long-standing de
// facto standard ABI layout of `struct link_map` from SVR4.  The doubly-linked
// list structure of `link_map::l_prev` and `link_map::l_next` is maintained to
// navigate the initial-exec set in load order (which is also symbol resolution
// precedence order for standard initial-exec symbol resolution).  The pointers
// are actually to ld::abi::Module structures with leading `link_map` portions.
// Maintaining this traditional format for all loaded modules in the process
// memory image can enable debuggers that know the de facto standard format to
// decode the `link_map` list found in the traditional `struct r_debug`.

template <class Elf, class AbiTraits>
struct Abi<Elf, AbiTraits>::Module {
  // This can be used in an initializer to allow the variable to go into bss.
  // InitLinkerZeroInitialized() must be called before the variable is used.
  static constexpr Module LinkerZeroInitialized() {
    return Module{.symbols{elfldltl::kLinkerZeroInitialized}};
  }

  constexpr void InitLinkerZeroInitialized() { symbols.InitLinkerZeroInitialized(); }

  // This is known to be the first member in the struct layout.  It forms the
  // old de facto ABI from SVR4 (traditionally `struct link_map` in <link.h>)
  // for enumerating the modules and their load/symbol-resolution order through
  // its doubly-linked list structure.  It also holds the load bias (.addr); a
  // pointer (.name) to the NUL-terminated DT_SONAME (or other) name string by
  // which the module was loaded; and a pointer (.ld) to the absolute runtime
  // address of the PT_DYNAMIC segment (aka .dynamic section) in this module's
  // load image.  The .dynamic data has already been parsed and validated so
  // it's safe to rely on the DT_NULL terminator being found by iteration.
  //
  // It's traditional for dynamic linkers to use this well-known format as the
  // leading portion of their own internal data structure.  For example, some
  // users may assume that the void* value returned by `dlopen` is in fact the
  // `struct link_map*` for the module just fetched.  This is not good practice
  // and no standard or documentation has ever made guarantees about the void*
  // values used in <dlfcn.h> interfaces being used in any such ways.  But it
  // makes sense that we follow suit here.  The `struct link_map` list serves
  // as the ld::abi::Module list as well.
  LinkMap link_map;

  // The rest of the ld::abi::Module layout is a distinct extension that doesdsy
  // not overlap with any historical ABI.  (Traditional uses that have longer
  // data structures prefixed with `struct link_map` are all private formats.)
  //
  // However, it does form the core of the quasi-public, quasi-stable ABI for
  // ld.so; see <lib/ld/abi.h> for full details on the ABI stability model.

  // This module's whole-page load image occupies the absolute virtual address
  // range [vaddr_start, vaddr_end).  The vaddr_start value is most often
  // redundant with link_map.addr (the load bias), but is not required to be.
  Addr vaddr_start = 0;
  Addr vaddr_end = 0;

  // This points to the module's program headers in some read-only memory,
  // usually in its own load image as located by PT_PHDR.  (The span may be
  // empty if the program headers are not visible in memory, and this won't
  // always prevent a module from being loaded.  But any such unusual modules
  // with that limitation may have unexpected behavior due to their metadata
  // pointers such as PT_GNU_EH_FRAME not being found at runtime.)
  Span<const Phdr> phdrs;

  // This collects information about the dynamic symbol table and can be
  // used to look up symbols.  See <lib/elfldltl/symbol.h> for details.
  Type<elfldltl::SymbolInfo> symbols;

  // Cached and hashed for quick comparison; possibly empty.
  Type<elfldltl::Soname> soname;

  // This lists the initializer functions this module expects to have run after
  // it's loaded.  This information isn't really needed after startup (or later
  // dynamic module loading).  But some means is needed to communicate this
  // information from the dynamic linker that gleans (and relocates) it to the
  // runtime code that calls initializers after C library setup.  The read-only
  // module data here is the means already at hand, and this data is only a few
  // words.  So it's just kept here permanently with everything else.
  Type<elfldltl::InitFiniInfo> init;

  // This lists the finalizer functions this module expects to have run at
  // program exit or when it's dynamically unloaded (if that's possible).
  Type<elfldltl::InitFiniInfo> fini;

  // Each module that has a PT_TLS segment of its own is assigned a module ID,
  // which is a nonzero index.  This value is zero if the module has no PT_TLS.
  // Note that a module's code might use TLS relocations (resolved to external
  // symbols) even if that module has no PT_TLS segment of its own.
  Addr tls_modid = 0;

  // If nonempty, this is the (first) NT_GNU_BUILD_ID note payload (not
  // including Elf::Nhdr or name parts): just the build ID bytes themselves.
  // This is simply the result of parsing PT_NOTE segments in the phdrs that
  // presumably point to read-only data in the module's load image, which can
  // always be repeated; this just caches the parsing result from load time.
  Span<const std::byte> build_id;

  // Each and every module gets a "module ID" number that's used in symbolizer
  // markup contextual elements describing the module.  These are expected to
  // be arbitrary integers, probably small, that are unique within the process
  // at a given moment.  The initial-exec dynamic linker assigns monotonically
  // increasing numbers to each module in the order they're loaded and linked
  // via the `link_map` member above, from zero.  For additional runtime-loaded
  // modules, it's reasonable to take the tail initial-exec module's ID and
  // increase from there for every new module loaded, without reusing old IDs
  // when modules are unloaded.
  Word symbolizer_modid = 0;

  // This is true if the module participates in symbolic resolution. If false,
  // the module will still be part of the unwinding domain, and therefore will
  // still be visible to dl_iterate_phdr.
  bool symbols_visible = false;

  // This makes explicit the alignment padding that would be here implicitly.
  // It can be reduced to introduce new flags or small integers without risk of
  // backward ABI incompatibility if zero is the safe default for new consumers
  // of old passive ABI data from an older producer.
  std::array<typename Elf::Byte, sizeof(Addr) - ((sizeof(Word) + sizeof(bool)) % sizeof(Addr))>
      reserved_zero{};

  // <lib/ld/remote-abi-transcriber.h> introspection API.

  using AbiLocal = Abi<Elf, elfldltl::LocalAbiTraits>::Module;

  template <template <class...> class Template>
  using AbiBases = Template<>;

  template <template <auto...> class Template>
  using AbiMembers =
      Template<&Module::link_map, &Module::vaddr_start, &Module::vaddr_end, &Module::phdrs,
               &Module::symbols, &Module::soname, &Module::init, &Module::fini, &Module::tls_modid,
               &Module::build_id, &Module::symbolizer_modid, &Module::symbols_visible,
               &Module::reserved_zero>;
};

}  // namespace abi

// elfldltl::LinkMapList-compatible Traits type for abi::Abi<>::Module lists.
template <class Elf = elfldltl::Elf<>>
using AbiModuleListTraits =
    elfldltl::LinkMapListInFirstMemberTraits<const typename abi::Abi<Elf>::Module>;

// This provides a container-like view on the doubly-linked list of modules.
template <class Elf = elfldltl::Elf<>>
using AbiModuleList =
    elfldltl::LinkMapList<const typename abi::Abi<Elf>::Module, AbiModuleListTraits<Elf>>;

// This returns the ld::AbiModuleList for an ld::Abi::Abi<>.
template <class Elf = elfldltl::Elf<>>
constexpr AbiModuleList<Elf> AbiLoadedModules(const abi::Abi<Elf>& abi) {
  return AbiModuleList<Elf>(abi.loaded_modules.get());
}

// This returns a view similar to AbiModuleList, but only for modules where
// symbols_visible is true.
template <class Elf = elfldltl::Elf<>>
constexpr auto AbiLoadedSymbolModules(const abi::Abi<Elf>& abi) {
  using Module = abi::Abi<Elf>::Module;
  return std::views::filter(AbiLoadedModules(abi), &Module::symbols_visible);
}

// This uses the symbolizer_markup::Writer API to emit the contextual elements
// describing this Module.  It requires the page size used to load the module.
template <class Module, class Writer>
constexpr Writer& ModuleSymbolizerContext(
    Writer& writer, const Module& module,
    typename decltype(module.vaddr_start)::value_type page_size, std::string_view prefix = {}) {
  using size_type = decltype(page_size);
  using Phdr = std::decay_t<decltype(module.phdrs.front())>;

  std::string_view name = module.link_map.name.get();
  if (name.empty()) {
    name = "<application>";
  }
  writer  //
      .Prefix(prefix)
      .ElfModule(module.symbolizer_modid, name, module.build_id.get())
      .Newline();

  const size_type load_bias = module.link_map.addr;
  const uint32_t modid = module.symbolizer_modid;
  for (const Phdr& phdr : module.phdrs) {
    const size_type vaddr = phdr.vaddr & -page_size;
    const size_type memsz = (vaddr + phdr.memsz + page_size - 1) & -page_size;
    writer  //
        .Prefix(prefix)
        .LoadImageMmap(vaddr + load_bias, memsz, modid,
                       {.read = (phdr.flags & Phdr::kRead) != 0,
                        .write = (phdr.flags & Phdr::kWrite) != 0,
                        .execute = (phdr.flags & Phdr::kExecute) != 0},
                       vaddr)
        .Newline();
  }

  return writer;
}

template <class Module>
constexpr bool ModuleUsesStaticTls(const Module& module) {
  return (module.symbols.flags() & elfldltl::ElfDynFlags::kStaticTls) ||
         (module.symbols.flags1() & elfldltl::ElfDynFlags1::kPie);
}

constexpr bool ModuleContainsVaddr(const auto& module, decltype(module.vaddr_start()) vaddr) {
  return module.vaddr_start <= vaddr && vaddr < module.vaddr_end;
}

// This returns the iterator in the range where the module containing vaddr was
// found, or modules.end() if no module contains vaddr.
constexpr std::input_iterator auto FindModuleByVaddr(
    std::ranges::input_range auto&& modules,
    decltype(std::ranges::begin(modules)->vaddr_start()) vaddr) {
  return std::ranges::find_if(
      std::forward<decltype(modules)>(modules),
      [vaddr](const auto& module) { return ModuleContainsVaddr(module, vaddr); });
}

// Given a module, call the callback as `bool(std::span<std::byte>)` to cover
// all the writable segment data in the module, excluding the RELRO region.
// This covers all the module's global (and static) variables (excluding TLS)
// that could hold root pointers for GC or leak detection, for example.
constexpr bool OnModuleWritableSegments(const auto& module,
                                        std::invocable<std::span<std::byte>> auto&& callback) {
  static_assert(std::is_invocable_r_v<bool, decltype(callback), std::span<std::byte>>);
  auto on_segment = [load_bias = module.link_map.addr(), &callback](  //
                        uintptr_t start, size_t size) -> bool {
    std::byte* const data = reinterpret_cast<std::byte*>(start + load_bias);
    return callback(std::span{data, size});
  };
  return elfldltl::OnPhdrWritableSegments(module.phdrs.get(), on_segment);
}

}  // namespace ld

#endif  // LIB_LD_MODULE_H_
