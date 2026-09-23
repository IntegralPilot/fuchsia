// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::Result;
use crate::pixel_format::PixelFormat;
use fidl_fuchsia_hardware_display::{
    BufferCollectionId as FidlBufferCollectionId, ClientPriority as FidlClientPriority,
    EventId as FidlEventId, ImageId as FidlImageId, Info, LayerId as FidlLayerId,
    TEST_UTILITY_CLIENT_PRIORITY_VALUE as FidlTestUtilityClientPriorityValue,
};
use fidl_fuchsia_hardware_display_types::{
    AlphaMode, Color as FidlColor, DisplayId as FidlDisplayId, INVALID_DISP_ID,
};
use fuchsia_async::OnSignals;
use std::fmt;

/// Strongly typed wrapper around a display ID.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DisplayId(pub u64);

/// Represents an invalid DisplayId value.
pub const INVALID_DISPLAY_ID: DisplayId = DisplayId(INVALID_DISP_ID);

impl Default for DisplayId {
    fn default() -> Self {
        INVALID_DISPLAY_ID
    }
}

impl From<FidlDisplayId> for DisplayId {
    fn from(fidl_display_id: FidlDisplayId) -> Self {
        DisplayId(fidl_display_id.value)
    }
}

impl From<DisplayId> for FidlDisplayId {
    fn from(display_id: DisplayId) -> Self {
        FidlDisplayId { value: display_id.0 }
    }
}

/// Strongly typed wrapper around a display driver event ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventId(pub u64);

/// Represents an invalid EventId value.
pub const INVALID_EVENT_ID: EventId = EventId(INVALID_DISP_ID);

impl Default for EventId {
    fn default() -> Self {
        INVALID_EVENT_ID
    }
}

impl From<FidlEventId> for EventId {
    fn from(fidl_event_id: FidlEventId) -> Self {
        EventId(fidl_event_id.value)
    }
}

impl From<EventId> for FidlEventId {
    fn from(event_id: EventId) -> Self {
        FidlEventId { value: event_id.0 }
    }
}

/// Strongly typed wrapper around a display layer ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayerId(pub u64);

/// Represents an invalid LayerId value.
pub const INVALID_LAYER_ID: LayerId = LayerId(INVALID_DISP_ID);

impl Default for LayerId {
    fn default() -> Self {
        INVALID_LAYER_ID
    }
}

impl From<FidlLayerId> for LayerId {
    fn from(fidl_layer_id: FidlLayerId) -> Self {
        LayerId(fidl_layer_id.value)
    }
}

impl From<LayerId> for FidlLayerId {
    fn from(layer_id: LayerId) -> Self {
        FidlLayerId { value: layer_id.0 }
    }
}

/// Strongly typed wrapper around an image ID.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ImageId(pub u64);

/// Represents an invalid ImageId value.
pub const INVALID_IMAGE_ID: ImageId = ImageId(INVALID_DISP_ID);

impl Default for ImageId {
    fn default() -> Self {
        INVALID_IMAGE_ID
    }
}

impl From<FidlImageId> for ImageId {
    fn from(fidl_image_id: FidlImageId) -> Self {
        ImageId(fidl_image_id.value)
    }
}

impl From<ImageId> for FidlImageId {
    fn from(image_id: ImageId) -> Self {
        FidlImageId { value: image_id.0 }
    }
}

/// Strongly typed wrapper around a sysmem buffer collection ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferCollectionId(pub u64);

impl From<FidlBufferCollectionId> for BufferCollectionId {
    fn from(fidl_buffer_collection_id: FidlBufferCollectionId) -> Self {
        BufferCollectionId(fidl_buffer_collection_id.value)
    }
}

impl From<BufferCollectionId> for FidlBufferCollectionId {
    fn from(buffer_collection_id: BufferCollectionId) -> Self {
        FidlBufferCollectionId { value: buffer_collection_id.0 }
    }
}

/// Strongly typed wrapper around a client priority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientPriority(pub u32);

impl From<FidlClientPriority> for ClientPriority {
    fn from(fidl_client_priority: FidlClientPriority) -> Self {
        ClientPriority(fidl_client_priority.value)
    }
}

