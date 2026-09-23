// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_COBALT_BIN_APP_UTILS_H_
#define SRC_COBALT_BIN_APP_UTILS_H_

#include <fuchsia/buildinfo/cpp/fidl.h>
#include <fuchsia/metrics/cpp/fidl.h>
#include <lib/fpromise/result.h>
#include <stdlib.h>

#include <string>

#include "third_party/cobalt/src/public/lib/status.h"

namespace cobalt {

fpromise::result<void, fuchsia::metrics::Error> ToMetricsResult(const Status &s);

// Reads the PEM file at the specified path and returns the contents as
// a string. CHECK fails if the file cannot be read.
std::string ReadPublicKeyPem(const std::string &pem_file_path);

// Returns the concatenation of the product and platform versions using a double dash, i.e.
// product_version--platform_version. If the two versions are identical, no concatenation will
// occur.
//
// Note: Returns <version not specified> if either the product or platform version is unavailable.
std::string GetSystemVersion(const fuchsia::buildinfo::BuildInfo &build_info);

}  // namespace cobalt

#endif  // SRC_COBALT_BIN_APP_UTILS_H_
