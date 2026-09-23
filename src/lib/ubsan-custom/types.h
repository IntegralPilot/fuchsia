// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef SRC_LIB_UBSAN_CUSTOM_TYPES_H_
#define SRC_LIB_UBSAN_CUSTOM_TYPES_H_

#include <stdint.h>

// These are the types used in the LLVM UndefinedBehaviorSanitizer runtime
// calls emitted by the compiler.  LLVM provides no documentation on the ABI
// between the compiler and the runtime.  The types here signatures here were
// culled from the LLVM sources for the compiler instrumentation and runtime.
//
// See
// https://github.com/llvm/llvm-project/tree/eb8ebabfb0efcae69e682b592f12366c3b82e78d/compiler-rt/lib/ubsan

namespace ubsan {

/// Situations in which we might emit a check for the suitability of a
/// pointer or glvalue. Needs to be kept in sync with CodeGenFunction.h in
/// clang.
enum TypeCheckKind : uint8_t {
  /// Checking the operand of a load. Must be suitably sized and aligned.
  TCK_Load,
  /// Checking the destination of a store. Must be suitably sized and aligned.
  TCK_Store,
  /// Checking the bound value in a reference binding. Must be suitably sized
  /// and aligned, but is not required to refer to an object (until the
  /// reference is used), per core issue 453.
  TCK_ReferenceBinding,
  /// Checking the object expression in a non-static data member access. Must
  /// be an object within its lifetime.
  TCK_MemberAccess,
  /// Checking the 'this' pointer for a call to a non-static member function.
  /// Must be an object within its lifetime.
  TCK_MemberCall,
  /// Checking the 'this' pointer for a constructor call.
  TCK_ConstructorCall,
  /// Checking the operand of a static_cast to a derived pointer type. Must be
  /// null or an object within its lifetime.
  TCK_DowncastPointer,
  /// Checking the operand of a static_cast to a derived reference type. Must
  /// be an object within its lifetime.
  TCK_DowncastReference,
  /// Checking the operand of a cast to a base object. Must be suitably sized
  /// and aligned.
  TCK_Upcast,
  /// Checking the operand of a cast to a virtual base object. Must be an
  /// object within its lifetime.
  TCK_UpcastToVirtualBase,
  /// Checking the value assigned to a _Nonnull pointer. Must not be null.
  TCK_NonnullAssign,
  /// Checking the operand of a dynamic_cast or a typeid expression.  Must be
  /// null or an object within its lifetime.
  TCK_DynamicOperation
};

constexpr const char* TypeCheckKindMsg(TypeCheckKind kind) {
  switch (kind) {
    case TCK_Load:
      return "load of";
    case TCK_Store:
      return "store to";
    case TCK_ReferenceBinding:
      return "reference binding to";
    case TCK_MemberAccess:
      return "member access within";
    case TCK_MemberCall:
      return "member call on";
    case TCK_ConstructorCall:
      return "constructor call on";
    case TCK_DowncastPointer:
    case TCK_DowncastReference:
      return "downcast of";
    case TCK_Upcast:
      return "upcast of";
    case TCK_UpcastToVirtualBase:
      return "cast to virtual base of";
    case TCK_NonnullAssign:
      return "_Nonnull binding to";
    case TCK_DynamicOperation:
      return "dynamic operation on";
    default:
      return "UBSAN BUG! Invalid TypeCheckKind";
  }
}

struct SourceLocation {
  const char* filename;
  uint32_t line;
  uint32_t column;
};

struct TypeDescriptor {
  uint16_t TypeKind;
  uint16_t TypeInfo;
  char TypeName[];
};

struct NonNullArgData {
  SourceLocation Loc;
  SourceLocation AttrLoc;
  int32_t ArgIndex;
};

struct TypeMismatchData {
  SourceLocation Loc;
  const TypeDescriptor& Type;
  uint8_t LogAlignment;
  TypeCheckKind TypeCheckKind;
};

struct FunctionTypeMismatchData {
  SourceLocation Loc;
  const TypeDescriptor& Type;
};

struct UnreachableData {
  SourceLocation Loc;
};

struct VLABoundData {
  SourceLocation Loc;
  const TypeDescriptor& Type;
};

struct AlignmentAssumptionData {
  SourceLocation Loc;
  SourceLocation AssumptionLoc;
  const TypeDescriptor& Type;
};

using ValueHandle = uintptr_t;

struct OverflowData {
  SourceLocation Loc;
  const TypeDescriptor& Type;
};

struct ReportOptions {
  // If FromUnrecoverableHandler is specified, UBSan runtime handler is not
  // expected to return.
  bool FromUnrecoverableHandler;
  /// pc/bp are used to unwind the stack trace.
  uintptr_t pc;
  uintptr_t bp;
};

struct InvalidValueData {
  SourceLocation Loc;
  const TypeDescriptor& Type;
};

// Known implicit conversion check kinds.
enum ImplicitConversionCheckKind : uint8_t {
  ICCK_IntegerTruncation = 0,  // Legacy, was only used by clang 7.
  ICCK_UnsignedIntegerTruncation = 1,
  ICCK_SignedIntegerTruncation = 2,
  ICCK_IntegerSignChange = 3,
  ICCK_SignedIntegerTruncationOrSignChange = 4,
};

struct ImplicitConversionData {
  SourceLocation Loc;
  const TypeDescriptor& FromType;
  const TypeDescriptor& ToType;
  ImplicitConversionCheckKind Kind;
};

struct OutOfBoundsData {
  SourceLocation Loc;
  const TypeDescriptor& ArrayType;
  const TypeDescriptor& IndexType;
};

struct ShiftOutOfBoundsData {
  SourceLocation Loc;
  const TypeDescriptor& LHSType;
  const TypeDescriptor& RHSType;
};

struct PointerOverflowData {
  SourceLocation Loc;
};

enum BuiltinCheckKind : uint8_t {
  BCK_CTZPassedZero,
  BCK_CLZPassedZero,
};

struct InvalidBuiltinData {
  SourceLocation Loc;
  BuiltinCheckKind Kind;
};

struct NonNullReturnData {
  SourceLocation AttrLoc;
};

}  // namespace ubsan

#endif  // SRC_LIB_UBSAN_CUSTOM_TYPES_H_
