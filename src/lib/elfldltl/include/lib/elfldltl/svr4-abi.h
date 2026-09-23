// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_SVR4_ABI_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_SVR4_ABI_H_

#include "abi-ptr.h"
#include "layout.h"

namespace elfldltl {

// These type layouts are not formally parts of the ELF format, but rather
// de facto standard ABI types from the original SVR4 implementation that
// introduced ELF that have been kept compatible in other implementations
// historically.  In SVR4 and other systems <link.h> declares these types.

// Expected value for RDebug::version field.
inline constexpr uint32_t kRDebugVersion = 1;

// Recognized values for RDebug::state field.  See below.
enum class RDebugState : uint32_t {
  // This indicates that no loading or unloading is in progress, so
  // everything is in a consistent state.
  kConsistent = 0,

  // This indicates that a new module is about to be loaded.  At the next
  // stop when state is kConsistent, the map list can be expected to have
  // an additional module (usually on the end of the list).
  kAdd = 1,

  // This indicates that a new module is about to be unloaded.  At the next
  // stop when state is kConsistent, the map list can be expected to have
  // fewer modules.
  kDelete = 2,
};

// This is the traditional `struct r_debug` that SVR4 and other systems declare
// in <link.h>.  In the original scheme, the DT_DEBUG element in the PT_DYNAMIC
// (.dynamic) data would be updated to point to this in memory so that a
// debugger could examine that in memory to find it.  However, debuggers also
// began looking for known symbol names like `_r_debug` in the dynamic linker
// to find this, so the de facto expectation is that it has that symbol.  The
// scheme of modifying DT_DEBUG is not feasible when .dynamic is read-only, so
// it's not used on systems like Fuchsia with `-z rodynamic` on by default.
template <ElfClass Class, ElfData Data>
template <class AbiTraits>
struct Elf<Class, Data>::RDebug {
  // A version number > 0.  The kRDebugVersion indicates this layout.
  // Higher version numbers would indicate a layout that's compatible with
  // this but has additional fields.
  Word version;

  // The list of modules currently loaded.  This is a doubly-linked list in
  // load order, with a null prev at the beginning and a null next at the
  // end.  For the initial-exec set of modules, the main executable is
  // first in the list and the remainder matches a breadth-first pre-order
  // traversal of the DT_NEEDED graph.
  AbiPtr<const LinkMap<AbiTraits>, Elf, AbiTraits> map;

  // This is a PC location where a breakpoint can be set.  When a dynamic
  // linker does loading or unloading, it executes this PC both before and
  // after the operation with different `.state` values.  A debugger is
  // expected to read out this address, set a breakpoint there, and then
  // when that breakpoint fires to examine `.state` and `.map` to assess
  // the new state of what modules are now loaded.
  Addr brk;

  // This is normally kConsistent.  When a dynamic linker is loading new
  // modules, it sets `.state` to kAdd before executing the `.brk` PC; when
  // unloading modules, it sets `.state` to kRemove before executing the
  // `.brk` PC.  When the operation is finished, it sets `.state` back to
  // kConsistent before executing the `.brk` PC for a second time.
  EnumField<RDebugState, kSwap> state;

  // This is the load bias of the dynamic linker itself.
  Addr ldbase;

  // <lib/ld/remote-abi-transcriber.h> introspection API.

  using AbiLocal = RDebug<LocalAbiTraits>;

  template <template <class...> class Template>
  using AbiBases = Template<>;

  template <template <auto...> class Template>
  using AbiMembers =
      Template<&RDebug::version, &RDebug::map, &RDebug::brk, &RDebug::state, &RDebug::ldbase>;
};

// This is the traditional `struct link_map` that SVR4 and other systems
// declare in <link.h>.  One of these describes each module loaded, forming a
// doubly-linked list in load order: first the main executable, then its
// DT_NEEDED dependency graph, then modules loaded later at runtime.
template <ElfClass Class, ElfData Data>
template <class AbiTraits>
struct Elf<Class, Data>::LinkMap {
  using ElfLayout = Elf;

  // This is the load bias, meaning the difference between runtime addresses in
  // this process and the link-timeaddress that they appear in this module's
  // ELF metadata (program headers, symbol table, etc.).  More precisely, it's
  // the difference between the runtime address of the module's load image and
  // the first PT_LOAD segment's p_vaddr rounded down to the runtime page size.
  // In most ELF modules the latter is zero and so the load bias is usually
  // equal to the load address; but this does not have to be so.
  Addr addr;

  // This points to a NUL-terminated C string.  For the main executable, it's
  // usually the empty string "".  For others, it's the "name" of the module.
  // What that means exactly depends on how the module was loaded and differs
  // on different systems.  On traditional POSIX systems, it's usually an
  // absolute path name.  On Fuchsia, it's just the name that was presented to
  // the loader service protocol, usually just the plain DT_NEEDED string
  // (which in turn usually matches the DT_SONAME string in the module itself).
  AbiPtr<const char, Elf, AbiTraits> name;

  // This is the runtime address of the PT_DYNAMIC segment (.dynamic) section
  // of this module.  The dynamic linker has already processed the data, so
  // it's guaranteed that there is a proper DT_NULL element terminating the
  // array, though there is no other indication here of the number of elements.
  //
  // Some dynamic linkers on some systems will apply the load bias to address
  // values in .dynamic entries and update them in place, while others do not;
  // on systems like Fuchsia where `-z rodynamic` is on by default, this points
  // to read-only space that cannot have been modified to apply the load bias.
  // A reasonable heuristic is to check whether an address value in an entry
  // falls within the bounds of the load image based on its PT_LOAD headers and
  // its load bias; if it does, it's probably already relocated and if not it's
  // probably unrelocated.
  AbiPtr<const Dyn, Elf, AbiTraits> ld;

  // These members form a doubly-linked list.  The `prev` of the first element
  // in the list and the `next` of the last element in the list are both null.
  // At startup, this lists the main executable and its dependencies in load
  // order, which is also symbol resolution order for that set of modules
  // (known as the "initial-exec set").  If more modules are loaded after
  // startup, they are added on the end of the list and may be removed later
  // from anywhere in the list after the initial-exec set, depending on the
  // order of adding and removing modules and how they may share dependencies.
  AbiPtr<LinkMap, Elf, AbiTraits> next, prev;

  // <lib/ld/remote-abi-transcriber.h> introspection API.

  using AbiLocal = LinkMap<LocalAbiTraits>;

  template <template <class...> class Template>
  using AbiBases = Template<>;

  template <template <auto...> class Template>
  using AbiMembers =
      Template<&LinkMap::addr, &LinkMap::name, &LinkMap::ld, &LinkMap::next, &LinkMap::prev>;
};

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_SVR4_ABI_H_
