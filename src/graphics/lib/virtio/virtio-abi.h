// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_LIB_VIRTIO_VIRTIO_ABI_H_
#define SRC_GRAPHICS_LIB_VIRTIO_VIRTIO_ABI_H_

// The constants and structures in this file are from the OASIS Virtual I/O
// Device (VIRTIO) specification, which can be downloaded from
// https://docs.oasis-open.org/virtio/virtio/
//
// virtio14 is Version 1.4, Committee Specification 01, dated 8 April 2026.

// We map the specification types "le32" and "le64" (little-endian 32/64-bit
// integers) to uint32_t and uint64_t, because Fuchsia only supports
// little-endian systems.
//
// We use static_asserts in the associated test file to ensure that our C++
// structure definitions are compatible with the C ABI specified by the spec.
// Concretely, we check that our structures have the same size (which implies
// the same packing) and a compatible alignment (same or larger) as the C
// structures defined by the specification.
//
// The specification uses "request" and "command" interchangeably. This header
// standardizes on "command". "request" must only be used when quoting the
// specification.

#include <cstdint>

namespace virtio_abi {

enum class CapsetId : uint32_t {
  kCapsetVirGl = 1,
  kCapsetVirGl2 = 2,
  kCapsetGfxstream = 3,
  kCapsetVenus = 4,
  kCapsetCrossDomain = 5,
};

// GPU device-specific feature bits are in virtio14 5.7.3 "Feature bits".
// Generic feature bits are in virtio14 6 "Reserved Feature Bits".
struct GpuDeviceFeatures {
  using Flags = uint64_t;

  // VirGL mode is supported.
  //
  // VIRTIO_GPU_F_VIRGL in virtio14 5.7.3 "Feature bits"
  static constexpr Flags kGpuVirGl = uint64_t{1} << 0;

  // EDID is supported.
  //
  // VIRTIO_GPU_F_EDID in virtio14 5.7.3 "Feature bits"
  static constexpr Flags kGpuEdid = uint64_t{1} << 1;

  // Assigning resource UUIDs is supported.
  //
  // VIRTIO_GPU_F_RESOURCE_UUID in virtio14 5.7.3 "Feature bits"
  static constexpr Flags kGpuResourceUuid = uint64_t{1} << 2;

  // Size-based blob resources are supported.
  //
  // VIRTIO_GPU_F_RESOURCE_BLOB in virtio14 5.7.3 "Feature bits"
  static constexpr Flags kGpuResourceBlob = uint64_t{1} << 3;

  // Multiple GPU contexts and timelines are supported.
  //
  // VIRTIO_GPU_F_CONTEXT_INIT in virtio14 5.7.3 "Feature bits"
  static constexpr Flags kGpuMultipleContexts = uint64_t{1} << 4;

  // Modern virtio (1.0 and above) specification supported.
  //
  // VIRTIO_F_VERSION_1 in virtio14 6 "Reserved Feature Bits"
  static constexpr Flags kVirtioVersion1 = uint64_t{1} << 32;

  // Packed virtqueue layout supported.
  //
  // VIRTIO_F_RING_PACKED in virtio14 6 "Reserved Feature Bits"
  static constexpr Flags kPackedQueueFormat = uint64_t{1} << 34;

  // Each virtqueue can be reset individually.
  //
  // VIRTIO_F_RING_RESET in virtio14 6 "Reserved Feature Bits"
  static constexpr Flags kPerQueueReset = uint64_t{1} << 40;
};

// GPU device configuration.
//
// struct virtio_gpu_config in virtio14 5.7.4 "Device configuration layout"
struct GpuDeviceConfig {
  using Events = uint32_t;

  // Informs the driver that the display configuration has changed.
  //
  // The driver is recommended to issue a `ControlType::kGetDisplayInfoCommand`
  // command and update its internal state to reflect changes. If the driver
  // supports EDID, it is also recommended to issue a
  // `ControlType::kGetExtendedDisplayIdCommand` to update its EDID information.
  //
  // VIRTIO_GPU_EVENT_DISPLAY in virtio14 5.7.4.2 "Events"
  static constexpr Events kDisplayConfigChanged = 1 << 0;

  // The driver must not write to this field.
  Events pending_events;

  // Setting bits to one here clears the corresponding bits in `pending_events`.
  //
  // This works similarly to W/C (Write-Clear) registers in hardware.
  Events clear_events;

