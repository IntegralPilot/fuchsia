
// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdint>
#include <type_traits>

#include <ffl/fixed.h>

namespace {

using ffl::Expression;
using ffl::Fixed;
using ffl::FixedFormat;
using ffl::Operation;
using ffl::Value;

// Tests that ensure the lvalue reference qualification on assignment operators
// correctly prevents assignments to temporaries.

using TestFormat = FixedFormat<int32_t, 16>;
using TestExpression = Expression<Operation::Value, TestFormat>;
using TestType = Fixed<int32_t, 16>;

static_assert(std::is_assignable_v<TestType&, TestType>);
static_assert(!std::is_assignable_v<const TestType&, TestType>);
static_assert(!std::is_assignable_v<TestType, TestType>);

static_assert(std::is_assignable_v<TestType&, Value<TestFormat>>);
static_assert(!std::is_assignable_v<const TestType&, Value<TestFormat>>);
static_assert(!std::is_assignable_v<TestType, Value<TestFormat>>);

static_assert(std::is_assignable_v<TestType&, TestExpression>);
static_assert(!std::is_assignable_v<const TestType&, TestExpression>);
static_assert(!std::is_assignable_v<TestType, TestExpression>);

template <typename T, typename U>
struct IsCompoundAssignable : std::false_type {};

template <typename T, typename U>
  requires requires {
    std::declval<T>() += std::declval<U>();
    std::declval<T>() -= std::declval<U>();
    std::declval<T>() *= std::declval<U>();
    std::declval<T>() /= std::declval<U>();
  }
struct IsCompoundAssignable<T, U> : std::true_type {};

template <typename T, typename U>
constexpr bool IsCompoundAssignableV = IsCompoundAssignable<T, U>::value;

static_assert(IsCompoundAssignableV<TestType&, TestType>);
static_assert(!IsCompoundAssignableV<const TestType&, TestType>);
static_assert(!IsCompoundAssignableV<TestType, TestType>);

static_assert(IsCompoundAssignableV<TestType&, TestExpression>);
static_assert(!IsCompoundAssignableV<const TestType&, TestExpression>);
static_assert(!IsCompoundAssignableV<TestType, TestExpression>);

}  // anonymous namespace
