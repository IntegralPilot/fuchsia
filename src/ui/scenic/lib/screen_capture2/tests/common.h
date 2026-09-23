// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_SCREEN_CAPTURE2_TESTS_COMMON_H_
#define SRC_UI_SCENIC_LIB_SCREEN_CAPTURE2_TESTS_COMMON_H_

#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>
#include <lib/zx/event.h>

#include "src/ui/scenic/lib/allocation/allocator.h"
#include "src/ui/scenic/lib/screen_capture/screen_capture_buffer_collection_importer.h"

namespace screen_capture2 {
namespace test {

std::shared_ptr<allocation::Allocator> CreateAllocator(
    std::shared_ptr<screen_capture::ScreenCaptureBufferCollectionImporter> importer,
    sys::ComponentContext* app_context, async_dispatcher_t* dispatcher);

void CreateBufferCollectionInfoWithConstraints(
    fuchsia_sysmem2::BufferCollectionConstraints constraints,
    fuchsia_ui_composition::BufferCollectionExportToken export_token,
    std::shared_ptr<allocation::Allocator> flatland_allocator,
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator, async_dispatcher_t* dispatcher,
    fit::function<void(fit::function<bool()>)> run_loop_until);

}  // namespace test
}  // namespace screen_capture2
#endif  // SRC_UI_SCENIC_LIB_SCREEN_CAPTURE2_TESTS_COMMON_H_
