// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_LOAD_VISITORS_HOST_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_LOAD_VISITORS_HOST_H_

#include <lib/driver/devicetree/visitors/registry.h>

#include <string>
#include <vector>

namespace fdf_devicetree {

// Host-side helper to load devicetree visitors from shared library files.
// |library_paths| is a list of paths to .so files containing visitors.
zx::result<> LoadVisitorsHost(VisitorRegistry& registry,
                              const std::vector<std::string>& library_paths);

}  // namespace fdf_devicetree

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_LOAD_VISITORS_HOST_H_