  // Maximum number of supported scanouts. Values must be in the range [1, 16].
  uint32_t scanout_limit;

  // Maximum number of supported capability sets. May be zero.
  uint32_t capability_set_limit;
};

// Type discriminant for driver commands and device responses.
//
// enum virtio_gpu_ctrl_type in virtio14 5.7.6.7 "Device Operation: Request
// header"
//
// NOLINTNEXTLINE(performance-enum-size): The enum size follows a standard.
enum class ControlType : uint32_t {
  // Command encoded by `GetDisplayInfoCommand`.
  //
  // VIRTIO_GPU_CMD_GET_DISPLAY_INFO
  kGetDisplayInfoCommand = 0x0100,

  // Command encoded by `Create2DResourceCommand`.
  //
  // VIRTIO_GPU_CMD_RESOURCE_CREATE_2D
  kCreate2DResourceCommand = 0x0101,

  // VIRTIO_GPU_CMD_RESOURCE_UNREF
  kDestroyResourceCommand = 0x0102,

  // Command encoded by `SetScanoutCommand`.
  //
  // VIRTIO_GPU_CMD_SET_SCANOUT
  kSetScanoutCommand = 0x0103,

  // Command encoded by `FlushResourceCommand`.
  //
  // VIRTIO_GPU_CMD_RESOURCE_FLUSH
  kFlushResourceCommand = 0x0104,

  // Command encoded by `Transfer2DResourceToHostCommand`.
  //
  // VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D
  kTransfer2DResourceToHostCommand = 0x0105,

  // Command encoded by `AttachResourceBackingCommand`.
  //
  // VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING
  kAttachResourceBackingCommand = 0x0106,

  // VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING
  kDetachResourceBackingCommand = 0x0107,

  // VIRTIO_GPU_CMD_GET_CAPSET_INFO
  kGetCapabilitySetInfoCommand = 0x0108,

  // VIRTIO_GPU_CMD_GET_CAPSET
  kGetCapabilitySetCommand = 0x0109,

  // Command encoded by `GetExtendedDisplayIdCommand`.
  //
  // VIRTIO_GPU_CMD_GET_EDID
  kGetExtendedDisplayIdCommand = 0x010a,

  // VIRTIO_GPU_CMD_RESOURCE_ASSIGN_UUID
  kAssignResourceUuidCommand = 0x010b,

  // VIRTIO_GPU_CMD_CREATE_BLOB
  kCreateBlobCommand = 0x010c,

  // VIRTIO_GPU_CMD_SET_SCANOUT_BLOB
  kSetScanoutBlobCommand = 0x010d,

  // Command encoded by `UpdateCursorCommand`.
  //
  // VIRTIO_GPU_CMD_UPDATE_CURSOR
  kUpdateCursorCommand = 0x0300,

  // Command encoding reuses the `UpdateCursorCommand` structure.
  //
  // VIRTIO_GPU_CMD_MOVE_CURSOR
  kMoveCursorCommand = 0x0301,

  // Response encoded by `EmptyResponse`.
  //
  // VIRTIO_GPU_RESP_OK_NODATA
  kEmptyResponse = 0x1100,

  // Response encoded by `DisplayInfoResponse`.
  //
  // VIRTIO_GPU_RESP_OK_DISPLAY_INFO
  kDisplayInfoResponse = 0x1101,

  // VIRTIO_GPU_RESP_OK_CAPSET_INFO
  kCapabilitySetInfoResponse = 0x1102,

  // VIRTIO_GPU_RESP_OK_CAPSET
  kCapabilitySetResponse = 0x1103,

  // Response encoded by `ExtendedDisplayIdResponse`.
  //
  // VIRTIO_GPU_RESP_OK_EDID
  kExtendedDisplayIdResponse = 0x1104,

  // VIRTIO_GPU_RESP_OK_RESOURCE_UUID
  kResourceUuidResponse = 0x1105,

  // VIRTIO_GPU_RESP_OK_MAP_INFO
  kMapInfoResponse = 0x1106,

  // VIRTIO_GPU_RESP_ERR_UNSPEC
  kUnspecifiedError = 0x1200,

  // VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY
  kOutOfMemoryError = 0x1201,

  // VIRTIO_GPU_RESP_ERR_INVALID_SCANOUT_ID
  kInvalidScanoutIdError = 0x1202,

  // VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID
  kInvalidResourceIdError = 0x1203,

  // VIRTIO_GPU_RESP_ERR_INVALID_CONTEXT_ID
  kInvalidContextIdError = 0x1204,

  // VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER
  kInvalidParameterError = 0x1205,
};

// Descriptor for logging and debugging.
const char* ControlTypeToString(ControlType type);

// struct virtio_gpu_ctrl_hdr in virtio14 5.7.6.7 "Device Operation: Request
// header"
struct ControlHeader {
  using Flags = uint32_t;

  // See `fence_id` and `ring_index` for details.
  //
  // VIRTIO_GPU_FLAG_FENCE
  static constexpr Flags kFence = 1 << 0;

  // See `fence_id` and `ring_index` for details.
  //
  // VIRTIO_GPU_FLAG_INFO_RING_IDX
  static constexpr Flags kRingIndex = 1 << 1;

  ControlType type;

  Flags flags = 0;

  // Used for synchronization between the driver and the device.
  //
  // Only valid if the `kFence` bit is set in the `flags` field.
  //
  // The device must complete a command with the `kFence` flag set before
  // sending a response. The response must also have the `kFence` flag set, and
  // the same `fence_id`.
  uint64_t fence_id = 0;

  // Rendering context ID. Only used in 3D mode.
  uint32_t context_id = 0;

  // Points to a context-specific timeline for fences.
  //
  // Only valid if the `kRingIndex` and `kFence` bits are set in the `flags`
  // field. Values must be in the range [0, 63].
  uint8_t ring_index = 0;
};

// Encodes all driver-to-device commands that have no data besides the header.
struct EmptyCommand {
  ControlHeader header;
};

// Encodes all device-to-driver responses that have no data besides the header.
struct EmptyResponse {
  ControlHeader header;
};

// Populates a `DisplayInfoResponse` with the current output configuration.
using GetDisplayInfoCommand = EmptyCommand;

// struct virtio_gpu_rect in virtio14 5.7.6.8 "Device Operation: controlq",
// under the VIRTIO_GPU_CMD_GET_DISPLAY_INFO command description
struct Rectangle {
  // The x coordinate of the top-left corner.
  //
  // 0 is the origin, the X axis points to the right.
  uint32_t x;

  // Position relative to other displays.
  //
  // 0 is the origin, the Y axis points down.
  uint32_t y;

  // The horizontal size, in pixels.
  uint32_t width;

  // The vertical size, in pixels.
  uint32_t height;
};

// struct virtio_gpu_display_one in virtio14 5.7.6.8 "Device Operation:
// controlq", under the VIRTIO_GPU_CMD_GET_DISPLAY_INFO command description
struct ScanoutInfo {
  // The scanout's dimensions and placement relative to other scanouts.
  //
  // The width and height represent the display's dimensions. The dimensions can
  // change, because the user can resize the window representing the scanout.
  //
  // The position can be used to reason about the scanout's position, in
  // relation to other scanouts.
  Rectangle geometry;

  // True as long as the display is "connected" (enabled by the user).
  //
  // This behaves similarly to the voltage level of the HPD (Hot-Plug Detect)
  // pin in connectors such as DisplayPort and HDMI. This is different from the
  // HPD interrupt generated by display hardware, which is triggered by changes
  // to the HPD pin voltage level.
  uint32_t enabled;

  // No flags are currently documented.
  uint32_t flags;
};

// VIRTIO_GPU_MAX_SCANOUTS in virtio14 5.7.6.8 "Device Operation: controlq",
// under the VIRTIO_GPU_CMD_GET_DISPLAY_INFO command description
constexpr int kMaxScanouts = 16;

// Response to a VIRTIO_GPU_CMD_GET_DISPLAY_INFO command.
//
// struct virtio_gpu_resp_display_info in virtio14 5.7.6.8 "Device Operation:
// controlq", under the VIRTIO_GPU_CMD_GET_DISPLAY_INFO command description
struct DisplayInfoResponse {
  // `type` must be `kDisplayInfoResponse`.
  ControlHeader header;

  ScanoutInfo scanouts[kMaxScanouts];
};

// struct virtio_gpu_get_edid in virtio14 5.7.6.8 "Device Operation: controlq",
// under the VIRTIO_GPU_CMD_GET_EDID command description
struct GetExtendedDisplayIdCommand {
  // `type` must be `kGetExtendedDisplayIdCommand`.
  ControlHeader header;

