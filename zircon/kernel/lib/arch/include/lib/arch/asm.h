// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ASM_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ASM_H_

// This may be overridden by a $current_cpu/include/lib/arch/asm.h
// file that does #include_next to get this one before adding more.

#include <lib/arch/internal/asm.h>

#define DW_CFA_def_cfa_expression 0xf  // ULEB128 length, bytes
#define DW_CFA_expression 0x10         // ULEB128 regno, ULEB128 length, bytes
#define DW_CFA_val_expression 0x16     // ditto
#define DW_OP_bra 0x28                 // signed 2-byte constant
#define DW_OP_breg(n) (0x70 + (n))     // n <= 31; SLEB128 offset
#define DW_OP_bregx 0x92               // ULEB128 regno, SLEB128 offset
#define DW_OP_consts 0x11
#define DW_OP_deref 0x06
#define DW_OP_drop 0x13
#define DW_OP_dup 0x12
#define DW_OP_lit(n) (DW_OP_lit0 + (n))  // n <= 31
#define DW_OP_lit0 0x30
#define DW_OP_minus 0x1c
#define DW_OP_mul 0x1e
#define DW_OP_plus 0x22
#define DW_OP_plus_uconst 0x23
#define DW_OP_swap 0x16
#define DW_OP_xor 0x27

#define ULEB128_1BYTE(n) (n)
#define ULEB128_2BYTE(n) (((n) & 0x7f) | 0x80), ((n) >> 7)
#define SLEB128_1BYTE(n) ((n) & 0x7f)
#define SLEB128_2BYTE(n) (((n) & 0x7f) | 0x80), (((n) >> 7) & 0x7f)

// Check that `n` is in the representable range of the one-byte SLEB128 encoding.
#define SLEB128_IS_1BYTE(n) ((n) > -0x40 && (n) < 0x3f)
// Check that `n` is in the representable range of the two-byte SLEB128 encoding.
#define SLEB128_IS_2BYTE(n) (!SLEB128_IS_1BYTE(n) && ((n) > -0x2000 && (n) < 0x1fff))

#ifdef __ASSEMBLER__  // clang-format off

/// Defines an ELF symbol at the current assembly position (or with an
/// arbitrary value), with specified scope and (optional) type.
///
/// Parameters
///
///   * name
///     - Required: Symbol name to define.
///
///   * scope
///     - Optional: One of these strings:
///       - `local`: The symbol is visible only within this assembly file.
///       - `global`: The symbol is visible throughout this linked module.
///       - `export`: The symbol is exported for dynamic linking (user mode).
///       - `weak`: Like `export`, but can be overridden.
///     - Default: `local`
///
///   * type
///     - Optional: ELF symbol type.  This only has practical effect when
///     dynamic linking is involved, but it's convention to set it consistently
///     to `function` or `object` for named entities with an st_size field,
///     and leave it the default `notype` only for labels within an entity.
///     - Default: `notype`
///
///   * value
///     - Optional: Expression for the value of the symbol.  Usually this is
///     just `.` (the default if it's omitted), but it can be another value.
///     - Default: `.`
///
///
/// This is only really useful when the scope and/or type is set to a
/// non-default value.  `.label name, local` is just `name:`.
.macro .label name, scope=local, type=notype, value:vararg
#ifdef __ELF__
  // Set ELF symbol type.
  .type \name, %\type
#endif

  _.scope \name, \scope

  // Define the label itself.
  .ifb \value
    \name\():
  .else
    \name = \value
  .endif
.endm  // .label

/// Defines an ELF symbol with the same value, size, and type as another.
///
/// Parameters
///
///   * name
///     - Required: Symbol name to define.
///
///   * target
///     - Required: Symbol name to alias.  This need not already be defined.
///     The value, type, and size eventually assigned to this will be copied.
///
///   * scope
///     - Optional: See `.label`; only the default differs here.
///     - Default: `global`
///
.macro .alias name, target, scope=global
  _.scope \name, \scope
  \name = \target
.endm  // .alias

/// Define a function that extends until `.end_function`.
///
/// Parameters
///
///   * name
///     - Required: Symbol name to define.
///
///   * scope
///     - Optional: See `.label`.
///     - Default: `local`
///
///   * cfi
///     - Optional: One of the strings:
///       - `abi`: This is a function with the standard C++ ABI.
///       - `custom`: This function includes `.cfi_*` directives that
///       describe its unwinding requirements completely.
///       - `none`: Don't emit normal function CFI for this function.
///     - Default: `abi`
///
///   * align
///     - Optional: Minimum byte alignment for this function's code.
///     - Default: none
///
///   * nosection
///     - Optional: Must be exactly `nosection` to indicate this function goes
///     into the assembly's current section rather than a per-function section.
///
///   * retain
///     - Optional: `R` for SHF_GNU_RETAIN, empty for not.
///     - Default: ``
///
.macro .function name, scope=local, cfi=abi, align=, nosection=, retain=
  // Validate the \cfi argument.  The valid values correspond to
  // the `_.function.{start,end}.\cfi` subroutine macros.
  .ifnc \cfi, abi
    .ifnc \cfi, custom
      .ifnc \cfi, none
        .error "`cfi` argument `\cfi` not `abi`, `custom`, or `none`"
      .endif
    .endif
  .endif

  _.entity \name, \scope, \align, \nosection, \retain, function, function, _.function.end.\cfi
  _.function.start.\cfi

  // This is a local name, reset inside each `.function`..`.end_function`.
  // It can be used by other macros like .llvm.stack_size, below.
  .L._.function.entry = .