impl From<ClientPriority> for FidlClientPriority {
    fn from(client_priority: ClientPriority) -> Self {
        FidlClientPriority { value: client_priority.0 }
    }
}

/// The priority level used by display test utilities.
pub const TEST_UTILITY_CLIENT_PRIORITY: ClientPriority =
    ClientPriority(FidlTestUtilityClientPriorityValue);

/// Enhances the `fuchsia.hardware.display.Info` FIDL struct.
#[derive(Clone, Debug)]
pub struct DisplayInfo(pub Info);

impl DisplayInfo {
    /// Returns the ID for this display.
    pub fn id(&self) -> DisplayId {
        self.0.id.into()
    }
}

/// Custom user-friendly format representation.
impl fmt::Display for DisplayInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Display (id: {})", self.0.id.value)?;
        writeln!(f, "\tManufacturer Name: \"{}\"", self.0.manufacturer_name)?;
        writeln!(f, "\tMonitor Name: \"{}\"", self.0.monitor_name)?;
        writeln!(f, "\tMonitor Serial: \"{}\"", self.0.monitor_serial)?;
        writeln!(
            f,
            "\tPhysical Dimensions: {}mm x {}mm",
            self.0.horizontal_size_mm, self.0.vertical_size_mm
        )?;

        writeln!(f, "\tPixel Formats:")?;
        for (i, format) in self.0.pixel_format.iter().map(PixelFormat::from).enumerate() {
            writeln!(f, "\t\t{}:\t{}", i, format)?;
        }

        writeln!(f, "\tDisplay Modes:")?;
        for (i, mode) in self.0.modes.iter().enumerate() {
            writeln!(
                f,
                "\t\t{}:\t{:.2} Hz @ {}x{}",
                i,
                (mode.refresh_rate_millihertz as f32) / 1000.,
                mode.active_area.width,
                mode.active_area.height
            )?;
        }

        write!(f, "")
    }
}

/// A zircon event that has been registered with the display driver.
pub struct Event {
    id: EventId,
    event: zx::Event,
}

impl Event {
    pub(crate) fn new(id: EventId, event: zx::Event) -> Event {
        Event { id, event }
    }

    /// Returns the ID for this event.
    pub fn id(&self) -> EventId {
        self.id
    }

    /// Returns a future that completes when the event has been signaled.
    pub async fn wait(&self) -> Result<()> {
        OnSignals::new(&self.event, zx::Signals::EVENT_SIGNALED).await?;
        self.event.as_handle_ref().signal(zx::Signals::EVENT_SIGNALED, zx::Signals::NONE)?;
        Ok(())
    }

    /// Signals the event.
    pub fn signal(&self) -> Result<()> {
        self.event.as_handle_ref().signal(zx::Signals::NONE, zx::Signals::EVENT_SIGNALED)?;
        Ok(())
    }
}

/// Represents the alpha blending configuration for a layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alpha {
    /// The blending mode to use.
    pub mode: AlphaMode,
    /// The constant alpha value to apply, in the range [0.0, 1.0].
    pub val: f32,
}

/// Enhances the `fuchsia.hardware.display.typers.Color` FIDL struct.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    /// The format of `bytes`.
    pub format: PixelFormat,

    /// The constant color, represented as one pixel using `format`.
    pub bytes: [u8; 8],
}

impl From<FidlColor> for Color {
    fn from(fidl_color: FidlColor) -> Self {
        Color { format: fidl_color.format.into(), bytes: fidl_color.bytes }
    }
}

impl From<&FidlColor> for Color {
    fn from(fidl_color: &FidlColor) -> Self {
        Self::from(*fidl_color)
    }
}

impl From<Color> for FidlColor {
    fn from(color: Color) -> Self {
        FidlColor { format: color.format.into(), bytes: color.bytes }
    }
}

