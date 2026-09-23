// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// This is doing the equivalent of:
// ```
// [[noreturn]] void _start(...) { __libc_start_main(..., &main); }
// ```
// Currently the `...` is three arguments: the zx_handle_t and vDSO address
// passed to zx_process_start, plus another zx_handle_t possibly added by
// the startup dynamic linker.
//
// That is, _start just calls __libc_start_main with the arguments it got, plus
// an additional argument that's the address of the main function.  (Note we
// want to ensure it's not a tail call so that there will be an outermost
// backtrace frame showing _start as the caller of __libc_start_main.)
//
// Three cases need to be considered:
//  * static PIE: _start from Scrt1.o, __libc_start_main from libc.a, and main
//    from the user's program (perhaps in a static library) are all statically
//    linked together into the executable itself.
//  * dynamic linking, static main: _start from Scrt1.o and main from the
//    user's program (perhaps via some other static library) are statically
//    linked into the executable itself and have hidden visibility;
//    __libc_start_main is in libc.so.
//  * dynamically linked main: _start from Scrt1.o is statically linked into
//    the executable itself; __libc_start_main is in libc.so; main is in some
//    other shared library specified at static link time.
//
// Most often main will be defined in the executable itself at static link
// time, so a direct PC-relative reference would work.  That's what the
// compiler would generate if main were declared with STV_HIDDEN visibility.
//
// If main is found in a shared library at static link time then it will
// require a runtime dynamic relocation.  When using a dynamic linker, dynamic
// relocation has already been done by now, so a normal GOT relocation here
// could load the address from main's GOT slot.  Likewise, that's what the
// compiler would normally generate if main weren't declared as STV_HIDDEN.
//
// In either case the __libc_start_main function will usually be found in the
// shared libc.so, so it needs to be called via PLT or GOT.  The compiler will
// normally use the `@PLT` form to request the PLT-call relocation, which tells
// the linker to generate a PLT entry as needed.  The PLT entry really just
// does a load from the GOT and a jump, so it's a bit superfluous in the usual
// case and adds a second jump.  The compiler under `-fno-plt` would instead
// generate a GOT load and indirect call here, in effect inlining that PLT
// entry.  A compiler doing LTO where it knows the call is resolved into a
// shared library might very well also elide the PLT and use the direct GOT
// access, an optimization that is a very good one in the general case.
//
// However, this one Scrt1.o is meant to support both the dynamic linking case
// and the static PIE case.  In the static PIE case, this is the real entry
// point directly from the program loader and the GOT (and other initialized
// data) is still in its link-time state.  The self-relocation work will be
// done by __libc_start_main as linked in from the static libc.a; before that
// the GOT cannot be used.  With a direct PC-relative reference to main here,
// that would be fine--but that's not compatible with the case of main in a
// shared library.  The relocation will always have been done by the time
// __libc_start_main actually needs to call main, it just won't always have
// been done by the time _start needs to pass the main function pointer to
// __libc_start_main.  So what's needed is a different, fixed jump target
// (i.e. function pointer) that will, when called, tail call the main function
// as resolved by that dynamic relocation--just like a PLT entry!  But, in PIC
// and PIE modes the compiler won't try to use a PLT entry to materialize a
// function pointer--only for an actual direct call.  The compiler will instead
// just use a GOT load, in essence doing the first half of the PLT's work
// without doing the second half of making the function call.
//
// This can be solved in C++ source by passing the address of an internal
// linkage wrapper function that calls main.  Since that function has internal
// linkage, the compiler's reference to it will use a direct PC-relative
// relocation to pass the function pointer to __libc_start_main.  Then the need
// for PLT or GOT resolution of main is deferred until that function is called.
// However, there is no way to force the compiler to guarantee a total tail
// call, i.e. a simple jump to main, such that a backtrace will never show this
// local function as the caller of main instead of __libc_start_main directly.
//
// The simplest solution all around is to just write _start entirely in
// assembly where it's possible to precisely control what kinds of relocations
// are used for both main and __libc_start_main.

.function _start, global
  .prologue.fp

  // Three argument registers are accepted by _start and passed through
  // to __libc_start_main, with &main (or proxy for it) in the fourth.

#if defined(__aarch64__)

  // There are only two kinds of relocations that can generate a PLT entry:
  //  1. Jumps / calls, only usable in a b or bl instruction.
  //  2. 32-bit data (PLT32), only usable in a flat 32-bit data field.
  // There is no way to use `main@PLT` in an `adrp` + `add` pair so as to
  // directly materialize the address of the PLT entry here.  Doing an `adrp`
  // sequence here for a PC-relative load of an RODATA word with a PLT32
  // relocation would require materializing the address of that word in one
  // register, loading it into another, and then adding the two together.  So
  // instead, we just materialize the address of a later trampoline that does a
  // pure tail call.  Since that trampoline is right below, a single `adr` is
  // always sufficient (whereas `adrp` + add would always be required for
  // either a PLT entry or an RODATA word).
  adr x3, 0f
  bl __libc_start_main

  // __libc_start_main is declared [[noreturn]] and cannot return.  This
  // appears as the return address for backtrace purposes, but will never be
  // reached.  Just in case, an explicit trap here makes that extra clear.
  udf #0

  // __libc_start_main bounces here to call main.  In the most common case,
  // main will be defined in the executable and this will be resolved to a
  // direct jump at static link time, so there is one extra jump but no load.
  // When this actually jumps to a PLT entry, that will do a GOT load and jump
  // again, so there will have been two extra jumps and a load.  This could
  // instead do a GOT load and jump here, for just one extra jump and one extra
  // load, but in the more common case that would instead do the same number of
  // jumps at the cost of an extra load of an extra relocated GOT slot.
0:b main

#elif defined(__riscv)

  // The relocation issues on RISC-V are all the same as just described for
  // AAarch64, but the PC-relative sequences are even more instructions though
  // hidden behind assembler pseudo-instructions.  The `lla` here will produce
  // multiple instructions for the general case of the distance to the 0: label
  // but it should be relaxed down at link time to only two.
  lla a3, 0f
  call __libc_start_main
  unimp
0:tail main

#elif defined(__x86_64__)

  // On x86-64, the PLT32 relocation type works with many instructions because
  // they just use flat 32-bit displacements and immediates anyway.  The other
  // alternative here would be to do a load from `main@GOTPCREL(%rip)`: that
  // requests a GOT slot load, which as described earlier can't work right in
  // the static PIE case.  But on x86-64 with modern assemblers and linkers,
  // this is the GOTPCRELX relocation type, which is guaranteed to be relaxed
  // at link time to a direct PC-relative `lea` instruction when main is
  // defined in the executable at static link time.  That would not support a
  // static PIE that does its own dynamic linking to resolve main in a shared
  // library, which would work on the other machines using PLT calls.  It
  // doesn't really need to be really supported to link in the static libc.a
  // and then do dynamic linking against any shared libraries except the vDSO.
  // But in the common case both relax into this same `lea` instruction anyway,
  // and in the case where main is in a shared library but dynamic linking was
  // already done an extra bounce through the PLT doesn't hurt much (less than
  // the double bounce on the other machines where PLT32 is harder to use).
  lea main@PLT(%rip), %rcx
  call __libc_start_main@PLT
  ud2

#else
#error "unsupported machine"
#endif

  // The end of the function is never reached, so .epilogue.fp is elided.
.end_function