  uint32_t scanout_id;
  uint32_t padding = 0;
};

// Response to a VIRTIO_GPU_CMD_GET_EDID command.
//
// struct virtio_gpu_resp_edid in virtio14 5.7.6.8 "Device Operation: controlq",
// under the VIRTIO_GPU_CMD_GET_EDID command description
struct ExtendedDisplayIdResponse {
  // Hardcoded size in struct virtio_gpu_resp_edid::edid in virtio14.
  static constexpr int kMaxEdidSize = 1024;

  // `type` must be `kGetExtendedDisplayIdResponse`.
  ControlHeader header;

  // Number of meaningful bytes in `edid_bytes`.
  //
  // Must be at most `kMaxEdidSize`.
  uint32_t edid_size;
  uint32_t padding;
  uint8_t edid_bytes[kMaxEdidSize];
};

// enum virtio_gpu_formats in virtio14 5.7.6.8 "Device Operation:
// controlq", under the VIRTIO_GPU_CMD_RESOURCE_CREATE_2D command description
//
// NOLINTNEXTLINE(performance-enum-size): The enum size follows a standard.
enum class ResourceFormat : uint32_t {
  // Equivalent to [`fuchsia.images2/PixelFormat.B8G8R8A8`]
  //
  // VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM
  kBgra32 = 1,

  // VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM
  kBgrx32 = 2,

  // VIRTIO_GPU_FORMAT_A8R8G8B8_UNORM
  kArgb32 = 3,

  // VIRTIO_GPU_FORMAT_X8R8G8B8_UNORM
  kXrgb32 = 4,

  // Equivalent to [`fuchsia.images2/PixelFormat.R8G8B8A8`].
  //
  // VIRTIO_GPU_FORMAT_R8G8B8A8_UNORM
  kR8g8b8a8 = 67,

  // VIRTIO_GPU_FORMAT_X8B8G8R8_UNORM
  kXbgr32 = 68,

  // VIRTIO_GPU_FORMAT_A8B8G8R8_UNORM
  kAbgr32 = 121,

  // VIRTIO_GPU_FORMAT_R8G8B8X8_UNORM
  kRgbx32 = 134,
};

// Values for CreateBlobResourceCommand::blob_mem.
enum class BlobMem : uint32_t {
  // Guest only blob resource.
  kGuest = 0x1,

  // Host only blob resource.
  kHost3D = 0x2,

  // Default (host and guest) blob resource.
  kHost3D_Guest = 0x3,
};

// Values for CreateBlobResourceCommand::blob_flags.
enum class BlobFlags : uint32_t {
  kUseMappable = 0x1,
  kUseShareable = 0x2,
  kUseCrossDevice = 0x4,
};

// Resource ID that has a special meaning in at least one operation.
//
// virtio14 5.7.6.8 "Device Operation: controlq", the
// VIRTIO_GPU_CMD_SET_SCANOUT command description states that using a resource
// ID with this value disables the scanout.
constexpr uint32_t kInvalidResourceId = 0;

// struct virtio_gpu_resource_create_2d in virtio14 5.7.6.8 "Device Operation:
// controlq", under the VIRTIO_GPU_CMD_RESOURCE_CREATE_2D command description
struct Create2DResourceCommand {
  // `type` must be `kCreate2DResourceCommand`.
  ControlHeader header;

  uint32_t resource_id;
  ResourceFormat format;
  uint32_t width;
  uint32_t height;
};

// Sets scanout parameters for a single output.
//
// The response does not have any data.
//
// struct virtio_gpu_set_scanout in virtio14 5.7.6.8 "Device Operation:
// controlq", under the VIRTIO_GPU_CMD_SET_SCANOUT command description
struct SetScanoutCommand {
  // `type` must be `kSetScanoutCommand`.
  ControlHeader header;

  // The area of the `resource_id` image used by the scanout.
  //
  // The area must be entirely contained within the resource's dimensions.
  Rectangle image_source;

  uint32_t scanout_id;

  // kInvalidResourceId means that the scanout is disabled.
  uint32_t resource_id;
};

struct SetScanoutBlobCommand {
  // `type` must be `kSetScanoutBlobCommand`.
  ControlHeader header;

