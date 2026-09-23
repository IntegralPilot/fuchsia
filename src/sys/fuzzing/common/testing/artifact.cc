// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/testing/artifact.h"

#include <lib/syslog/cpp/macros.h>

#include "src/sys/fuzzing/common/async-socket.h"

namespace fuzzing {

ZxPromise<Artifact> WatchArtifact(const ExecutorPtr& executor, ControllerPtr& controller) {
  Bridge<FidlArtifact> bridge;
  controller->WatchArtifact(bridge.completer.bind());
  return ConsumeBridge(bridge).and_then(
      [executor, fuzz_result = FuzzResult::NO_ERRORS, read = ZxFuture<Input>()](
          Context& context, FidlArtifact& fidl_artifact) mutable -> ZxResult<Artifact> {
        if (!read) {
          if (fidl_artifact.IsEmpty()) {
            return fpromise::ok(Artifact());
          }
          if (fidl_artifact.has_error()) {
            return fpromise::error(fidl_artifact.error());
          }
          FX_DCHECK(fidl_artifact.has_result());
          fuzz_result = fidl_artifact.result();
          if (!fidl_artifact.has_input()) {
            return fpromise::ok(Artifact(fuzz_result));
          }
          auto* fidl_input = fidl_artifact.mutable_input();
          read = AsyncSocketRead(executor, std::move(*fidl_input));
        }
        if (!read(context)) {
          return fpromise::pending();
        }
        if (read.is_error()) {
          return fpromise::error(ZX_ERR_CANCELED);
        }
        return fpromise::ok(Artifact(fuzz_result, read.take_value()));
      });
}

}  // namespace fuzzing
