// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::{Alpha, Color, DisplayId, EventId, ImageId, LayerId};
use {fidl_fuchsia_hardware_display_types as fdisplay_types, fidl_fuchsia_math as fmath};

/// LayerConfig is a variant type of the two distinct layer configuration types that are
/// supported by the display driver: Primary and Color.
// TODO(armansito): Complete the missing layer parameters.
#[derive(Clone, Debug)]
pub enum LayerConfig {
    /// A color layer contains a single color.
    Color {
        /// The layer's color.
        color: Color,
        /// The destination frame on the display for the color layer.
        display_destination: fmath::RectU,
    },

    /// A primary layer is draws its pixels from a sysmem buffer backed image and supports various
    /// transofmations.
    Primary {
        /// The ID of the image that should be assigned to the primary layer. See the `image` mod
        /// in this crate to negotiate an image buffer with the display driver that can be used in
        /// this configuration.
        image_id: ImageId,

        /// Describes the dimensions, pixel format, and usage of the layer image.
        image_metadata: fdisplay_types::ImageMetadata,

        /// When present, the display driver will not apply the configuration until the client
        /// signals this event.
        unblock_event: Option<EventId>,

        /// Optional alpha blending configuration for the layer.
        /// If None, the driver's default alpha behavior is used.
        alpha: Option<Alpha>,
    },
}

/// Represents an individual layer configuration.
#[derive(Clone, Debug)]
pub struct Layer {
    /// The ID of the layer. A layer ID can be obtained from a `Controller` instance by creating
    /// a layer.
    pub id: LayerId,

    /// Describes how the layer should be configured.
    pub config: LayerConfig,
}

/// Represents an individual display configuration.
#[derive(Clone, Debug)]
pub struct DisplayConfig {
    /// The ID of the display to configure.
    pub id: DisplayId,

    /// The list of layers in ascending z-order that should be assigned to the display.
    pub layers: Vec<Layer>,
}