  // The area of the `resource_id` image used by the scanout.
  //
  // The area must be entirely contained within the resource's dimensions.
  Rectangle image_source;

  uint32_t scanout_id;

  // kInvalidResourceId means that the scanout is disabled.
  uint32_t resource_id;

  uint32_t width;
  uint32_t height;
  uint32_t format;
  uint32_t padding;
  uint32_t strides[4];
  uint32_t offsets[4];
};

// Flushes a scanout resource to the screen.
//
// The response does not have any data.
//
// struct virtio_gpu_resource_flush in virtio14 5.7.6.8 "Device Operation:
// controlq", under the VIRTIO_GPU_CMD_RESOURCE_FLUSH command description
struct FlushResourceCommand {
  // `type` must be `kFlushResourceCommand`.
  ControlHeader header;

  // The area of the `resource_id` image to be flushed.
  //
  // The area must be entirely contained within the resource's dimensions.
  //
  // All scanouts that use this area of `resource_id` will be updated.
  Rectangle image_source;

  // Any scanouts that use this resource will be flushed.
  uint32_t resource_id;
};

// Flushes a scanout resource to the screen.
//
// The response does not have any data.
//
// struct virtio_gpu_transfer_to_host_2d in virtio14 5.7.6.8 "Device Operation:
// controlq", under the VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D command description
struct Transfer2DResourceToHostCommand {
  // `type` must be `kTransfer2DResourceToHostCommand`.
  ControlHeader header;

  // The area of the `resource_id` image to be transferred to the host.
  Rectangle image_source;

  uint64_t destination_offset;
  uint32_t resource_id;
};

// A continuous list of memory pages assigned to a 2D resource.
//
// The response does not have any data.
//
// struct virtio_gpu_mem_entry in virtio14 5.7.6.8 "Device Operation:
// controlq", under the VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING command
// description
struct MemoryEntry {
  uint64_t address;
  uint32_t length;
};

template <uint32_t N>
struct CreateBlobResourceCommand {
  // `type` must be `kCreateBlobResourceCommand`.
  ControlHeader header;

  uint32_t resource_id;
  uint32_t blob_mem;
  uint32_t blob_flags;
  uint32_t nr_entries = N;
  uint64_t blob_id;
  uint64_t size;
  MemoryEntry entries[N];
};

// Assigns backing pages to a resource.
//
// The response does not have any data.
//
// Typesafe combination of struct virtio_gpu_resource_attach_backing and
// struct virtio_gpu_mem_entry in virtio14 5.7.6.8 "Device Operation: controlq",
// under the VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING command
template <uint32_t N>
struct AttachResourceBackingCommand {
  // `type` must be `kAttachResourceBackingCommand`.
  ControlHeader header;
  uint32_t resource_id;
  uint32_t entry_count = N;
  MemoryEntry entries[N];
};

struct GetCapsetInfoCommand {
  ControlHeader header;
  uint32_t capset_index;
  uint32_t padding = 0;
};

struct GetCapsetInfoResponse {
  ControlHeader header;
  uint32_t capset_id;
  uint32_t capset_max_version;
  uint32_t capset_max_size;
  uint32_t padding = 0;
};

struct GetCapsetCommand {
  ControlHeader header;
  uint32_t capset_id;
  uint32_t capset_version;
};

struct GetCapsetResponse {
  ControlHeader header;
  // Variable length response.
  uint8_t capset_data[];
};

// struct virtio_gpu_cursor_pos in virtio14 5.7.6.10 "Device Operation: cursorq"
struct CursorPosition {
  uint32_t scanout_id;
  uint32_t x;
  uint32_t y;
  uint32_t padding = 0;
};

// struct virtio_gpu_update_cursor in virtio14 5.7.6.10 "Device Operation:
// cursorq"
struct UpdateCursorCommand {
  // `type` must be `kUpdateCursorCommand` or `kMoveCursorCommand`.
  ControlHeader header;
  CursorPosition position;

  // Ignored when `type` is `kMoveCursorCommand`
  uint32_t resource_id;
  uint32_t hot_x;
  uint32_t hot_y;
  uint32_t padding = 0;
};

}  // namespace virtio_abi

#endif  // SRC_GRAPHICS_LIB_VIRTIO_VIRTIO_ABI_H_
