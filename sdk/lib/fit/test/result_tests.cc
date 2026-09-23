// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/result.h>

#include <gtest/gtest.h>

namespace {

struct TrivialObject {
  TrivialObject() = default;
  ~TrivialObject() = default;
  TrivialObject(const TrivialObject&) = delete;
  TrivialObject(TrivialObject&&) = delete;
  TrivialObject& operator=(const TrivialObject&) = delete;
  TrivialObject& operator=(TrivialObject&&) = delete;
};
static_assert(fit::internal::storage_class_trait<TrivialObject> ==
              fit::internal::storage_class_e::trivial);

struct NonTrivialObject {
  NonTrivialObject() = default;
  ~NonTrivialObject() {
    // Non-trivial destructor
  }
  NonTrivialObject(const NonTrivialObject&) = delete;
  NonTrivialObject(NonTrivialObject&&) = delete;
  NonTrivialObject& operator=(const NonTrivialObject&) = delete;
  NonTrivialObject& operator=(NonTrivialObject&&) = delete;
};
static_assert(fit::internal::storage_class_trait<NonTrivialObject> ==
              fit::internal::storage_class_e::non_trivial);

struct ErrorType {};

template <typename T>
fit::result<ErrorType, T> make_object() {
  return fit::result<ErrorType, T>{std::in_place, fit::success{}};
}

}  // namespace

TEST(ResultTest, construct_trivial_success_in_place) {
  [[maybe_unused]] auto discard = make_object<TrivialObject>();
}

TEST(ResultTest, construct_nontrivial_success_in_place) {
  [[maybe_unused]] auto discard = make_object<NonTrivialObject>();
}
