// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_TESTS_UTILS_SCREEN_CAPTURE_UTILS_H_
#define SRC_UI_SCENIC_TESTS_UTILS_SCREEN_CAPTURE_UTILS_H_

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/ui/scenic/cpp/buffer_collection_import_export_tokens.h>

#include <vector>

#include "src/ui/scenic/tests/utils/flatland_client_with_event_handler.h"

namespace integration_tests {

static constexpr uint32_t kBytesPerPixel = 4;

// BGRA
static constexpr uint8_t kRed[] = {0, 0, 255, 255};
static constexpr uint8_t kGreen[] = {0, 255, 0, 255};
static constexpr uint8_t kBlue[] = {255, 0, 0, 255};
static constexpr uint8_t kYellow[] = {0, 255, 255, 255};
static constexpr uint8_t kZero[] = {0, 0, 0, 0};

bool PixelEquals(const uint8_t* a, const uint8_t* b);

void AppendPixel(std::vector<uint8_t>* values, const uint8_t* pixel);

void GenerateImageForFlatlandInstance(
    uint32_t buffer_collection_index, FlatlandClientWithEventHandler& flatland,
    fuchsia_ui_composition::TransformId parent_transform,
    fuchsia_ui_composition::BufferCollectionImportToken import_token, fuchsia_math::SizeU size,
    fuchsia_math::Vec translation, uint32_t image_id, uint32_t transform_id);

void WriteToSysmemBuffer(const std::vector<uint8_t>& write_values,
                         const fuchsia_sysmem2::BufferCollectionInfo& buffer_collection_info,
                         uint32_t buffer_collection_idx, uint32_t kBytesPerPixel,
                         uint32_t image_width, uint32_t image_height);

fuchsia_sysmem2::BufferCollectionInfo CreateBufferCollectionInfoWithConstraints(
    fuchsia_sysmem2::BufferCollectionConstraints constraints,
    fuchsia_ui_composition::BufferCollectionExportToken export_token,
    fidl::SyncClient<fuchsia_ui_composition::Allocator>& flatland_allocator,
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fuchsia_ui_composition::RegisterBufferCollectionUsages usage);

// This function returns a linear buffer of pixels of size width * height.
std::vector<uint8_t> ExtractScreenCapture(
    uint32_t buffer_id, const fuchsia_sysmem2::BufferCollectionInfo& buffer_collection_info,
    uint32_t kBytesPerPixel, uint32_t render_target_width, uint32_t render_target_height);

}  // namespace integration_tests

#endif  // SRC_UI_SCENIC_TESTS_UTILS_SCREEN_CAPTURE_UTILS_H_
