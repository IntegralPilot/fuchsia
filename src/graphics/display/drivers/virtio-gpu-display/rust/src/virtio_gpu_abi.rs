// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/504722357): Remove this in favor of more granular
// attributes when the Rust port is completed.
#![allow(dead_code)]

use bitfield::bitfield;
use fidl_next_fuchsia_images2 as fidl_images2;
use std::num::NonZero;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

// These definitions map the specification types "le32" and "le64"
// (little-endian 32/64-bit integers) to u32 and u64, because Fuchsia only
// supports little-endian systems.
//
// Each structure has a test ensuring that the Rust structure definition is
// compatible with the C ABI specified by the spec. Concretely, the tests check
// that the Rust structures have the same size (which implies the same packing)
// and a compatible alignment (same or larger) as the C structures defined by
// the specification.
//
// The specification uses "request" and "command" interchangeably. These
// definitions standardize on "command". "request" must only be used when
// quoting the specification.

bitfield! {
    /// Documented feature bits for virtio-gpu devices.
    // @cite(virtio): sec="5.7.3" title="Feature bits"
    #[repr(transparent)]
    #[derive(Copy, Clone, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
    pub struct FeatureBits(u32);
    impl Debug;

    /// True iff the device supports the virgl 3D mode.
    // @cite(virtio): sec="5.7.3" title="Feature bits"
    // @alias(virtio): theirs="VIRTIO_GPU_F_VIRGL"
    pub bool, supports_virgl_3d, set_supports_virgl_3d: 0;

    /// True iff the device supports EDID.
    // @cite(virtio): sec="5.7.3" title="Feature bits"
    // @alias(virtio): theirs="VIRTIO_GPU_F_EDID"
    pub bool, supports_edid, set_supports_edid: 1;

    /// True iff the device supports assigning resources UUIDs for export to other virtio devices.
    // @cite(virtio): sec="5.7.3" title="Feature bits"
    // @alias(virtio): theirs="VIRTIO_GPU_F_RESOURCE_UUID"
    pub bool, supports_resource_uuids, set_supports_resource_uuids: 2;

    /// True iff the device supports creating and using size-based blob resources.
    // @cite(virtio): sec="5.7.3" title="Feature bits"
    // @alias(virtio): theirs="VIRTIO_GPU_F_RESOURCE_BLOB"
    pub bool, supports_resource_blobs, set_supports_resource_blobs: 3;

    /// True iff the device supports multiple context types and synchronization timelines.
    // @cite(virtio): sec="5.7.3" title="Feature bits"
    // @alias(virtio): theirs="VIRTIO_GPU_F_CONTEXT_INIT"
    pub bool, supports_contexts_and_timelines, set_supports_contexts_and_timelines: 4;

    /// True iff [`DeviceConfiguration::blob_alignment`] is valid.
    // @cite(virtio): sec="5.7.3" title="Feature bits"
    // @alias(virtio): theirs="VIRTIO_GPU_F_BLOB_ALIGNMENT"
    pub bool, blob_alignment_is_valid, set_blob_alignment_is_valid: 5;
}

bitfield! {
    /// Events signaled by the virtio-gpu device.
    // @cite(virtio): sec="5.7.4.2" title="Events"
    #[repr(transparent)]
    #[derive(Copy, Clone, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
    pub struct Events(u32);
    impl Debug;

    /// The display configuration has changed.
    // @cite(virtio): sec="5.7.4.2" title="Events"
    // @alias(virtio): theirs="VIRTIO_GPU_EVENT_DISPLAY"
    pub bool, display_changed, set_display_changed: 0;
}

/// The virtio-gpu device-specific configuration structure.
// @cite(virtio): sec="5.7.4" title="Device configuration layout"
// @alias(virtio): theirs="virtio_gpu_config"
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct DeviceConfiguration {
    /// Signals pending events to the driver.
    ///
    /// Read-only for the driver.
    // @cite(virtio): sec="5.7.4.1" title="Device configuration fields"
    // @alias(virtio): theirs="events_read"
    pub pending_events: Events,

    /// Clears pending events in `pending_events`.
    ///
    /// Write-only for the guest driver.
    ///
    /// The bits have W1/C (Write 1 to Clear) semantics. Writing true (1) into a
    /// bit will clear the corresponding bit in `pending_events`.
    // @cite(virtio): sec="5.7.4.1" title="Device configuration fields"
    // @alias(virtio): theirs="events_clear"
    pub pending_events_to_be_cleared: Events,

    /// The maximum number of scanouts supported by the device.
    ///
    /// Minimum value is 1, maximum value is 16.
    ///
    /// Read-only for the driver.
    // @cite(virtio): sec="5.7.4.1" title="Device configuration fields"
    // @alias(virtio): theirs="num_scanouts"
    pub scanout_count: u32,

    /// The maximum number of capability sets supported by the device.
    ///
    /// The minimum value is zero.
    ///
    /// Read-only for the driver.
    // @cite(virtio): sec="5.7.4.1" title="Device configuration fields"
    // @alias(virtio): theirs="num_capsets"
    pub max_capability_set_count: u32,

    /// The minimum alignment, in bytes, required for resource blobs.
    ///
    /// The value must be a power of two.
    ///
    /// TODO(costan): Check the field's encoding. The specification
    /// states that the field's minimum value is 1, and
    /// the maximum value is 4294967296. The stated maximum is 1 more than
    /// the maximum value that fits in an u32. Either the specification is
    /// wrong, or it fails to document an unusual field encoding. (minus-one?
    /// 0 for 4294967296?)
    ///
    /// Read-only for the driver. Valid if
    /// [`FeatureBits::blob_alignment_is_valid`] was negotiated.
    // @cite(virtio): sec="5.7.4.1" title="Device configuration fields"
    // @alias(virtio): theirs="blob_alignment"
    pub blob_alignment: u32,
}

/// Discriminant for the information in a virtqueue buffer used by virtio-gpu.
// @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
// @alias(virtio): theirs="virtio_gpu_ctrl_type"
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct BufferType(pub u32);

impl BufferType {
    /// Command encoded by [`GetDisplayInfoCommand`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_GET_DISPLAY_INFO"
    pub const GET_DISPLAY_INFO_COMMAND: Self = BufferType(0x0100);

    /// Command encoded by [`Create2DResourceCommand`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_RESOURCE_CREATE_2D"
    pub const CREATE_2D_RESOURCE_COMMAND: Self = BufferType(0x0101);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_RESOURCE_UNREF"
    pub const DESTROY_RESOURCE_COMMAND: Self = BufferType(0x0102);

    /// Command encoded by [`SetScanoutCommand`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_SET_SCANOUT"
    pub const SET_SCANOUT_COMMAND: Self = BufferType(0x0103);

    /// Command encoded by [`FlushResourceCommand`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_RESOURCE_FLUSH"
    pub const FLUSH_RESOURCE_COMMAND: Self = BufferType(0x0104);

