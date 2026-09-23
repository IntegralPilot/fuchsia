// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_POWER_STATE_RECORDER_CPP_CONCEPTS_H_
#define LIB_POWER_STATE_RECORDER_CPP_CONCEPTS_H_

#include <type_traits>

namespace power_observability {

// The concepts below, combined with a few natural types, specify the numeric types that can be
// used with NumericStateRecorder and how they are recorded to trace and Inspect.
//
// | Concept        | Trace type | Inspect type |
// |----------------|------------|--------------|
// | WidensToUint32 | uint32_t   | uint64_t     |
// | uint64_t       | uint64_t   | uint64_t     |
// | WidensToInt32  | int32_t    | int64_t      |
// | int64_t        | int64_t    | int64_t      |
// | WidensToDouble | double     | double       |
template <typename T>
concept WidensToUint32 = std::is_same_v<T, bool> || std::is_same_v<T, uint8_t> ||
                         std::is_same_v<T, uint16_t> || std::is_same_v<T, uint32_t>;

template <typename T>
concept WidensToUint64 = WidensToUint32<T> || std::is_same_v<T, uint64_t>;

template <typename T>
concept WidensToInt32 =
    std::is_same_v<T, int8_t> || std::is_same_v<T, int16_t> || std::is_same_v<T, int32_t>;

template <typename T>
concept WidensToInt64 = WidensToInt32<T> || std::is_same_v<T, int64_t>;

template <typename T>
concept WidensToDouble = std::is_same_v<T, float> || std::is_same_v<T, double>;

template <typename T>
concept IsRecordableNumericType = WidensToUint64<T> || WidensToInt64<T> || WidensToDouble<T>;

// An enum type is recordable if it can be mapped in an obvious way to Inspect.
template <typename T>
concept IsRecordableEnumType = std::is_enum_v<T> && (WidensToUint64<std::underlying_type_t<T>> ||
                                                     WidensToInt64<std::underlying_type_t<T>>);

template <typename T>
concept IsRecordableValueType = IsRecordableNumericType<T> || IsRecordableEnumType<T>;

}  // namespace power_observability

#endif  // LIB_POWER_STATE_RECORDER_CPP_CONCEPTS_H_