.endm  // .function

/// Define a data object that extends until `.end_object`.
///
/// This starts the definition of a data object and is matched by
/// `.end_object` to finish that object's definition.  `.end_object` must
/// appear before any other `.object` or `.function` directive.
///
/// Parameters
///
///   * name
///     - Required: Symbol name to define.
///
///   * type
///     - Optional: One of the strings:
///       - `bss`: Define a zero-initialized (.bss) writable data object.
///       This is usually followed by just a `.skip` directive and then
///       `.end_object`.
///       - `data`: Define an initialized writable data object.  This is
///       followed by data-emitting directives (`.int` et al) to provide
///       the initializer, and then `.end_object`.
///       - `relro`: Define a read-only initialized data object requiring
///       dynamic relocation.  Use this instead of `rodata` if initializer
///       data includes any absolute address constants.
///       - `rodata`: Define a pure read-only initialized data object.
///     - Default: `data`
///
///   * scope
///     - Optional: See `.label`.
///     - Default: `local`
///
///   * align
///     - Optional: Minimum byte alignment for this function's code.
///     - Default: none
///
///   * nosection
///     - Optional: Must be exactly `nosection` to indicate this object goes
///     into the assembly's current section rather than a per-object section.
///
///   * retain
///     - Optional: `R` for SHF_GNU_RETAIN, empty for not.
///     - Default: ``
///
.macro .object name, type=data, scope=local, align=, nosection=, retain=
  .ifnc \type, bss
    .ifnc \type, data
      .ifnc \type, relro
        .ifnc \type, rodata
          .error "`type` argument `\type` not `bss`, `data, `relro`, or `rodata`"
        .endif
      .endif
    .endif
  .endif
  _.entity \name, \scope, \align, \nosection, \retain, object, \type
.endm  // .start_object

/// Emit `.stack_sizes` metadata compatible with what LLVM compilers emit.
///
/// This uses the format that Clang's -fstack-size-section emits (the Rust
/// compiler can also emit it with the right switches).  This macro can
/// only be used inside a `.function` definition before `.end_function`.
///
/// Parameters
///
///   * frame_size_bytes
///     - Required: An assembly-time constant expression (without relocations).
///     This gives the size in bytes of the machine stack frame.  Consistent
///     with the compiler, this is defined as the maximum net (absolute value)
///     SP adjustment between the current `.function` entry point and any PC
///     inside the function.  Note that on x86, the return address pushed by
///     `call` instructions is not accounted either in a callee (which uses
///     frame_size_bytes=0 if it's a leaf that doesn't push anything) nor in a
///     caller (which doesn't count the word pushed by its own `call`
///     instructions, only the pushes done _before_ any such call).
///
///   * no_metadata
///     - Optional: If given, must be exactly the string `no-metadata`.
///     This makes the macro do nothing at all.  This is a convenience for
///     users like machine-specific .prologue.fp macros to pass through.
///     - Default: none
///
.macro .llvm.stack_size frame_size_bytes:req, no_metadata=
  .ifnb \no_metadata
    .ifnc no-metadata,\no_metadata
      .error "optional `no_metadata=no-metadata` argument syntax must be exact"
    .endif
  .else
#ifdef __ELF__
    // Non-allocated section with SHF_LINK_ORDER and the entry point section
    // as linked-to.  That section link matters to GC behavior when ? doesn't
    // become implied G because a group contains that (current) section.
    .pushsection .stack_sizes, "?o", %progbits, .L._.function.entry
      .dc.a .L._.function.entry
      .uleb128 \frame_size_bytes
    .popsection
#endif
  .endif
.endm

/// Dispatch to the appropriate macro for one-byte or two-byte SLEB128 encoding.
///
/// Results in an error for values out of the valid range.
///
/// Parameters
///
///   * if_1byte
///     - Required: A macro that expects a value in range for one-byte SLEB128.
///
///   * if_2byte
///     - Required: A macro that expects a value in range for two-byte SLEB128.
///
///   * value
///     - Required: The numerical value to encode as SLEB128.
///
///   * args
///     - Vararg: additional arguments for if_1byte and if_2byte.
///
.macro .sleb128.size_dispatch if_1byte:req, if_2byte:req, value:req, args:vararg
  .if SLEB128_IS_1BYTE(\value)
    \if_1byte \value, \args
  .elseif SLEB128_IS_2BYTE(\value)
    \if_2byte \value, \args
  .else
    .error "value out of range for one-byte or two-byte SLEB128: \value"
  .endif
.endm

#else  // clang-format on

#include <cstddef>
#include <cstdint>

namespace arch {

// Use `extern arch::AsmLabel name;` for something defined in assembly code
// with `.label name, global` or the equivalent.  Then use name directly when
// a `const std::byte*` or `const void*` is needed.  This is often used for
// code, but it's used from C++ only as memory to copy around or compare an
// address to, so it's not a function type (even to be never called).
using AsmLabel = const std::byte[];

// Use `arch::kAsmLabelAddress<name>` when an address is needed.
template <AsmLabel& Label>
inline const uintptr_t kAsmLabelAddress = reinterpret_cast<uintptr_t>(Label);

// Use `arch::kAsmLabelSize<start, end>` for the distance between two labels.
template <AsmLabel& Start, AsmLabel& End>
inline const uintptr_t kAsmLabelSize = End - Start;

}  // namespace arch

#endif  // !__ASSEMBLER__

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ASM_H_
