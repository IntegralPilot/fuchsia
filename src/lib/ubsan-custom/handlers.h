// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef SRC_LIB_UBSAN_CUSTOM_HANDLERS_H_
#define SRC_LIB_UBSAN_CUSTOM_HANDLERS_H_

#include <inttypes.h>
#include <stdint.h>

#include "report.h"
#include "types.h"

// This implements a minimal ubsan runtime.  This header should be included by
// one and only one translation unit (source file).  The including code must
// also define the interfaces described in "ubsan-report.h".  That file should
// not be included directly but only included via this file and only in the one
// translation unit that defines those interfaces.
//
// The rest of this file is implementation detail.

namespace [[gnu::visibility("hidden")]] ubsan {

// LLVM provides no documentation on the ABI between the compiler and
// the runtime.  The set of function signatures here was culled from
// the LLVM sources for the compiler instrumentation and the runtime.
//
// See
// https://github.com/llvm/llvm-project/tree/eb8ebabfb0efcae69e682b592f12366c3b82e78d/compiler-rt/lib/ubsan

inline void PrintTypeDescriptor(const TypeDescriptor& type, const char* prefix = nullptr) {
  // TODO(https://fxbug.dev/42056251): Improve logging by interpreting TypeDescriptor values.
  Printf("%s%sType Kind (0x%04hx)\tInfo (0x%04hx)\tName %s\n", prefix ? prefix : "",
         prefix ? ":" : "", type.TypeKind, type.TypeInfo, type.TypeName);
}

// UBSAN_FMT_PRIxPTR is a string literal fragment for a printf format that goes
// with an intptr_t or uintptr_t argument.  It prints "0x..." with a fixed
// number of digits for any value, always 8 for ILP32 and always 16 for LP64.
#if __INTPTR_WIDTH__ == 32
#define UBSAN_FMT_PRIxPTR "0x%08" PRIxPTR
#elif __INTPTR_WIDTH__ == 64
#define UBSAN_FMT_PRIxPTR "0x%016" PRIxPTR
#else
#error "__INTPTR_WIDTH__ not predefined?"
#endif

// This is a helper used below.  It's always inlined so that Report is
// constructed in the frame of the actual handler entrypoint and records as its
// caller PC the call site in the failing instrumented code.
[[gnu::always_inline]] inline void HandleNonnullReturn(  //
    NonNullReturnData& data, SourceLocation& loc, const char* annotation) {
  Report failure{
      "null pointer returned from function declared to never return null",
      loc,
  };
  if (data.AttrLoc.column == 0) {
    Printf("%s:%u: %s specified here\n", data.AttrLoc.filename, data.AttrLoc.line, annotation);
  } else {
    Printf("%s:%u:%u: %s specified here\n", data.AttrLoc.filename, data.AttrLoc.line,
           data.AttrLoc.column, annotation);
  }
}

// All the runtime handlers have unmangled C-style external linkage names.
extern "C" {

// Declare each function `inline` so that it has "vague linkage", i.e. COMDAT
// semantics; that's really just to be entirely proper for a header file in
// case this were included in more than one TU though it ought not be.  Then
// use `[[gnu::noinline]]` to make sure that LTO doesn't try to inline these to
// their call sites, primarily so that the `__builtin_return_address(0)` in the
// default argument to each Report constructor will be a useful call site
// address for each instance.  Since they have vague linkage, the compiler's
// front end usually won't emit them at all in a TU where it doesn't see any
// references.  So also use `[[gnu::used]]` to tell the compiler that each
// function is used by some kind of linkage it can't understand.  When the
// later instrumentation passes add in the calls to the runtime handlers, their
// linkage will resolve happily.  If an individual function is never actually
// called by any generated code, then --gc-sections will drop its COMDAT group.
//
// Vague linkage (via ELF COMDAT) also means that these definitions will
// implicitly have weak linkage, so if there is an alternative ubsan runtime
// implementation linked in (e.g. from the compiler's prebuilt libraries meant
// for the default build environment), its functions will take precedence.
#define UBSAN_HANDLER [[gnu::used, gnu::noinline]] inline void

UBSAN_HANDLER __ubsan_handle_nonnull_arg(NonNullArgData* Data) {
  Report failure("NULL ARG passed to NonNullarg parameter, specified by nonnull attribute.",
                 Data->Loc);
}

UBSAN_HANDLER __ubsan_handle_nullability_arg(NonNullArgData* Data) {
  Report failure("NULL ARG passed to NonNullarg parameter, specified by _Nonnull type annotation.",
                 Data->Loc);
}

UBSAN_HANDLER __ubsan_handle_type_mismatch_v1(TypeMismatchData* Data, ValueHandle Pointer) {
  Report failure("Type Mismatch", Data->Loc);

  const uintptr_t Alignment = uintptr_t{1} << Data->LogAlignment;
  const uintptr_t AlignmentMask = Alignment - 1;

  Printf("Pointer: " UBSAN_FMT_PRIxPTR "\n", Pointer);
  Printf("Alignment: 0x%" PRIxPTR " bytes\n", Alignment);

  if (Pointer & AlignmentMask) {
    Printf("%s misaligned address " UBSAN_FMT_PRIxPTR "\n", TypeCheckKindMsg(Data->TypeCheckKind),
           Pointer);
  } else {
    Printf("TypeCheck Kind: %s (0x%hhx)\n", TypeCheckKindMsg(Data->TypeCheckKind),
           Data->TypeCheckKind);
  }

  PrintTypeDescriptor(Data->Type);
}

UBSAN_HANDLER __ubsan_handle_function_type_mismatch(FunctionTypeMismatchData* Data,
                                                    ValueHandle Val) {
  Report failure("Function Type Mismatch", Data->Loc);

  PrintTypeDescriptor(Data->Type);
}

#define UBSAN_OVERFLOW_HANDLER(handler_name, op)                                     \
  UBSAN_HANDLER handler_name(OverflowData* Data, ValueHandle LHS, ValueHandle RHS) { \
    Report failure("Integer " op " overflow\n", Data->Loc);                          \
    Printf("LHS: " UBSAN_FMT_PRIxPTR "\n", LHS);                                     \
    Printf("RHS: " UBSAN_FMT_PRIxPTR "\n", RHS);                                     \
    PrintTypeDescriptor(Data->Type);                                                 \
  }

UBSAN_OVERFLOW_HANDLER(__ubsan_handle_add_overflow, "ADD")
UBSAN_OVERFLOW_HANDLER(__ubsan_handle_mul_overflow, "MUL")
UBSAN_OVERFLOW_HANDLER(__ubsan_handle_sub_overflow, "SUB")

#undef UBSAN_OVERFLOW_HANDLER

UBSAN_HANDLER __ubsan_handle_divrem_overflow(OverflowData* Data, ValueHandle LHS, ValueHandle RHS,
                                             ReportOptions Opts) {
  Report failure("Integer DIVREM overflow", Data->Loc);
  Printf("LHS: " UBSAN_FMT_PRIxPTR "\n", LHS);
  Printf("RHS: " UBSAN_FMT_PRIxPTR "\n", RHS);
  PrintTypeDescriptor(Data->Type);
}

UBSAN_HANDLER __ubsan_handle_negate_overflow(OverflowData* Data, ValueHandle OldVal) {
  Report failure("Integer NEGATE overflow", Data->Loc);
  Printf("old value: " UBSAN_FMT_PRIxPTR "\n", OldVal);
  PrintTypeDescriptor(Data->Type);
}

UBSAN_HANDLER __ubsan_handle_load_invalid_value(InvalidValueData* Data, ValueHandle Val) {
  Report failure("Load invalid value into enum/bool", Data->Loc);
  Printf("Val: " UBSAN_FMT_PRIxPTR "\n", Val);
  PrintTypeDescriptor(Data->Type);
}

UBSAN_HANDLER __ubsan_handle_implicit_conversion(ImplicitConversionData* Data, ValueHandle Src,
                                                 ValueHandle Dst) {
  Report failure("Implicit Conversion", Data->Loc);
  Printf("Src: " UBSAN_FMT_PRIxPTR "\n", Src);
  Printf("Dst: " UBSAN_FMT_PRIxPTR "\n", Dst);
  PrintTypeDescriptor(Data->FromType, "From");
  PrintTypeDescriptor(Data->ToType, "To");
}

UBSAN_HANDLER __ubsan_handle_out_of_bounds(OutOfBoundsData* Data, ValueHandle Index) {
  Report failure("Out of bounds access", Data->Loc);
  Printf("Index: " UBSAN_FMT_PRIxPTR "\n", Index);
  PrintTypeDescriptor(Data->ArrayType, "Array");
  PrintTypeDescriptor(Data->IndexType, "Index");
}

UBSAN_HANDLER __ubsan_handle_shift_out_of_bounds(ShiftOutOfBoundsData* Data, ValueHandle LHS,
                                                 ValueHandle RHS) {
  Report failure("SHIFT overflow", Data->Loc);
  Printf("LHS: " UBSAN_FMT_PRIxPTR "\n", LHS);
  Printf("RHS: " UBSAN_FMT_PRIxPTR "\n", RHS);
  PrintTypeDescriptor(Data->LHSType, "LHS");
  PrintTypeDescriptor(Data->RHSType, "RHS");
}

UBSAN_HANDLER __ubsan_handle_pointer_overflow(PointerOverflowData* Data, ValueHandle Base,
                                              ValueHandle Result) {
  Report failure("POINTER overflow", Data->Loc);
  Printf("Base: " UBSAN_FMT_PRIxPTR "\n", Base);
  Printf("Result: " UBSAN_FMT_PRIxPTR "\n", Result);
}

UBSAN_HANDLER __ubsan_handle_builtin_unreachable(UnreachableData* Data) {
  Report failure("Executed unreachable code", Data->Loc);
}

UBSAN_HANDLER __ubsan_handle_alignment_assumption(AlignmentAssumptionData* Data,
                                                  ValueHandle Pointer, ValueHandle Alignment,
                                                  ValueHandle Offset) {
  Report failure("Alignment Assumption violation", Data->Loc);
  PrintTypeDescriptor(Data->Type);
  Printf("Pointer: " UBSAN_FMT_PRIxPTR "\n", Pointer);
  Printf("Alignment: " UBSAN_FMT_PRIxPTR "\n", Alignment);
  Printf("Offset: " UBSAN_FMT_PRIxPTR "\n", Offset);
}

UBSAN_HANDLER __ubsan_handle_invalid_builtin(InvalidBuiltinData& Data) {
  Report failure("invalid builtin use", Data.Loc);
  Printf("passing zero to %s, which is not a valid argument\n",
         Data.Kind == BCK_CTZPassedZero ? "ctz()" : "clz()");
}

UBSAN_HANDLER __ubsan_handle_nonnull_return_v1(NonNullReturnData& Data, SourceLocation& Loc) {
  HandleNonnullReturn(Data, Loc, "returns_nonnull attribute");
}

UBSAN_HANDLER __ubsan_handle_nullability_return_v1(NonNullReturnData& Data, SourceLocation& Loc) {
  HandleNonnullReturn(Data, Loc, "_Nonnull return type annotation");
}

UBSAN_HANDLER __ubsan_handle_missing_return(UnreachableData* Data) {
  Report failure(
      "execution reached the end of a value-returning function "
      "without returning a value",
      Data->Loc);
}

UBSAN_HANDLER
__ubsan_handle_vla_bound_not_positive(VLABoundData* Data, ValueHandle Bound) {
  Report failure("variable length array bound evaluates to non-positive value", Data->Loc);
  Printf("Computed bound: " UBSAN_FMT_PRIxPTR "\n", Bound);
  PrintTypeDescriptor(Data->Type);
}

// TODO(https://fxbug.dev/42056251): Add missing handlers:
// * cfi_check_fail
// * cfi_bad_type

// NOTE: The following functions should never be generated in the kernel ubsan:
//  * float_cast_overflow

#undef UBSAN_HANDLER

}  // extern "C"
}  // namespace ubsan

#endif  // SRC_LIB_UBSAN_CUSTOM_HANDLERS_H_
