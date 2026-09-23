// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_BACKOFF_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_BACKOFF_H_

#include "src/lib/backoff/backoff.h"

namespace forensics {

class MonotonicBackoff : public backoff::Backoff {
 public:
  zx::duration GetNext() override {
    const zx::duration backoff = backoff_;
    backoff_ = backoff + zx::sec(1);
    return backoff;
  }
  void Reset() override { backoff_ = zx::sec(1); }

 private:
  zx::duration backoff_{zx::sec(1)};
};

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_BACKOFF_H_