    /// Command encoded by [`Transfer2DResourceToHostCommand`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D"
    pub const TRANSFER_2D_RESOURCE_TO_HOST_COMMAND: Self = BufferType(0x0105);

    /// Command encoded by [`AttachResourceBackingCommandHeader`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING"
    pub const ATTACH_RESOURCE_BACKING_COMMAND: Self = BufferType(0x0106);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING"
    pub const DETACH_RESOURCE_BACKING_COMMAND: Self = BufferType(0x0107);

    /// Command encoded by [`GetCapabilitySetInfoCommand`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_GET_CAPSET_INFO"
    pub const GET_CAPABILITY_SET_INFO_COMMAND: Self = BufferType(0x0108);

    /// Command encoded by [`GetCapabilitySetCommand`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_GET_CAPSET"
    pub const GET_CAPABILITY_SET_COMMAND: Self = BufferType(0x0109);

    /// Command encoded by [`GetExtendedDisplayIdCommand`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_GET_EDID"
    pub const GET_EXTENDED_DISPLAY_ID_COMMAND: Self = BufferType(0x010a);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_RESOURCE_ASSIGN_UUID"
    pub const ASSIGN_RESOURCE_UUID_COMMAND: Self = BufferType(0x010b);

    /// Command encoded by [`CreateBlobResourceCommandHeader`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_RESOURCE_CREATE_BLOB"
    pub const CREATE_BLOB_COMMAND: Self = BufferType(0x010c);

    /// Command encoded by [`SetScanoutBlobCommand`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_SET_SCANOUT_BLOB"
    pub const SET_SCANOUT_BLOB_COMMAND: Self = BufferType(0x010d);

    /// Command encoded by [`UpdateCursorCommand`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_UPDATE_CURSOR"
    pub const UPDATE_CURSOR_COMMAND: Self = BufferType(0x0300);

    /// Command encoding reuses [`UpdateCursorCommand`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_CMD_MOVE_CURSOR"
    pub const MOVE_CURSOR_COMMAND: Self = BufferType(0x0301);

    /// Response encoded by [`EmptyResponse`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_OK_NODATA"
    pub const EMPTY_RESPONSE: Self = BufferType(0x1100);

    /// Response encoded by [`DisplayInfoResponse`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_OK_DISPLAY_INFO"
    pub const DISPLAY_INFO_RESPONSE: Self = BufferType(0x1101);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_OK_CAPSET_INFO"
    pub const CAPABILITY_SET_INFO_RESPONSE: Self = BufferType(0x1102);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_OK_CAPSET"
    pub const CAPABILITY_SET_RESPONSE: Self = BufferType(0x1103);

    /// Response encoded by [`ExtendedDisplayIdResponse`].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_OK_EDID"
    pub const EXTENDED_DISPLAY_ID_RESPONSE: Self = BufferType(0x1104);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_OK_RESOURCE_UUID"
    pub const RESOURCE_UUID_RESPONSE: Self = BufferType(0x1105);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_OK_MAP_INFO"
    pub const MAP_INFO_RESPONSE: Self = BufferType(0x1106);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_ERR_UNSPEC"
    pub const UNSPECIFIED_ERROR: Self = BufferType(0x1200);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY"
    pub const OUT_OF_MEMORY_ERROR: Self = BufferType(0x1201);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_ERR_INVALID_SCANOUT_ID"
    pub const INVALID_SCANOUT_ID_ERROR: Self = BufferType(0x1202);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID"
    pub const INVALID_RESOURCE_ID_ERROR: Self = BufferType(0x1203);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_ERR_INVALID_CONTEXT_ID"
    pub const INVALID_CONTEXT_ID_ERROR: Self = BufferType(0x1204);

    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER"
    pub const INVALID_PARAMETER_ERROR: Self = BufferType(0x1205);

    pub fn is_known(&self) -> bool {
        matches!(
            *self,
            Self::GET_DISPLAY_INFO_COMMAND
                | Self::CREATE_2D_RESOURCE_COMMAND
                | Self::DESTROY_RESOURCE_COMMAND
                | Self::SET_SCANOUT_COMMAND
                | Self::FLUSH_RESOURCE_COMMAND
                | Self::TRANSFER_2D_RESOURCE_TO_HOST_COMMAND
                | Self::ATTACH_RESOURCE_BACKING_COMMAND
                | Self::DETACH_RESOURCE_BACKING_COMMAND
                | Self::GET_CAPABILITY_SET_INFO_COMMAND
                | Self::GET_CAPABILITY_SET_COMMAND
                | Self::GET_EXTENDED_DISPLAY_ID_COMMAND
                | Self::ASSIGN_RESOURCE_UUID_COMMAND
                | Self::CREATE_BLOB_COMMAND
                | Self::SET_SCANOUT_BLOB_COMMAND
                | Self::UPDATE_CURSOR_COMMAND
                | Self::MOVE_CURSOR_COMMAND
                | Self::EMPTY_RESPONSE
                | Self::DISPLAY_INFO_RESPONSE
                | Self::CAPABILITY_SET_INFO_RESPONSE
                | Self::CAPABILITY_SET_RESPONSE
                | Self::EXTENDED_DISPLAY_ID_RESPONSE
                | Self::RESOURCE_UUID_RESPONSE
                | Self::MAP_INFO_RESPONSE
                | Self::UNSPECIFIED_ERROR
                | Self::OUT_OF_MEMORY_ERROR
                | Self::INVALID_SCANOUT_ID_ERROR
                | Self::INVALID_RESOURCE_ID_ERROR
                | Self::INVALID_CONTEXT_ID_ERROR
                | Self::INVALID_PARAMETER_ERROR
        )
    }
}

