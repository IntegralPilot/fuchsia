// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_COMMON_ARTIFACT_H_
#define SRC_SYS_FUZZING_COMMON_ARTIFACT_H_

#include <fuchsia/fuzzer/cpp/fidl.h>

#include <memory>

#include "src/sys/fuzzing/common/input.h"
#include "src/sys/fuzzing/common/result.h"

namespace fuzzing {

using FidlArtifact = fuchsia::fuzzer::Artifact;

// An |Artifact| is a |FuzzResult| and the associated |Input| that caused it.
class Artifact final {
 public:
  Artifact() = default;
  explicit Artifact(FuzzResult fuzz_result);
  Artifact(FuzzResult fuzz_result, Input input);
  Artifact(Artifact&& other) noexcept;
  ~Artifact() = default;

  Artifact& operator=(Artifact&& other) noexcept;
  bool operator==(const Artifact& other) const;
  bool operator!=(const Artifact& other) const;

  bool is_empty() const { return empty_; }
  FuzzResult fuzz_result() const { return fuzz_result_; }
  bool has_input() const { return !!input_; }

  // These will panic if the object does not have an input.
  const Input& input() const;
  Input take_input();

  Artifact Duplicate() const;

 private:
  bool empty_ = true;
  FuzzResult fuzz_result_ = FuzzResult::NO_ERRORS;
  std::unique_ptr<Input> input_;

  FXL_DISALLOW_COPY_AND_ASSIGN(Artifact);
};

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_COMMON_ARTIFACT_H_
