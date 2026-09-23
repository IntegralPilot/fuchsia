// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/artifact.h"

#include <lib/syslog/cpp/macros.h>

namespace fuzzing {

Artifact::Artifact(FuzzResult fuzz_result) : empty_(false), fuzz_result_(fuzz_result) {}

Artifact::Artifact(FuzzResult fuzz_result, Input input) : empty_(false), fuzz_result_(fuzz_result) {
  input_ = std::make_unique<Input>(std::move(input));
}

Artifact::Artifact(Artifact&& other) noexcept { *this = std::move(other); }

Artifact& Artifact::operator=(Artifact&& other) noexcept {
  empty_ = other.empty_;
  other.empty_ = true;
  fuzz_result_ = other.fuzz_result_;
  other.fuzz_result_ = FuzzResult::NO_ERRORS;
  input_ = std::move(other.input_);
  return *this;
}

bool Artifact::operator==(const Artifact& other) const {
  if (empty_ && other.empty_) {
    return true;
  }
  if (empty_ || other.empty_) {
    return false;
  }
  if (fuzz_result_ != other.fuzz_result_) {
    return false;
  }
  if (!input_ && !other.input_) {
    return true;
  }
  if (!input_ || !other.input_) {
    return false;
  }
  return *input_ == *other.input_;
}

bool Artifact::operator!=(const Artifact& other) const { return !(*this == other); }

const Input& Artifact::input() const {
  FX_CHECK(!empty_);
  return *input_;
}

Input Artifact::take_input() {
  FX_CHECK(!empty_);
  fuzz_result_ = FuzzResult::NO_ERRORS;
  auto input = std::move(*input_);
  input_.reset();
  empty_ = true;
  return input;
}

Artifact Artifact::Duplicate() const {
  FX_CHECK(!empty_);
  Artifact duplicate(fuzz_result_);
  if (input_) {
    duplicate.input_ = std::make_unique<Input>(input_->Duplicate());
  }
  return duplicate;
}

}  // namespace fuzzing