impl std::fmt::Debug for BufferType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::GET_DISPLAY_INFO_COMMAND => write!(f, "GET_DISPLAY_INFO_COMMAND"),
            Self::CREATE_2D_RESOURCE_COMMAND => write!(f, "CREATE_2D_RESOURCE_COMMAND"),
            Self::DESTROY_RESOURCE_COMMAND => write!(f, "DESTROY_RESOURCE_COMMAND"),
            Self::SET_SCANOUT_COMMAND => write!(f, "SET_SCANOUT_COMMAND"),
            Self::FLUSH_RESOURCE_COMMAND => write!(f, "FLUSH_RESOURCE_COMMAND"),
            Self::TRANSFER_2D_RESOURCE_TO_HOST_COMMAND => {
                write!(f, "TRANSFER_2D_RESOURCE_TO_HOST_COMMAND")
            }
            Self::ATTACH_RESOURCE_BACKING_COMMAND => write!(f, "ATTACH_RESOURCE_BACKING_COMMAND"),
            Self::DETACH_RESOURCE_BACKING_COMMAND => write!(f, "DETACH_RESOURCE_BACKING_COMMAND"),
            Self::GET_CAPABILITY_SET_INFO_COMMAND => write!(f, "GET_CAPABILITY_SET_INFO_COMMAND"),
            Self::GET_CAPABILITY_SET_COMMAND => write!(f, "GET_CAPABILITY_SET_COMMAND"),
            Self::GET_EXTENDED_DISPLAY_ID_COMMAND => write!(f, "GET_EXTENDED_DISPLAY_ID_COMMAND"),
            Self::ASSIGN_RESOURCE_UUID_COMMAND => write!(f, "ASSIGN_RESOURCE_UUID_COMMAND"),
            Self::CREATE_BLOB_COMMAND => write!(f, "CREATE_BLOB_COMMAND"),
            Self::SET_SCANOUT_BLOB_COMMAND => write!(f, "SET_SCANOUT_BLOB_COMMAND"),
            Self::UPDATE_CURSOR_COMMAND => write!(f, "UPDATE_CURSOR_COMMAND"),
            Self::MOVE_CURSOR_COMMAND => write!(f, "MOVE_CURSOR_COMMAND"),
            Self::EMPTY_RESPONSE => write!(f, "EMPTY_RESPONSE"),
            Self::DISPLAY_INFO_RESPONSE => write!(f, "DISPLAY_INFO_RESPONSE"),
            Self::CAPABILITY_SET_INFO_RESPONSE => write!(f, "CAPABILITY_SET_INFO_RESPONSE"),
            Self::CAPABILITY_SET_RESPONSE => write!(f, "CAPABILITY_SET_RESPONSE"),
            Self::EXTENDED_DISPLAY_ID_RESPONSE => write!(f, "EXTENDED_DISPLAY_ID_RESPONSE"),
            Self::RESOURCE_UUID_RESPONSE => write!(f, "RESOURCE_UUID_RESPONSE"),
            Self::MAP_INFO_RESPONSE => write!(f, "MAP_INFO_RESPONSE"),
            Self::UNSPECIFIED_ERROR => write!(f, "UNSPECIFIED_ERROR"),
            Self::OUT_OF_MEMORY_ERROR => write!(f, "OUT_OF_MEMORY_ERROR"),
            Self::INVALID_SCANOUT_ID_ERROR => write!(f, "INVALID_SCANOUT_ID_ERROR"),
            Self::INVALID_RESOURCE_ID_ERROR => write!(f, "INVALID_RESOURCE_ID_ERROR"),
            Self::INVALID_CONTEXT_ID_ERROR => write!(f, "INVALID_CONTEXT_ID_ERROR"),
            Self::INVALID_PARAMETER_ERROR => write!(f, "INVALID_PARAMETER_ERROR"),
            _ => write!(f, "UnknownBufferType({})", self.0),
        }
    }
}

bitfield! {
    /// Documented header flags for all buffers in a virtio-gpu queue.
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    #[repr(transparent)]
    #[derive(Copy, Clone, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
    pub struct BufferHeaderFlags(u32);
    impl Debug;

    /// Triggers synchronization between the driver and the device.
    ///
    /// If true, the device must complete the command before sending a response.
    /// The response buffer's header must have the flag set to true, and must
    /// have the same [`BufferHeader::fence_id`] value.
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_FLAG_FENCE"
    pub bool, has_fence_id, set_has_fence_id: 0;

    /// Marks a command as belonging to a rendering context timeline.
    ///
    /// The driver must only set this flag to true if it has negotiated
    /// `FeatureBits::supports_contexts_and_timelines`.
    ///
    /// If true, the command belongs to the timeline uniquely identified by
    /// [`BufferHeader::context_id`] and [`BufferHeader::ring_index`].
    ///
    /// If true and `has_fence_id` is also true, the device must also complete
    /// all the commands that belong to the same rendering context timeline and
    /// were issued before this command.
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="VIRTIO_GPU_FLAG_INFO_RING_IDX"
    pub bool, has_ring_index, set_has_ring_index: 1;
}

/// Header shared by all buffers in virtio-gpu queues.
// @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
// @alias(virtio): theirs="virtio_gpu_ctrl_hdr"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct BufferHeader {
    /// See [`BufferType`].
    pub type_: BufferType,

    /// See [`BufferHeaderFlags`].
    pub flags: BufferHeaderFlags,

    /// Used for synchronization between the driver and the device.
    ///
    /// Only valid if the [`BufferHeaderFlags::has_fence_id`] bit is set to true.
    pub fence_id: u64,

    /// Rendering context ID. Only used in 3D mode.
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="ctx_id"
    pub context_id: u32,

    /// Points to a rendering context-specific timeline for fences.
    ///
    /// Only valid if the [`BufferHeaderFlags::has_ring_index`] bit is set to
    /// true. Values must be in the range [0, 63].
    // @cite(virtio): sec="5.7.6.7" title="Device Operation: Request header"
    // @alias(virtio): theirs="ring_idx"
    pub ring_index: u8,

    pub _padding: [u8; 3],
}

/// virtio-gpu representation of the [`fuchsia.math/RectU`] FIDL structure.
///
/// Instances represent rectangular axis-aligned regions inside raster images.
/// virtio-gpu uses the same coordinate space as Vulkan. The origin is at the
/// image's top-left corner. The X axis points to the right, and the Y axis
/// points downwards.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_rect"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Rectangle {
    /// The X coordinate of the display's position, relative to other displays.
    pub x: u32,

    /// The Y coordinate of the display's position, relative to other displays.
    pub y: u32,

    /// The horizontal size, in pixels.
    pub width: u32,

    /// The vertical size, in pixels.
    pub height: u32,
}

/// virtio-gpu representation of [`fuchsia.images2/PixelFormat`] values.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_formats"
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct ResourceFormat(pub u32);

impl ResourceFormat {
    /// Equivalent to [`fuchsia.images2/PixelFormat.B8G8R8A8`]
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM"
    pub const B8G8R8A8: Self = ResourceFormat(1);

    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM"
    pub const B8G8R8X8: Self = ResourceFormat(2);

    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_FORMAT_A8R8G8B8_UNORM"
    pub const A8R8G8B8: Self = ResourceFormat(3);

    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_FORMAT_X8R8G8B8_UNORM"
    pub const X8R8G8B8: Self = ResourceFormat(4);

    /// Equivalent to [`fuchsia.images2/PixelFormat.R8G8B8A8`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_FORMAT_R8G8B8A8_UNORM"
    pub const R8G8B8A8: Self = ResourceFormat(67);

    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_FORMAT_X8B8G8R8_UNORM"
    pub const X8B8G8R8: Self = ResourceFormat(68);

    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_FORMAT_A8B8G8R8_UNORM"
    pub const A8B8G8R8: Self = ResourceFormat(121);

    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_FORMAT_R8G8B8X8_UNORM"
    pub const R8G8B8X8: Self = ResourceFormat(134);

