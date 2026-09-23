// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_ASM_LINKAGE_H_
#define LIB_C_ASM_LINKAGE_H_

#include <lib/arch/asm.h>

// LIBC_ASM_LINKAGE(SymbolName) is used as the linkage name for some
// LIBC_NAMESPACE::SymbolName that needs to be referenced either directly from
// assembly code or across an hermetic partial link boundary (where the build
// logic has to include verbatim linkage symbol names).  This is essentially a
// private name-mangling protocol that's trivial enough to be used in assembly
// and build code, unlike the complexity of C++ ABI name mangling.
//
// A function or variable used in assembly as LIBC_ASM_LINKAGE(SymbolName) and
// in build code as "${libc_namespace}_SymbolName" must be declared `extern` by
// some header file, inside `namespace LIBC_NAMESPACE_DECL` by adding the
// clause `LIBC_ASM_LINKAGE_DECLARE(SymbolName)` at the end of the declaration.
// The actual definition might be either in C++ code that includes that header,
// or in assembly code that uses the prefixed name via this macro.
//
// The optional second argument can be a literal prefix for the mangled name.
// This is used for the `__start_` and `__stop_` when the name isn't really a
// symbol but is instead an identifier-compatible section name.
#define LIBC_ASM_LINKAGE(name, ...) LIBC_ASM_LINKAGE_PASTE4(__VA_ARGS__, LIBC_NAMESPACE, _, name)
#define LIBC_ASM_LINKAGE_PASTE4(a, b, c, d) LIBC_ASM_LINKAGE_PASTE4_HELPER(a, b, c, d)
#define LIBC_ASM_LINKAGE_PASTE4_HELPER(a, b, c, d) a##b##c##d

#ifndef __ASSEMBLER__

#include "src/__support/macros/macro-utils.h"

// This is used like:
// ```
// namespace LIBC_NAMESPACE_DECL {
// void SomeFunction(T1, T2) LIBC_ASM_LINKAGE_DECLARE(SomeFunction);
// extern int SomeVariable LIBC_ASM_LINKAGE_DECLARE(SomeVariable);
// }  // namespace LIBC_NAMESPACE_DECL
// ```
#define LIBC_ASM_LINKAGE_DECLARE(name, ...) \
  __asm__(LIBC_ASM_LINKAGE_STRING(name, __VA_ARGS__)) LIBC_ASM_LINKAGE_ATTR
#define LIBC_ASM_LINKAGE_STRING(name, ...) LLVM_LIBC_STRINGIFY(LIBC_ASM_LINKAGE(name, __VA_ARGS__))
#ifdef __clang__
// When definitions or references are in assembly or user.basic code, they
// won't have tagged symbol values.  This can't use [[...]] syntax when it
// follows __asm__(...).
#define LIBC_ASM_LINKAGE_ATTR __attribute__((no_sanitize("hwaddress")))
#else
#define LIBC_ASM_LINKAGE_ATTR
#endif

#else  // clang-format off

// This replaces `.function` for a function defined in LIBC_NAMESPACE_DECL.
.macro .llvm_libc_function name, args:vararg
  .function LIBC_ASM_LINKAGE(\name), global, \args
.endm

// This makes a namespaced alias of an `.llvm_libc_function`-defined symbol.
.macro .llvm_libc_alias name, alias
  .alias LIBC_ASM_LINKAGE(\alias), LIBC_ASM_LINKAGE(\name)
.endm

// This puts an `.llvm_libc_function`-defined symbol into the public libc ABI.
// The optional \public argument can be used for a different public alias name.
.macro .llvm_libc_public name, public=, scope=export, type=function
#if defined(LIBC_COPT_PUBLIC_PACKAGING)
  .ifb \public
    .label \name, \scope, \type, LIBC_ASM_LINKAGE(\name)
  .else
    .label \public, \scope, \type, LIBC_ASM_LINKAGE(\name)
  .endif
#endif
.endm

#endif  // clang-format on

#endif  // LIB_C_ASM_LINKAGE_H_
