// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/lib/virtio/virtio-abi.h"

#include <cstddef>
#include <type_traits>

namespace virtio_abi {

static_assert(std::is_standard_layout_v<GpuDeviceConfig>);
static_assert(sizeof(GpuDeviceConfig) == 16);
static_assert(alignof(GpuDeviceConfig) == 4);
static_assert(offsetof(GpuDeviceConfig, pending_events) == 0);
static_assert(offsetof(GpuDeviceConfig, clear_events) == 4);
static_assert(offsetof(GpuDeviceConfig, scanout_limit) == 8);
static_assert(offsetof(GpuDeviceConfig, capability_set_limit) == 12);

static_assert(std::is_standard_layout_v<ControlHeader>);
static_assert(sizeof(ControlHeader) == 24);
static_assert(alignof(ControlHeader) == 8);
static_assert(offsetof(ControlHeader, type) == 0);
static_assert(offsetof(ControlHeader, flags) == 4);
static_assert(offsetof(ControlHeader, fence_id) == 8);
static_assert(offsetof(ControlHeader, context_id) == 16);
static_assert(offsetof(ControlHeader, ring_index) == 20);

static_assert(std::is_standard_layout_v<EmptyCommand>);
static_assert(sizeof(EmptyCommand) == 24);
static_assert(alignof(EmptyCommand) == 8);
static_assert(offsetof(EmptyCommand, header) == 0);

static_assert(std::is_standard_layout_v<EmptyResponse>);
static_assert(sizeof(EmptyResponse) == 24);
static_assert(alignof(EmptyResponse) == 8);
static_assert(offsetof(EmptyResponse, header) == 0);

static_assert(std::is_standard_layout_v<Rectangle>);
static_assert(sizeof(Rectangle) == 16);
static_assert(alignof(Rectangle) == 4);
static_assert(offsetof(Rectangle, x) == 0);
static_assert(offsetof(Rectangle, y) == 4);
static_assert(offsetof(Rectangle, width) == 8);
static_assert(offsetof(Rectangle, height) == 12);

static_assert(std::is_standard_layout_v<ScanoutInfo>);
static_assert(sizeof(ScanoutInfo) == 24);
static_assert(alignof(ScanoutInfo) == 4);
static_assert(offsetof(ScanoutInfo, geometry) == 0);
static_assert(offsetof(ScanoutInfo, enabled) == 16);
static_assert(offsetof(ScanoutInfo, flags) == 20);

static_assert(std::is_standard_layout_v<DisplayInfoResponse>);
static_assert(sizeof(DisplayInfoResponse) == size_t{24} * 17);
static_assert(alignof(DisplayInfoResponse) == 8);
static_assert(offsetof(DisplayInfoResponse, header) == 0);
static_assert(offsetof(DisplayInfoResponse, scanouts) == 24);

static_assert(std::is_standard_layout_v<GetExtendedDisplayIdCommand>);
static_assert(sizeof(GetExtendedDisplayIdCommand) == 32);
static_assert(alignof(GetExtendedDisplayIdCommand) == 8);
static_assert(offsetof(GetExtendedDisplayIdCommand, header) == 0);
static_assert(offsetof(GetExtendedDisplayIdCommand, scanout_id) == 24);
static_assert(offsetof(GetExtendedDisplayIdCommand, padding) == 28);

static_assert(std::is_standard_layout_v<ExtendedDisplayIdResponse>);
static_assert(sizeof(ExtendedDisplayIdResponse) == 1056);
static_assert(alignof(ExtendedDisplayIdResponse) == 8);
static_assert(offsetof(ExtendedDisplayIdResponse, header) == 0);
static_assert(offsetof(ExtendedDisplayIdResponse, edid_size) == 24);
static_assert(offsetof(ExtendedDisplayIdResponse, padding) == 28);
static_assert(offsetof(ExtendedDisplayIdResponse, edid_bytes) == 32);

static_assert(std::is_standard_layout_v<Create2DResourceCommand>);
static_assert(sizeof(Create2DResourceCommand) == 40);
static_assert(alignof(Create2DResourceCommand) == 8);
static_assert(offsetof(Create2DResourceCommand, header) == 0);
static_assert(offsetof(Create2DResourceCommand, resource_id) == 24);
static_assert(offsetof(Create2DResourceCommand, format) == 28);
static_assert(offsetof(Create2DResourceCommand, width) == 32);
static_assert(offsetof(Create2DResourceCommand, height) == 36);

static_assert(std::is_standard_layout_v<SetScanoutCommand>);
static_assert(sizeof(SetScanoutCommand) == 48);
static_assert(alignof(SetScanoutCommand) == 8);
static_assert(offsetof(SetScanoutCommand, header) == 0);
static_assert(offsetof(SetScanoutCommand, image_source) == 24);
static_assert(offsetof(SetScanoutCommand, scanout_id) == 40);
static_assert(offsetof(SetScanoutCommand, resource_id) == 44);

static_assert(std::is_standard_layout_v<FlushResourceCommand>);
static_assert(sizeof(FlushResourceCommand) == 48);
static_assert(alignof(FlushResourceCommand) == 8);
static_assert(offsetof(FlushResourceCommand, header) == 0);
static_assert(offsetof(FlushResourceCommand, image_source) == 24);
static_assert(offsetof(FlushResourceCommand, resource_id) == 40);

static_assert(std::is_standard_layout_v<Transfer2DResourceToHostCommand>);
static_assert(sizeof(Transfer2DResourceToHostCommand) == 56);
static_assert(alignof(Transfer2DResourceToHostCommand) == 8);
static_assert(offsetof(Transfer2DResourceToHostCommand, header) == 0);
static_assert(offsetof(Transfer2DResourceToHostCommand, image_source) == 24);
static_assert(offsetof(Transfer2DResourceToHostCommand, destination_offset) == 40);
static_assert(offsetof(Transfer2DResourceToHostCommand, resource_id) == 48);

static_assert(std::is_standard_layout_v<MemoryEntry>);
static_assert(sizeof(MemoryEntry) == 16);
static_assert(alignof(MemoryEntry) == 8);
static_assert(offsetof(MemoryEntry, address) == 0);
static_assert(offsetof(MemoryEntry, length) == 8);

static_assert(std::is_standard_layout_v<AttachResourceBackingCommand<1>>);
static_assert(sizeof(AttachResourceBackingCommand<1>) == 48);
static_assert(alignof(AttachResourceBackingCommand<1>) == 8);
static_assert(offsetof(AttachResourceBackingCommand<1>, header) == 0);
static_assert(offsetof(AttachResourceBackingCommand<1>, resource_id) == 24);
static_assert(offsetof(AttachResourceBackingCommand<1>, entry_count) == 28);
static_assert(offsetof(AttachResourceBackingCommand<1>, entries) == 32);
static_assert(std::is_standard_layout_v<AttachResourceBackingCommand<2>>);
static_assert(sizeof(AttachResourceBackingCommand<2>) == 64);
static_assert(alignof(AttachResourceBackingCommand<2>) == 8);
static_assert(offsetof(AttachResourceBackingCommand<2>, header) == 0);
static_assert(offsetof(AttachResourceBackingCommand<2>, resource_id) == 24);
static_assert(offsetof(AttachResourceBackingCommand<2>, entry_count) == 28);
static_assert(offsetof(AttachResourceBackingCommand<2>, entries) == 32);

static_assert(std::is_standard_layout_v<CursorPosition>);
static_assert(sizeof(CursorPosition) == 16);
static_assert(alignof(CursorPosition) == 4);
static_assert(offsetof(CursorPosition, scanout_id) == 0);
static_assert(offsetof(CursorPosition, x) == 4);
static_assert(offsetof(CursorPosition, y) == 8);
static_assert(offsetof(CursorPosition, padding) == 12);

static_assert(std::is_standard_layout_v<UpdateCursorCommand>);
static_assert(sizeof(UpdateCursorCommand) == 56);
static_assert(alignof(UpdateCursorCommand) == 8);
static_assert(offsetof(UpdateCursorCommand, header) == 0);
static_assert(offsetof(UpdateCursorCommand, position) == 24);
static_assert(offsetof(UpdateCursorCommand, resource_id) == 40);
static_assert(offsetof(UpdateCursorCommand, hot_x) == 44);
static_assert(offsetof(UpdateCursorCommand, hot_y) == 48);
static_assert(offsetof(UpdateCursorCommand, padding) == 52);

static_assert(std::is_standard_layout_v<SetScanoutBlobCommand>);
static_assert(sizeof(SetScanoutBlobCommand) == 96);
static_assert(alignof(SetScanoutBlobCommand) == 8);
static_assert(offsetof(SetScanoutBlobCommand, header) == 0);
static_assert(offsetof(SetScanoutBlobCommand, image_source) == 24);
static_assert(offsetof(SetScanoutBlobCommand, scanout_id) == 40);
static_assert(offsetof(SetScanoutBlobCommand, resource_id) == 44);
static_assert(offsetof(SetScanoutBlobCommand, width) == 48);
static_assert(offsetof(SetScanoutBlobCommand, height) == 52);
static_assert(offsetof(SetScanoutBlobCommand, format) == 56);
static_assert(offsetof(SetScanoutBlobCommand, padding) == 60);
static_assert(offsetof(SetScanoutBlobCommand, strides) == 64);
static_assert(offsetof(SetScanoutBlobCommand, offsets) == 80);

static_assert(std::is_standard_layout_v<CreateBlobResourceCommand<0>>);
static_assert(sizeof(CreateBlobResourceCommand<0>) == 56);
static_assert(alignof(CreateBlobResourceCommand<0>) == 8);
static_assert(offsetof(CreateBlobResourceCommand<0>, header) == 0);
static_assert(offsetof(CreateBlobResourceCommand<0>, resource_id) == 24);
static_assert(offsetof(CreateBlobResourceCommand<0>, blob_mem) == 28);
static_assert(offsetof(CreateBlobResourceCommand<0>, blob_flags) == 32);
static_assert(offsetof(CreateBlobResourceCommand<0>, nr_entries) == 36);
static_assert(offsetof(CreateBlobResourceCommand<0>, blob_id) == 40);
static_assert(offsetof(CreateBlobResourceCommand<0>, size) == 48);

static_assert(std::is_standard_layout_v<GetCapsetInfoCommand>);
static_assert(sizeof(GetCapsetInfoCommand) == 32);
static_assert(alignof(GetCapsetInfoCommand) == 8);
static_assert(offsetof(GetCapsetInfoCommand, header) == 0);
static_assert(offsetof(GetCapsetInfoCommand, capset_index) == 24);
static_assert(offsetof(GetCapsetInfoCommand, padding) == 28);

static_assert(std::is_standard_layout_v<GetCapsetInfoResponse>);
static_assert(sizeof(GetCapsetInfoResponse) == 40);
static_assert(alignof(GetCapsetInfoResponse) == 8);
static_assert(offsetof(GetCapsetInfoResponse, header) == 0);
static_assert(offsetof(GetCapsetInfoResponse, capset_id) == 24);
static_assert(offsetof(GetCapsetInfoResponse, capset_max_version) == 28);
static_assert(offsetof(GetCapsetInfoResponse, capset_max_size) == 32);
static_assert(offsetof(GetCapsetInfoResponse, padding) == 36);

static_assert(std::is_standard_layout_v<GetCapsetCommand>);
static_assert(sizeof(GetCapsetCommand) == 32);
static_assert(alignof(GetCapsetCommand) == 8);
static_assert(offsetof(GetCapsetCommand, header) == 0);
static_assert(offsetof(GetCapsetCommand, capset_id) == 24);
static_assert(offsetof(GetCapsetCommand, capset_version) == 28);

static_assert(std::is_standard_layout_v<GetCapsetResponse>);
static_assert(sizeof(GetCapsetResponse) == 24);
static_assert(alignof(GetCapsetResponse) == 8);
static_assert(offsetof(GetCapsetResponse, header) == 0);

}  // namespace virtio_abi