    pub fn is_known(&self) -> bool {
        matches!(
            *self,
            Self::B8G8R8A8
                | Self::B8G8R8X8
                | Self::A8R8G8B8
                | Self::X8R8G8B8
                | Self::R8G8B8A8
                | Self::X8B8G8R8
                | Self::A8B8G8R8
                | Self::R8G8B8X8
        )
    }
}

impl std::fmt::Debug for ResourceFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::B8G8R8A8 => write!(f, "B8G8R8A8"),
            Self::B8G8R8X8 => write!(f, "B8G8R8X8"),
            Self::A8R8G8B8 => write!(f, "A8R8G8B8"),
            Self::X8R8G8B8 => write!(f, "X8R8G8B8"),
            Self::R8G8B8A8 => write!(f, "R8G8B8A8"),
            Self::X8B8G8R8 => write!(f, "X8B8G8R8"),
            Self::A8B8G8R8 => write!(f, "A8B8G8R8"),
            Self::R8G8B8X8 => write!(f, "R8G8B8X8"),
            _ => write!(f, "UnknownResourceFormat({})", self.0),
        }
    }
}

impl TryFrom<fidl_images2::PixelFormat> for ResourceFormat {
    type Error = zx::Status;

    fn try_from(value: fidl_images2::PixelFormat) -> Result<Self, Self::Error> {
        match value {
            fidl_images2::PixelFormat::B8G8R8A8 => Ok(Self::B8G8R8A8),
            fidl_images2::PixelFormat::R8G8B8A8 => Ok(Self::R8G8B8A8),
            _ => Err(zx::Status::NOT_SUPPORTED),
        }
    }
}

/// Populates a [`DisplayInfoResponse`] with the current output configuration.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct GetDisplayInfoCommand {
    /// `header.type_` must be [`BufferType::GET_DISPLAY_INFO_COMMAND`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,
}

/// virtio-gpu resource ID.
///
/// The guest manages resource IDs. [`Create2DResourceCommand`] assigns a
/// resource ID. [`BufferType::DESTROY_RESOURCE_COMMAND`] (not yet implemented)
/// frees a previously assigned resource ID.
///
/// The virtio protocol for the command [`SetScanoutCommand`] uses a zero
/// `resource_id` to disable a scanout. So, within the scope of
/// [`SetScanoutCommand`], zero is effectively an invalid resource ID. For
/// simplicity, we never use zero as a valid resource ID.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
pub type ResourceId = Option<NonZero<u32>>;

/// Creates a 2D resource on the host.
///
/// This allocates a resource on the host with the specified dimensions and format.
/// The driver must attach backing storage before it can be used.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_resource_create_2d"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Create2DResourceCommand {
    /// `header.type_` must be [`BufferType::CREATE_2D_RESOURCE_COMMAND`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    /// Must not already be assigned to a resource.
    pub resource_id: ResourceId,

    /// Backing image pixel format.
    pub format: ResourceFormat,

    /// Backing image width, in pixels.
    pub width: u32,

    /// Backing image height, in pixels.
    pub height: u32,
}

/// A contiguous list of memory pages assigned to a 2D resource.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_mem_entry"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct MemoryEntry {
    /// Guest physical address of the first page in the memory region.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="addr"
    pub address: u64,

    /// Length of the memory region in bytes.
    pub length: u32,

    pub _padding: u32,
}

/// Assigns backing pages to a resource.
///
/// The response does not have any data.
macro_rules! define_attach_resource_backing_command {
    ($name:ident, $n:expr) => {
        // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
        // @alias(virtio): theirs="virtio_gpu_resource_attach_backing"
        #[repr(C)]
        #[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
        pub struct $name {
            /// `header.type_` must be [`BufferType::ATTACH_RESOURCE_BACKING_COMMAND`].
            // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
            // @alias(virtio): theirs="hdr"
            pub header: BufferHeader,

            /// Must be assigned via a successful [`Create2DResourceCommand`].
            pub resource_id: ResourceId,

            /// Number of populated entries in `entries`.
            ///
            /// Must not exceed the size of `entries`.
            // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
            // @alias(virtio): theirs="nr_entries"
            pub entry_count: u32,

            /// The memory entries.
            pub entries: [MemoryEntry; $n],
        }
    };
}

define_attach_resource_backing_command!(AttachResourceBackingCommandHeader, 0);
define_attach_resource_backing_command!(AttachResourceBackingCommand1, 1);
define_attach_resource_backing_command!(AttachResourceBackingCommand2, 2);

/// Newtype for scanout IDs.
///
/// Scanouts (heads / displays) are identified by their index in
/// [`DisplayInfoResponse::scanouts`]. So, valid IDs are between 0 and
/// [`MAX_SCANOUT_COUNT`].
#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct ScanoutId(pub(crate) u32);

impl ScanoutId {
    /// True iff the value represents a valid scanout ID.
    pub fn is_valid(&self) -> bool {
        self.0 < MAX_SCANOUT_COUNT as u32
    }
}

/// Sets scanout parameters for a single output.
///
/// The response does not have any data.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_set_scanout"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct SetScanoutCommand {
    /// `header.type_` must be [`BufferType::SET_SCANOUT_COMMAND`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    /// The area of the [`resource_id`] image used by the scanout.
    ///
    /// The area must be entirely contained within the resource's dimensions.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="r"
    pub image_source: Rectangle,

    /// The scanout whose pixel data is being displayed.
    pub scanout_id: ScanoutId,

    /// The source of pixel data displayed by the scanout.
    pub resource_id: ResourceId,
}

/// Flushes a scanout resource to the screen.
///
/// The response does not have any data.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_resource_flush"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct FlushResourceCommand {
    /// `header.type_` must be [`BufferType::FLUSH_RESOURCE_COMMAND`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    /// The area of the [`resource_id`] image to be flushed.
    ///
    /// The area must be entirely contained within the resource's dimensions.
    ///
    /// All scanouts that use this area of [`resource_id`] will be updated.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="r"
    pub image_source: Rectangle,

    /// Any scanouts that use this resource will be flushed.
    pub resource_id: ResourceId,

    pub _padding: u32,
}

/// Transfers data from guest memory to host resource.
///
/// The response does not have any data.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_transfer_to_host_2d"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Transfer2DResourceToHostCommand {
    /// `header.type_` must be [`BufferType::TRANSFER_2D_RESOURCE_TO_HOST_COMMAND`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    /// The area of the [`resource_id`] image to be transferred to the host.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="r"
    pub image_source: Rectangle,

    /// The first byte in the host memory that receives pixel data.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="offset"
    pub destination_offset: u64,

    /// Must have backing memory via a successful [`AttachResourceBackingCommandHeader`].
    pub resource_id: ResourceId,

    pub _padding: u32,
}

