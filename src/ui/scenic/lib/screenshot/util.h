// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_SCREENSHOT_UTIL_H_
#define SRC_UI_SCENIC_LIB_SCREENSHOT_UTIL_H_

#include <fidl/fuchsia.io/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async/cpp/wait.h>
#include <lib/async/default.h>
#include <lib/async/dispatcher.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/vfs/cpp/vmo_file.h>

namespace screenshot {

// Serves a screenshot through a channel using |fuchsia.io.File|
// Updates an unordered_map of current screenshots being served.
bool ServeScreenshot(
    fidl::ServerEnd<fuchsia_io::File> file_server, zx::vmo response_vmo, size_t screenshot_index,
    std::unordered_map<size_t,
                       std::pair<std::unique_ptr<vfs::VmoFile>, std::unique_ptr<async::WaitOnce>>>*
        served_screenshots_);

// Extract the data stored in an unreadable vmo, i.e. screenshot on emulator
std::vector<uint8_t> ExtractVmoData(fzl::VmoMapper mapper, size_t size);

// Store a readable version of 'response_vmo' in 'readable_vmo'.
// If a readable version cannot be made, no changes will occur to 'readable_vmo'.
zx_status_t GenerateReadableVmo(zx::vmo response_vmo, size_t vmo_size, zx::vmo* readable_vmo);
}  // namespace screenshot

#endif  // SRC_UI_SCENIC_LIB_SCREENSHOT_UTIL_H_