impl From<&Color> for FidlColor {
    fn from(color: &Color) -> Self {
        Self::from(*color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_images2::PixelFormat as FidlPixelFormat;
    use googletest::prelude::*;

    #[gtest]
    #[fuchsia::test]
    fn layer_id_from_fidl_layer_id() {
        expect_eq!(LayerId(1), LayerId::from(FidlLayerId { value: 1 }));
        expect_eq!(LayerId(2), LayerId::from(FidlLayerId { value: 2 }));
        const LARGE: u64 = 1 << 63;
        expect_eq!(LayerId(LARGE), LayerId::from(FidlLayerId { value: LARGE }));
        expect_eq!(INVALID_LAYER_ID, LayerId::from(FidlLayerId { value: INVALID_DISP_ID }));
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_layer_id_from_layer_id() {
        expect_eq!(FidlLayerId { value: 1 }, FidlLayerId::from(LayerId(1)));
        expect_eq!(FidlLayerId { value: 2 }, FidlLayerId::from(LayerId(2)));
        const LARGE: u64 = 1 << 63;
        expect_eq!(FidlLayerId { value: LARGE }, FidlLayerId::from(LayerId(LARGE)));
        expect_eq!(FidlLayerId { value: INVALID_DISP_ID }, FidlLayerId::from(INVALID_LAYER_ID));
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_layer_id_to_layer_id() {
        expect_eq!(LayerId(1), FidlLayerId { value: 1 }.into());
        expect_eq!(LayerId(2), FidlLayerId { value: 2 }.into());
        const LARGE: u64 = 1 << 63;
        expect_eq!(LayerId(LARGE), FidlLayerId { value: LARGE }.into());
        expect_eq!(INVALID_LAYER_ID, FidlLayerId { value: INVALID_DISP_ID }.into());
    }

    #[gtest]
    #[fuchsia::test]
    fn layer_id_to_fidl_layer_id() {
        expect_eq!(FidlLayerId { value: 1 }, LayerId(1).into());
        expect_eq!(FidlLayerId { value: 2 }, LayerId(2).into());
        const LARGE: u64 = 1 << 63;
        expect_eq!(FidlLayerId { value: LARGE }, LayerId(LARGE).into());
        expect_eq!(FidlLayerId { value: INVALID_DISP_ID }, INVALID_LAYER_ID.into());
    }

    #[gtest]
    #[fuchsia::test]
    fn layer_id_default() {
        let default: LayerId = Default::default();
        expect_eq!(default, INVALID_LAYER_ID);
    }

    #[gtest]
    #[fuchsia::test]
    fn display_id_from_fidl_display_id() {
        expect_eq!(DisplayId(1), DisplayId::from(FidlDisplayId { value: 1 }));
        expect_eq!(DisplayId(2), DisplayId::from(FidlDisplayId { value: 2 }));
        const LARGE: u64 = 1 << 63;
        expect_eq!(DisplayId(LARGE), DisplayId::from(FidlDisplayId { value: LARGE }));
        expect_eq!(INVALID_DISPLAY_ID, DisplayId::from(FidlDisplayId { value: INVALID_DISP_ID }));
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_display_id_from_display_id() {
        expect_eq!(FidlDisplayId { value: 1 }, FidlDisplayId::from(DisplayId(1)));
        expect_eq!(FidlDisplayId { value: 2 }, FidlDisplayId::from(DisplayId(2)));
        const LARGE: u64 = 1 << 63;
        expect_eq!(FidlDisplayId { value: LARGE }, FidlDisplayId::from(DisplayId(LARGE)));
        expect_eq!(
            FidlDisplayId { value: INVALID_DISP_ID },
            FidlDisplayId::from(INVALID_DISPLAY_ID)
        );
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_display_id_to_display_id() {
        expect_eq!(DisplayId(1), FidlDisplayId { value: 1 }.into());
        expect_eq!(DisplayId(2), FidlDisplayId { value: 2 }.into());
        const LARGE: u64 = 1 << 63;
        expect_eq!(DisplayId(LARGE), FidlDisplayId { value: LARGE }.into());
        expect_eq!(INVALID_DISPLAY_ID, FidlDisplayId { value: INVALID_DISP_ID }.into());
    }

    #[gtest]
    #[fuchsia::test]
    fn display_id_to_fidl_display_id() {
        expect_eq!(FidlDisplayId { value: 1 }, DisplayId(1).into());
        expect_eq!(FidlDisplayId { value: 2 }, DisplayId(2).into());
        const LARGE: u64 = 1 << 63;
        expect_eq!(FidlDisplayId { value: LARGE }, DisplayId(LARGE).into());
        expect_eq!(FidlDisplayId { value: INVALID_DISP_ID }, INVALID_DISPLAY_ID.into());
    }

    #[gtest]
    #[fuchsia::test]
    fn display_id_default() {
        let default: DisplayId = Default::default();
        expect_eq!(default, INVALID_DISPLAY_ID);
    }

    #[gtest]
    #[fuchsia::test]
    fn buffer_collection_id_from_fidl_buffer_collection_id() {
        expect_eq!(
            BufferCollectionId(1),
            BufferCollectionId::from(FidlBufferCollectionId { value: 1 })
        );
        expect_eq!(
            BufferCollectionId(2),
            BufferCollectionId::from(FidlBufferCollectionId { value: 2 })
        );
        const LARGE: u64 = 1 << 63;
        expect_eq!(
            BufferCollectionId(LARGE),
            BufferCollectionId::from(FidlBufferCollectionId { value: LARGE })
        );
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_buffer_collection_id_from_buffer_collection_id() {
        expect_eq!(
            FidlBufferCollectionId { value: 1 },
            FidlBufferCollectionId::from(BufferCollectionId(1))
        );
        expect_eq!(
            FidlBufferCollectionId { value: 2 },
            FidlBufferCollectionId::from(BufferCollectionId(2))
        );
        const LARGE: u64 = 1 << 63;
        expect_eq!(
            FidlBufferCollectionId { value: LARGE },
            FidlBufferCollectionId::from(BufferCollectionId(LARGE))
        );
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_buffer_collection_id_to_buffer_collection_id() {
        expect_eq!(BufferCollectionId(1), FidlBufferCollectionId { value: 1 }.into());
        expect_eq!(BufferCollectionId(2), FidlBufferCollectionId { value: 2 }.into());
        const LARGE: u64 = 1 << 63;
        expect_eq!(BufferCollectionId(LARGE), FidlBufferCollectionId { value: LARGE }.into());
    }

    #[gtest]
    #[fuchsia::test]
    fn buffer_collection_id_to_fidl_buffer_collection_id() {
        expect_eq!(FidlBufferCollectionId { value: 1 }, BufferCollectionId(1).into());
        expect_eq!(FidlBufferCollectionId { value: 2 }, BufferCollectionId(2).into());
        const LARGE: u64 = 1 << 63;
        expect_eq!(FidlBufferCollectionId { value: LARGE }, BufferCollectionId(LARGE).into());
    }

    #[gtest]
    #[fuchsia::test]
    fn event_id_from_fidl_event_id() {
        expect_eq!(EventId(1), EventId::from(FidlEventId { value: 1 }));
        expect_eq!(EventId(2), EventId::from(FidlEventId { value: 2 }));
        const LARGE: u64 = 1 << 63;
        expect_eq!(EventId(LARGE), EventId::from(FidlEventId { value: LARGE }));
        expect_eq!(INVALID_EVENT_ID, EventId::from(FidlEventId { value: INVALID_DISP_ID }));
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_event_id_from_event_id() {
        expect_eq!(FidlEventId { value: 1 }, FidlEventId::from(EventId(1)));
        expect_eq!(FidlEventId { value: 2 }, FidlEventId::from(EventId(2)));
        const LARGE: u64 = 1 << 63;
        expect_eq!(FidlEventId { value: LARGE }, FidlEventId::from(EventId(LARGE)));
        expect_eq!(FidlEventId { value: INVALID_DISP_ID }, FidlEventId::from(INVALID_EVENT_ID));
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_event_id_to_event_id() {
        expect_eq!(EventId(1), FidlEventId { value: 1 }.into());
        expect_eq!(EventId(2), FidlEventId { value: 2 }.into());
        const LARGE: u64 = 1 << 63;
        expect_eq!(EventId(LARGE), FidlEventId { value: LARGE }.into());
        expect_eq!(INVALID_EVENT_ID, FidlEventId { value: INVALID_DISP_ID }.into());
    }

    #[gtest]
    #[fuchsia::test]
    fn event_id_to_fidl_event_id() {
        expect_eq!(FidlEventId { value: 1 }, EventId(1).into());
        expect_eq!(FidlEventId { value: 2 }, EventId(2).into());
        const LARGE: u64 = 1 << 63;
        expect_eq!(FidlEventId { value: LARGE }, EventId(LARGE).into());
        expect_eq!(FidlEventId { value: INVALID_DISP_ID }, INVALID_EVENT_ID.into());
    }

    #[gtest]
    #[fuchsia::test]
    fn event_id_default() {
        let default: EventId = Default::default();
        expect_eq!(default, INVALID_EVENT_ID);
    }

    #[gtest]
    #[fuchsia::test]
    fn image_id_from_fidl_image_id() {
        expect_eq!(ImageId(1), ImageId::from(FidlImageId { value: 1 }));
        expect_eq!(ImageId(2), ImageId::from(FidlImageId { value: 2 }));
        const LARGE: u64 = 1 << 63;
        expect_eq!(ImageId(LARGE), ImageId::from(FidlImageId { value: LARGE }));
        expect_eq!(INVALID_IMAGE_ID, ImageId::from(FidlImageId { value: INVALID_DISP_ID }));
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_image_id_from_image_id() {
        expect_eq!(FidlImageId { value: 1 }, FidlImageId::from(ImageId(1)));
        expect_eq!(FidlImageId { value: 2 }, FidlImageId::from(ImageId(2)));
        const LARGE: u64 = 1 << 63;
        expect_eq!(FidlImageId { value: LARGE }, FidlImageId::from(ImageId(LARGE)));
        expect_eq!(FidlImageId { value: INVALID_DISP_ID }, FidlImageId::from(INVALID_IMAGE_ID));
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_image_id_to_image_id() {
        expect_eq!(ImageId(1), FidlImageId { value: 1 }.into());
        expect_eq!(ImageId(2), FidlImageId { value: 2 }.into());
        const LARGE: u64 = 1 << 63;
        expect_eq!(ImageId(LARGE), FidlImageId { value: LARGE }.into());
        expect_eq!(INVALID_IMAGE_ID, FidlImageId { value: INVALID_DISP_ID }.into());
    }

    #[gtest]
    #[fuchsia::test]
    fn image_id_to_fidl_image_id() {
        expect_eq!(FidlImageId { value: 1 }, ImageId(1).into());
        expect_eq!(FidlImageId { value: 2 }, ImageId(2).into());
        const LARGE: u64 = 1 << 63;
        expect_eq!(FidlImageId { value: LARGE }, ImageId(LARGE).into());
        expect_eq!(FidlImageId { value: INVALID_DISP_ID }, INVALID_IMAGE_ID.into());
    }

    #[gtest]
    #[fuchsia::test]
    fn image_id_default() {
        let default: ImageId = Default::default();
        expect_eq!(default, INVALID_IMAGE_ID);
    }

    #[gtest]
    #[fuchsia::test]
    fn color_from_fidl_color() {
        expect_eq!(
            Color {
                format: PixelFormat::R8G8B8A8,
                bytes: [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48]
            },
            Color::from(FidlColor {
                format: FidlPixelFormat::R8G8B8A8,
                bytes: [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48],
            })
        );
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_color_from_color() {
        expect_eq!(
            FidlColor {
                format: FidlPixelFormat::R8G8B8A8,
                bytes: [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48]
            },
            FidlColor::from(Color {
                format: PixelFormat::R8G8B8A8,
                bytes: [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48],
            })
        );
    }

    #[gtest]
    #[fuchsia::test]
    fn fidl_color_to_color() {
        expect_eq!(
            Color {
                format: PixelFormat::R8G8B8A8,
                bytes: [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48]
            },
            FidlColor {
                format: FidlPixelFormat::R8G8B8A8,
                bytes: [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48]
            }
            .into()
        );
    }

    #[gtest]
    #[fuchsia::test]
    fn color_to_fidl_color() {
        expect_eq!(
            FidlColor {
                format: FidlPixelFormat::R8G8B8A8,
                bytes: [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48]
            },
            Color {
                format: PixelFormat::R8G8B8A8,
                bytes: [0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48]
            }
            .into()
        );
    }
}