/// Encodes all device-to-driver responses that have no data besides the header.
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct EmptyResponse {
    /// `header.type_` must be [`BufferType::EMPTY_RESPONSE`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,
}

/// Identifies a capability set (rendering protocol).
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct CapabilitySetId(pub u32);

impl CapabilitySetId {
    /// Gallium OpenGL protocol, first edition.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_CAPSET_VIRGL"
    pub const VIRGL: Self = CapabilitySetId(1);

    /// Gallium OpenGL protocol, second edition.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_CAPSET_VIRGL2"
    pub const VIRGL2: Self = CapabilitySetId(2);

    /// GLES and Vulkan streaming protocols.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_CAPSET_GFXSTREAM"
    pub const GFXSTREAM: Self = CapabilitySetId(3);

    /// Mesa's Vulkan protocol.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_CAPSET_VENUS"
    pub const VENUS: Self = CapabilitySetId(4);

    /// Protocol for display initialization via Wayland proxying.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="VIRTIO_GPU_CAPSET_CROSS_DOMAIN"
    pub const CROSS_DOMAIN: Self = CapabilitySetId(5);
}

impl std::fmt::Debug for CapabilitySetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::VIRGL => write!(f, "VIRGL"),
            Self::VIRGL2 => write!(f, "VIRGL2"),
            Self::GFXSTREAM => write!(f, "GFXSTREAM"),
            Self::VENUS => write!(f, "VENUS"),
            Self::CROSS_DOMAIN => write!(f, "CROSS_DOMAIN"),
            _ => write!(f, "UnknownCapabilitySetId({})", self.0),
        }
    }
}

/// Identifies the blob resource's backing memory pool.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct BlobMemoryPool(pub u32);

impl BlobMemoryPool {
    /// Guest memory.
    pub const GUEST: Self = BlobMemoryPool(0x1);

    /// Host memory accessible to the virtual device's 3D pipeline.
    pub const HOST_3D: Self = BlobMemoryPool(0x2);

    /// Host memory mapped into the guest.
    pub const HOST_3D_GUEST: Self = BlobMemoryPool(0x3);
}

impl std::fmt::Debug for BlobMemoryPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::GUEST => write!(f, "GUEST"),
            Self::HOST_3D => write!(f, "HOST_3D"),
            Self::HOST_3D_GUEST => write!(f, "HOST_3D_GUEST"),
            _ => write!(f, "UnknownBlobMemoryPool({})", self.0),
        }
    }
}

bitfield! {
    /// Information about a blob's planned usage.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    #[repr(transparent)]
    #[derive(Copy, Clone, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
    pub struct BlobUsageFlags(u32);
    impl Debug;

    /// The blob can be mapped into the guest address space.
    pub bool, use_mappable, set_use_mappable: 0;

    /// The blob can be shared with other contexts or devices.
    pub bool, use_shareable, set_use_shareable: 1;

    /// The blob can be used across different devices.
    pub bool, use_cross_device, set_use_cross_device: 2;
}

/// Maximum number of supported scanouts.
// @cite(virtio): sec="5.7.4.1" title="Device configuration fields"
// @alias(virtio): theirs="num_scanouts"
pub const MAX_SCANOUT_COUNT: usize = 16;

/// Information about a single scanout (head).
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_display_one"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct ScanoutInfo {
    /// The scanout's dimensions and placement relative to other scanouts.
    ///
    /// The width and height represent the display's dimensions. The dimensions can
    /// change, because the user can resize the window representing the scanout.
    ///
    /// The position can be used to reason about the scanout's position, in
    /// relation to other scanouts.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="r"
    pub geometry: Rectangle,

    /// True as long as the display is "connected" (enabled by the user).
    ///
    /// This behaves similarly to the voltage level of the HPD (Hot-Plug Detect)
    /// pin in connectors such as DisplayPort and HDMI. This is different from the
    /// HPD interrupt generated by display hardware, which is triggered by changes
    /// to the HPD pin voltage level.
    pub enabled: u32,

    /// No flags are currently documented.
    pub flags: u32,
}

/// Response to a [`GetDisplayInfoCommand`].
///
/// Contains information about all supported scanouts.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_resp_display_info"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct DisplayInfoResponse {
    /// `header.type_` must be [`BufferType::DISPLAY_INFO_RESPONSE`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    /// Identifies the device's (virtual) scanouts (heads / displays).
    ///
    /// [`DeviceConfiguration::scanout_count`] identifies the number of populated
    /// entries.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="pmodes"
    pub scanouts: [ScanoutInfo; MAX_SCANOUT_COUNT],
}

/// Retrieves the EDID data for a given scanout.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_get_edid"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct GetExtendedDisplayIdCommand {
    /// `header.type_` must be [`BufferType::GET_EXTENDED_DISPLAY_ID_COMMAND`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    /// The ID of the scanout to query.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="scanout"
    pub scanout_id: ScanoutId,

    pub _padding: u32,
}

/// Hardcoded size of EDID payload array.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
pub const MAX_EDID_SIZE: usize = 1024;

/// Response to a [`GetExtendedDisplayIdCommand`].
///
/// Contains the EDID blob for the requested scanout.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_resp_edid"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct ExtendedDisplayIdResponse {
    /// `header.type_` must be [`BufferType::EXTENDED_DISPLAY_ID_RESPONSE`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    /// Number of meaningful bytes in [`edid_bytes`].
    ///
    /// Must be at most [`MAX_EDID_SIZE`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="size"
    pub edid_size: u32,

    pub _padding: u32,

    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="edid"
    pub edid_bytes: [u8; MAX_EDID_SIZE],
}

/// Position of the cursor on a specific scanout.
// @cite(virtio): sec="5.7.6.10" title="Device Operation: cursorq"
// @alias(virtio): theirs="virtio_gpu_cursor_pos"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct CursorPosition {
    /// The ID of the scanout to place the cursor on.
    pub scanout_id: ScanoutId,

    /// X coordinate of the cursor, in the scanout's coordinate space.
    pub x: u32,

    /// Y coordinate of the cursor, in the scanout's coordinate space.
    pub y: u32,

    pub _padding: u32,
}

