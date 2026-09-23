// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYS_FUZZING_COMMON_TESTING_ARTIFACT_H_
#define SRC_SYS_FUZZING_COMMON_TESTING_ARTIFACT_H_

#include <fuchsia/fuzzer/cpp/fidl.h>

#include "src/sys/fuzzing/common/artifact.h"
#include "src/sys/fuzzing/common/async-types.h"

namespace fuzzing {

using fuchsia::fuzzer::ControllerPtr;

// Helper function to get an optional FIDL artifact from the given |controller| and convert it to an
// optional |Artifact| while preserving errors.
ZxPromise<Artifact> WatchArtifact(const ExecutorPtr& executor, ControllerPtr& controller);

}  // namespace fuzzing

#endif  // SRC_SYS_FUZZING_COMMON_TESTING_ARTIFACT_H_
