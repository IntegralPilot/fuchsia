// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/errors.h"

#include <lib/fidl/cpp/wire/status.h>

namespace forensics {

Error FidlErrorToForensicsError(const ::fidl::Error& error) {
  const zx_status_t status = error.status();

  if (status == ZX_ERR_NOT_FOUND) {
    return Error::kNotAvailableInProduct;
  }
  if (status == ZX_ERR_TIMED_OUT) {
    return Error::kTimeout;
  }

  return Error::kConnectionError;
}

}  // namespace forensics
