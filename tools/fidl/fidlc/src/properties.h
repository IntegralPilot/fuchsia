// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_PROPERTIES_H_
#define TOOLS_FIDL_FIDLC_SRC_PROPERTIES_H_

#include <cstdint>
#include <variant>

namespace fidlc {

// TODO(https://fxbug.dev/42128146): zircon/types.h's zx_rights_t needs to be kept in sync with
// this. Eventually, zx_rights_t will be generated from a fidl
// declaration. This is currently tested by fidl-compiler's
// TypesTests' rights test.
using RightsWrappedType = uint32_t;

enum class Nullability : uint8_t {
  kNullable,
  kNonnullable,
};

enum class Strictness : uint8_t {
  kFlexible,
  kStrict,
};

enum class Resourceness : uint8_t {
  kValue,
  kResource,
};

enum class Openness : uint8_t {
  kClosed,
  kAjar,
  kOpen,
};

using ModifierValue = std::variant<Strictness, Resourceness, Openness>;

// TODO(https://fxbug.dev/42128146): zircon/types.h's zx_obj_type_t and related values must be
// kept in sync with this. Eventually, they will be generated from
// fidl declarations. This is currently tested by fidl-compiler's
// TypesTests's handle_subtype test.
// TODO(https://fxbug.dev/42143256): Remove this enumeration once handle generalization is
// fully implemented. The enum `obj_type` defined in the FIDL library zx will
// become the only source of truth.
// NOLINTNEXTLINE(performance-enum-size): The size matches zx_obj_type_t.
enum class HandleSubtype : uint32_t {
  // special case to indicate subtype is not specified.
  kHandle = 0,

  kBti = 24,
  kChannel = 4,
  kClock = 30,
  kCounter = 34,
  kDebugLog = 12,
  kEvent = 5,
  kEventpair = 16,
  kException = 29,
  kFifo = 19,
  kGuest = 20,
  kInterrupt = 9,
  kIommu = 23,
  kIob = 33,
  kJob = 17,
  kMsi = 32,
  kPager = 28,
  kPciDevice = 11,
  kPmt = 26,
  kPort = 6,
  kProcess = 1,
  kProfile = 25,
  kResource = 15,
  kSocket = 14,
  kStream = 31,
  kSuspendToken = 27,
  kThread = 2,
  kTimer = 22,
  kVcpu = 21,
  kVmar = 18,
  kVmo = 3,
};

enum class PrimitiveSubtype : uint8_t {
  kBool,
  kInt8,
  kInt16,
  kInt32,
  kInt64,
  kUint8,
  kZxUchar,
  kUint16,
  kUint32,
  kUint64,
  kZxUsize64,
  kZxUintptr64,
  kFloat32,
  kFloat64,
};

enum class InternalSubtype : uint8_t {
  kFrameworkErr,
};

enum class MessageKind : uint8_t {
  kRequest,
  kResponse,
  kEvent,
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_PROPERTIES_H_