/// Updates the cursor shape and/or position.
///
/// Used for both [`BufferType::UPDATE_CURSOR_COMMAND`] and [`BufferType::MOVE_CURSOR_COMMAND`].
// @cite(virtio): sec="5.7.6.10" title="Device Operation: cursorq"
// @alias(virtio): theirs="virtio_gpu_update_cursor"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct UpdateCursorCommand {
    /// `header.type_` must be [`BufferType::UPDATE_CURSOR_COMMAND`] or
    /// [`BufferType::MOVE_CURSOR_COMMAND`].
    // @cite(virtio): sec="5.7.6.10" title="Device Operation: cursorq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    /// New cursor position.
    // @cite(virtio): sec="5.7.6.10" title="Device Operation: cursorq"
    // @alias(virtio): theirs="pos"
    pub position: CursorPosition,

    /// Ignored when `type` is [`BufferType::MOVE_CURSOR_COMMAND`].
    pub resource_id: ResourceId,

    /// X coordinate of the cursor hotspot, in cursor image coordinates.
    // @cite(virtio): sec="5.7.6.10" title="Device Operation: cursorq"
    // @alias(virtio): theirs="hot_x"
    pub hotspot_x: u32,

    /// Y coordinate of the cursor hotspot, in cursor image coordinates.
    // @cite(virtio): sec="5.7.6.10" title="Device Operation: cursorq"
    // @alias(virtio): theirs="hot_y"
    pub hotspot_y: u32,

    pub _padding: u32,
}

/// Sets scanout parameters for a blob resource.
///
/// Similar to [`SetScanoutCommand`] but for blob resources, supporting multiple planes.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_set_scanout_blob"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct SetScanoutBlobCommand {
    /// `header.type_` must be [`BufferType::SET_SCANOUT_BLOB_COMMAND`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="r"
    pub image_source: Rectangle,

    pub scanout_id: ScanoutId,
    pub resource_id: ResourceId,
    pub width: u32,
    pub height: u32,
    pub format: ResourceFormat,

    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="padding"
    pub _padding: u32,

    /// Strides for up to 4 planes.
    pub strides: [u32; 4],

    /// Offsets for up to 4 planes.
    pub offsets: [u32; 4],
}

/// Creates a blob resource.
///
/// Blob resources can be guest-memory, host-memory, or shared.
///
/// The header is followed by zero or more [`MemoryEntry`] structs
/// in the buffer.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_resource_create_blob"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct CreateBlobResourceCommandHeader {
    /// `header.type_` must be [`BufferType::CREATE_BLOB_COMMAND`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    pub resource_id: ResourceId,

    /// The memory pool backing the blob's data.
    pub memory_pool: BlobMemoryPool,

    /// Indicates the blob's planned usage.
    pub usage_flags: BlobUsageFlags,

    /// Number of entries in the MemoryEntry array that follows this header.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="nr_entries"
    pub entry_count: u32,

    /// Context-local object ID used to create the blob (if applicable).
    pub blob_id: u64,

    /// Size of the blob resource.
    ///
    /// Must be a multiple of [`DeviceConfiguration::blob_alignment`] if the
    /// field is valid.
    pub size: u64,
}

/// Retrieves information about a supported capability set by index.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_get_capset_info"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct GetCapabilitySetInfoCommand {
    /// `header.type_` must be [`BufferType::GET_CAPABILITY_SET_INFO_COMMAND`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    /// Index of the capability set to query, must be less than num_capsets.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="capset_index"
    pub capability_set_index: u32,

    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="padding"
    pub _padding: u32,
}

/// Response containing information about a capability set.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_resp_capset_info"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct GetCapabilitySetInfoResponse {
    /// `header.type_` must be [`BufferType::CAPABILITY_SET_INFO_RESPONSE`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    /// ID of the capability set (e.g., VIRGL, VENUS).
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="capset_id"
    pub capability_set_id: CapabilitySetId,

    /// Maximum supported version of the capability set.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="capset_max_version"
    pub capability_set_max_version: u32,

    /// Maximum size of the capability set data.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="capset_max_size"
    pub capability_set_max_size: u32,

    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="padding"
    pub _padding: u32,
}

/// Retrieves the actual capability set data.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_get_capset"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct GetCapabilitySetCommand {
    /// `header.type_` must be [`BufferType::GET_CAPABILITY_SET_COMMAND`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,

    /// ID of the capability set to retrieve.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="capset_id"
    pub capability_set_id: CapabilitySetId,

    /// Requested version of the capability set.
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="capset_version"
    pub capability_set_version: u32,
}

/// Response containing the capability set data.
///
/// The actual data follows this header in the response buffer.
// @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
// @alias(virtio): theirs="virtio_gpu_resp_capset"
#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct GetCapabilitySetResponseHeader {
    /// `header.type_` must be [`BufferType::CAPABILITY_SET_RESPONSE`].
    // @cite(virtio): sec="5.7.6.8" title="Device Operation: controlq"
    // @alias(virtio): theirs="hdr"
    pub header: BufferHeader,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[fuchsia::test]
    fn test_buffer_header_abi() {
        assert_eq!(size_of::<BufferHeader>(), 24);
        assert_eq!(align_of::<BufferHeader>(), 8);
        assert_eq!(offset_of!(BufferHeader, type_), 0);
        assert_eq!(offset_of!(BufferHeader, flags), 4);
        assert_eq!(offset_of!(BufferHeader, fence_id), 8);
        assert_eq!(offset_of!(BufferHeader, context_id), 16);
        assert_eq!(offset_of!(BufferHeader, ring_index), 20);
        assert_eq!(offset_of!(BufferHeader, _padding), 21);
    }

    #[fuchsia::test]
    fn test_get_display_info_command_abi() {
        assert_eq!(size_of::<GetDisplayInfoCommand>(), 24);
        assert_eq!(align_of::<GetDisplayInfoCommand>(), 8);
        assert_eq!(offset_of!(GetDisplayInfoCommand, header), 0);
    }

    #[fuchsia::test]
    fn test_device_configuration_abi() {
        assert_eq!(size_of::<DeviceConfiguration>(), 20);
        assert_eq!(align_of::<DeviceConfiguration>(), 4);
        assert_eq!(offset_of!(DeviceConfiguration, pending_events), 0);
        assert_eq!(offset_of!(DeviceConfiguration, pending_events_to_be_cleared), 4);
        assert_eq!(offset_of!(DeviceConfiguration, scanout_count), 8);
        assert_eq!(offset_of!(DeviceConfiguration, max_capability_set_count), 12);
        assert_eq!(offset_of!(DeviceConfiguration, blob_alignment), 16);
    }

    #[fuchsia::test]
    fn test_resource_id_abi() {
        assert_eq!(size_of::<ResourceId>(), 4);
        assert_eq!(align_of::<ResourceId>(), 4);
    }

    #[fuchsia::test]
    fn test_scanout_info_abi() {
        assert_eq!(size_of::<ScanoutInfo>(), 24);
        assert_eq!(align_of::<ScanoutInfo>(), 4);
        assert_eq!(offset_of!(ScanoutInfo, geometry), 0);
        assert_eq!(offset_of!(ScanoutInfo, enabled), 16);
        assert_eq!(offset_of!(ScanoutInfo, flags), 20);
    }

    #[fuchsia::test]
    fn test_display_info_response_abi() {
        assert_eq!(size_of::<DisplayInfoResponse>(), 408);
        assert_eq!(align_of::<DisplayInfoResponse>(), 8);
        assert_eq!(offset_of!(DisplayInfoResponse, header), 0);
        assert_eq!(offset_of!(DisplayInfoResponse, scanouts), 24);
    }

    #[fuchsia::test]
    fn test_get_extended_display_id_command_abi() {
        assert_eq!(size_of::<GetExtendedDisplayIdCommand>(), 32);
        assert_eq!(align_of::<GetExtendedDisplayIdCommand>(), 8);
        assert_eq!(offset_of!(GetExtendedDisplayIdCommand, header), 0);
        assert_eq!(offset_of!(GetExtendedDisplayIdCommand, scanout_id), 24);
        assert_eq!(offset_of!(GetExtendedDisplayIdCommand, _padding), 28);
    }

    #[fuchsia::test]
    fn test_extended_display_id_response_abi() {
        assert_eq!(size_of::<ExtendedDisplayIdResponse>(), 1056);
        assert_eq!(align_of::<ExtendedDisplayIdResponse>(), 8);
        assert_eq!(offset_of!(ExtendedDisplayIdResponse, header), 0);
        assert_eq!(offset_of!(ExtendedDisplayIdResponse, edid_size), 24);
        assert_eq!(offset_of!(ExtendedDisplayIdResponse, _padding), 28);
        assert_eq!(offset_of!(ExtendedDisplayIdResponse, edid_bytes), 32);
    }

    #[fuchsia::test]
    fn test_cursor_position_abi() {
        assert_eq!(size_of::<CursorPosition>(), 16);
        assert_eq!(align_of::<CursorPosition>(), 4);
        assert_eq!(offset_of!(CursorPosition, scanout_id), 0);
        assert_eq!(offset_of!(CursorPosition, x), 4);
        assert_eq!(offset_of!(CursorPosition, y), 8);
        assert_eq!(offset_of!(CursorPosition, _padding), 12);
    }

    #[fuchsia::test]
    fn test_update_cursor_command_abi() {
        assert_eq!(size_of::<UpdateCursorCommand>(), 56);
        assert_eq!(align_of::<UpdateCursorCommand>(), 8);
        assert_eq!(offset_of!(UpdateCursorCommand, header), 0);
        assert_eq!(offset_of!(UpdateCursorCommand, position), 24);
        assert_eq!(offset_of!(UpdateCursorCommand, resource_id), 40);
        assert_eq!(offset_of!(UpdateCursorCommand, hotspot_x), 44);
        assert_eq!(offset_of!(UpdateCursorCommand, hotspot_y), 48);
        assert_eq!(offset_of!(UpdateCursorCommand, _padding), 52);
    }

    #[fuchsia::test]
    fn test_set_scanout_blob_command_abi() {
        assert_eq!(size_of::<SetScanoutBlobCommand>(), 96);
        assert_eq!(align_of::<SetScanoutBlobCommand>(), 8);
        assert_eq!(offset_of!(SetScanoutBlobCommand, header), 0);
        assert_eq!(offset_of!(SetScanoutBlobCommand, image_source), 24);
        assert_eq!(offset_of!(SetScanoutBlobCommand, scanout_id), 40);
        assert_eq!(offset_of!(SetScanoutBlobCommand, resource_id), 44);
        assert_eq!(offset_of!(SetScanoutBlobCommand, width), 48);
        assert_eq!(offset_of!(SetScanoutBlobCommand, height), 52);
        assert_eq!(offset_of!(SetScanoutBlobCommand, format), 56);
        assert_eq!(offset_of!(SetScanoutBlobCommand, _padding), 60);
        assert_eq!(offset_of!(SetScanoutBlobCommand, strides), 64);
        assert_eq!(offset_of!(SetScanoutBlobCommand, offsets), 80);
    }

    #[fuchsia::test]
    fn test_create_blob_resource_command_abi() {
        assert_eq!(size_of::<CreateBlobResourceCommandHeader>(), 56);
        assert_eq!(align_of::<CreateBlobResourceCommandHeader>(), 8);
        assert_eq!(offset_of!(CreateBlobResourceCommandHeader, header), 0);
        assert_eq!(offset_of!(CreateBlobResourceCommandHeader, resource_id), 24);
        assert_eq!(offset_of!(CreateBlobResourceCommandHeader, memory_pool), 28);
        assert_eq!(offset_of!(CreateBlobResourceCommandHeader, usage_flags), 32);
        assert_eq!(offset_of!(CreateBlobResourceCommandHeader, entry_count), 36);
        assert_eq!(offset_of!(CreateBlobResourceCommandHeader, blob_id), 40);
        assert_eq!(offset_of!(CreateBlobResourceCommandHeader, size), 48);
    }

    #[fuchsia::test]
    fn test_get_capability_set_info_command_abi() {
        assert_eq!(size_of::<GetCapabilitySetInfoCommand>(), 32);
        assert_eq!(align_of::<GetCapabilitySetInfoCommand>(), 8);
        assert_eq!(offset_of!(GetCapabilitySetInfoCommand, header), 0);
        assert_eq!(offset_of!(GetCapabilitySetInfoCommand, capability_set_index), 24);
        assert_eq!(offset_of!(GetCapabilitySetInfoCommand, _padding), 28);
    }

    #[fuchsia::test]
    fn test_get_capability_set_info_response_abi() {
        assert_eq!(size_of::<GetCapabilitySetInfoResponse>(), 40);
        assert_eq!(align_of::<GetCapabilitySetInfoResponse>(), 8);
        assert_eq!(offset_of!(GetCapabilitySetInfoResponse, header), 0);
        assert_eq!(offset_of!(GetCapabilitySetInfoResponse, capability_set_id), 24);
        assert_eq!(offset_of!(GetCapabilitySetInfoResponse, capability_set_max_version), 28);
        assert_eq!(offset_of!(GetCapabilitySetInfoResponse, capability_set_max_size), 32);
        assert_eq!(offset_of!(GetCapabilitySetInfoResponse, _padding), 36);
    }

    #[fuchsia::test]
    fn test_get_capability_set_command_abi() {
        assert_eq!(size_of::<GetCapabilitySetCommand>(), 32);
        assert_eq!(align_of::<GetCapabilitySetCommand>(), 8);
        assert_eq!(offset_of!(GetCapabilitySetCommand, header), 0);
        assert_eq!(offset_of!(GetCapabilitySetCommand, capability_set_id), 24);
        assert_eq!(offset_of!(GetCapabilitySetCommand, capability_set_version), 28);
    }

    #[fuchsia::test]
    fn test_get_capability_set_response_abi() {
        assert_eq!(size_of::<GetCapabilitySetResponseHeader>(), 24);
        assert_eq!(align_of::<GetCapabilitySetResponseHeader>(), 8);
        assert_eq!(offset_of!(GetCapabilitySetResponseHeader, header), 0);
    }

    #[fuchsia::test]
    fn test_rectangle_abi() {
        assert_eq!(size_of::<Rectangle>(), 16);
        assert_eq!(align_of::<Rectangle>(), 4);
        assert_eq!(offset_of!(Rectangle, x), 0);
        assert_eq!(offset_of!(Rectangle, y), 4);
        assert_eq!(offset_of!(Rectangle, width), 8);
        assert_eq!(offset_of!(Rectangle, height), 12);
    }

    #[fuchsia::test]
    fn test_create_2d_resource_command_abi() {
        assert_eq!(size_of::<Create2DResourceCommand>(), 40);
        assert_eq!(align_of::<Create2DResourceCommand>(), 8);
        assert_eq!(offset_of!(Create2DResourceCommand, header), 0);
        assert_eq!(offset_of!(Create2DResourceCommand, resource_id), 24);
        assert_eq!(offset_of!(Create2DResourceCommand, format), 28);
        assert_eq!(offset_of!(Create2DResourceCommand, width), 32);
        assert_eq!(offset_of!(Create2DResourceCommand, height), 36);
    }

    #[fuchsia::test]
    fn test_memory_entry_abi() {
        assert_eq!(size_of::<MemoryEntry>(), 16);
        assert_eq!(align_of::<MemoryEntry>(), 8);
        assert_eq!(offset_of!(MemoryEntry, address), 0);
        assert_eq!(offset_of!(MemoryEntry, length), 8);
        assert_eq!(offset_of!(MemoryEntry, _padding), 12);
    }

    #[fuchsia::test]
    fn test_attach_resource_backing_command_abi() {
        assert_eq!(size_of::<AttachResourceBackingCommandHeader>(), 32);
        assert_eq!(align_of::<AttachResourceBackingCommandHeader>(), 8);
        assert_eq!(offset_of!(AttachResourceBackingCommandHeader, header), 0);
        assert_eq!(offset_of!(AttachResourceBackingCommandHeader, resource_id), 24);
        assert_eq!(offset_of!(AttachResourceBackingCommandHeader, entry_count), 28);
        assert_eq!(offset_of!(AttachResourceBackingCommandHeader, entries), 32);
    }

    #[fuchsia::test]
    fn test_set_scanout_command_abi() {
        assert_eq!(size_of::<SetScanoutCommand>(), 48);
        assert_eq!(align_of::<SetScanoutCommand>(), 8);
        assert_eq!(offset_of!(SetScanoutCommand, header), 0);
        assert_eq!(offset_of!(SetScanoutCommand, image_source), 24);
        assert_eq!(offset_of!(SetScanoutCommand, scanout_id), 40);
        assert_eq!(offset_of!(SetScanoutCommand, resource_id), 44);
    }

    #[fuchsia::test]
    fn test_flush_resource_command_abi() {
        assert_eq!(size_of::<FlushResourceCommand>(), 48);
        assert_eq!(align_of::<FlushResourceCommand>(), 8);
        assert_eq!(offset_of!(FlushResourceCommand, header), 0);
        assert_eq!(offset_of!(FlushResourceCommand, image_source), 24);
        assert_eq!(offset_of!(FlushResourceCommand, resource_id), 40);
        assert_eq!(offset_of!(FlushResourceCommand, _padding), 44);
    }

    #[fuchsia::test]
    fn test_transfer_2d_resource_to_host_command_abi() {
        assert_eq!(size_of::<Transfer2DResourceToHostCommand>(), 56);
        assert_eq!(align_of::<Transfer2DResourceToHostCommand>(), 8);
        assert_eq!(offset_of!(Transfer2DResourceToHostCommand, header), 0);
        assert_eq!(offset_of!(Transfer2DResourceToHostCommand, image_source), 24);
        assert_eq!(offset_of!(Transfer2DResourceToHostCommand, destination_offset), 40);
        assert_eq!(offset_of!(Transfer2DResourceToHostCommand, resource_id), 48);
        assert_eq!(offset_of!(Transfer2DResourceToHostCommand, _padding), 52);
    }

    #[fuchsia::test]
    fn test_attach_resource_backing_command1_abi() {
        assert_eq!(size_of::<AttachResourceBackingCommand1>(), 48);
        assert_eq!(align_of::<AttachResourceBackingCommand1>(), 8);
        assert_eq!(offset_of!(AttachResourceBackingCommand1, header), 0);
        assert_eq!(offset_of!(AttachResourceBackingCommand1, resource_id), 24);
        assert_eq!(offset_of!(AttachResourceBackingCommand1, entry_count), 28);
        assert_eq!(offset_of!(AttachResourceBackingCommand1, entries), 32);
    }

    #[fuchsia::test]
    fn test_attach_resource_backing_command2_abi() {
        assert_eq!(size_of::<AttachResourceBackingCommand2>(), 64);
        assert_eq!(align_of::<AttachResourceBackingCommand2>(), 8);
        assert_eq!(offset_of!(AttachResourceBackingCommand2, header), 0);
        assert_eq!(offset_of!(AttachResourceBackingCommand2, resource_id), 24);
        assert_eq!(offset_of!(AttachResourceBackingCommand2, entry_count), 28);
        assert_eq!(offset_of!(AttachResourceBackingCommand2, entries), 32);
    }

    #[fuchsia::test]
    fn test_empty_response_abi() {
        assert_eq!(size_of::<EmptyResponse>(), 24);
        assert_eq!(align_of::<EmptyResponse>(), 8);
        assert_eq!(offset_of!(EmptyResponse, header), 0);
    }

    #[fuchsia::test]
    fn test_flags() {
        let mut flags = BufferHeaderFlags(0);
        assert!(!flags.has_fence_id());
        assert!(!flags.has_ring_index());

        flags.set_has_fence_id(true);
        assert!(flags.has_fence_id());
        assert_eq!(flags.0, 1);

        flags.set_has_ring_index(true);
        assert!(flags.has_ring_index());
        assert_eq!(flags.0, 3);
    }

    #[fuchsia::test]
    fn test_buffer_type_debug() {
        assert_eq!(
            format!("{:?}", BufferType::GET_DISPLAY_INFO_COMMAND),
            "GET_DISPLAY_INFO_COMMAND"
        );
        assert_eq!(format!("{:?}", BufferType(33)), "UnknownBufferType(33)");
    }

    #[fuchsia::test]
    fn test_capability_set_id_debug() {
        assert_eq!(format!("{:?}", CapabilitySetId::VIRGL), "VIRGL");
        assert_eq!(format!("{:?}", CapabilitySetId(100)), "UnknownCapabilitySetId(100)");
    }

    #[fuchsia::test]
    fn test_blob_memory_pool_debug() {
        assert_eq!(format!("{:?}", BlobMemoryPool::GUEST), "GUEST");
        assert_eq!(format!("{:?}", BlobMemoryPool(100)), "UnknownBlobMemoryPool(100)");
    }
}
