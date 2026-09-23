// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{anyhow, format_err, Error};
use fidl_fuchsia_math::{RectU, SizeU};
use {
    fidl_fuchsia_images2 as fimages2, fidl_fuchsia_sysmem as fsysmem,
    fidl_fuchsia_sysmem2 as fsysmem2,
};

use super::linux_drm::DRM_FORMAT_MOD_LINEAR;
use super::round_up_to_increment;

/// The default image format constraints for allocating buffers.
pub const IMAGE_FORMAT_CONSTRAINTS_DEFAULT: fsysmem::ImageFormatConstraints =
    fsysmem::ImageFormatConstraints {
        pixel_format: fsysmem::PixelFormat {
            type_: fsysmem::PixelFormatType::Nv12,
            has_format_modifier: false,
            format_modifier: fsysmem::FormatModifier { value: 0 },
        },
        color_spaces_count: 0,
        color_space: [fsysmem::ColorSpace { type_: fsysmem::ColorSpaceType::Invalid }; 32],
        min_coded_width: 0,
        max_coded_width: 0,
        min_coded_height: 0,
        max_coded_height: 0,
        min_bytes_per_row: 0,
        max_bytes_per_row: 0,
        max_coded_width_times_coded_height: 0,
        layers: 0,
        coded_width_divisor: 0,
        coded_height_divisor: 0,
        bytes_per_row_divisor: 0,
        start_offset_divisor: 0,
        display_width_divisor: 0,
        display_height_divisor: 0,
        required_min_coded_width: 0,
        required_max_coded_width: 0,
        required_min_coded_height: 0,
        required_max_coded_height: 0,
        required_min_bytes_per_row: 0,
        required_max_bytes_per_row: 0,
    };

/// The default buffers usage for allocating buffers.
pub const BUFFER_USAGE_DEFAULT: fsysmem::BufferUsage =
    fsysmem::BufferUsage { none: 0, cpu: 0, vulkan: 0, display: 0, video: 0 };

/// The default buffer memory constraints for allocating buffers.
pub const BUFFER_MEMORY_CONSTRAINTS_DEFAULT: fsysmem::BufferMemoryConstraints =
    fsysmem::BufferMemoryConstraints {
        min_size_bytes: 0,
        max_size_bytes: u32::MAX,
        physically_contiguous_required: false,
        secure_required: false,
        ram_domain_supported: false,
        cpu_domain_supported: true,
        inaccessible_domain_supported: false,
        heap_permitted_count: 0,
        heap_permitted: [fsysmem::HeapType::SystemRam; 32],
    };

/// The default buffer collection constraints for allocating buffers.
pub const BUFFER_COLLECTION_CONSTRAINTS_DEFAULT: fsysmem::BufferCollectionConstraints =
    fsysmem::BufferCollectionConstraints {
        usage: BUFFER_USAGE_DEFAULT,
        min_buffer_count_for_camping: 0,
        min_buffer_count_for_dedicated_slack: 0,
        min_buffer_count_for_shared_slack: 0,
        min_buffer_count: 0,
        max_buffer_count: 0,
        has_buffer_memory_constraints: false,
        buffer_memory_constraints: BUFFER_MEMORY_CONSTRAINTS_DEFAULT,
        image_format_constraints_count: 0,
        image_format_constraints: [IMAGE_FORMAT_CONSTRAINTS_DEFAULT; 32],
    };

/// Returns the number of bytes per row for a given plane.
///
/// Returns an error if no such number of bytes can be found, either because a number can't be
/// generated from `image_format` or because the `plane` is unsupported.
pub fn get_plane_row_bytes_2(
    image_format: &fimages2::ImageFormat,
    plane: u32,
) -> Result<u32, Error> {
    let bytes_per_row = *image_format
        .bytes_per_row
        .as_ref()
        .ok_or_else(|| anyhow!("ImageFormat.bytes_per_row missing - tiled format?"))?;
    match plane {
        0 => Ok(bytes_per_row),
        1 => {
            let pixel_format = image_format.pixel_format.as_ref().expect("pixel_format");
            match pixel_format {
                fimages2::PixelFormat::Nv12 => Ok(bytes_per_row),
                fimages2::PixelFormat::I420 | fimages2::PixelFormat::Yv12 => Ok(bytes_per_row / 2),
                _ => Err(anyhow!("Invalid pixel format for plane 1.")),
            }
        }
        2 => {
            let pixel_format = image_format.pixel_format.as_ref().expect("pixel_format");
            match pixel_format {
                fimages2::PixelFormat::I420 | fimages2::PixelFormat::Yv12 => Ok(bytes_per_row / 2),
                _ => Err(anyhow!("Invalid pixel format for plane 2.")),
            }
        }
        _ => Err(anyhow!("Invalid plane.")),
    }
}

/// Returns the number of bytes per row for a given plane.
///
/// Returns an error if no such number of bytes can be found, either because a number can't be
/// generated from `image_format` or because the `plane` is unsupported.
pub fn get_plane_row_bytes(image_format: &fsysmem::ImageFormat2, plane: u32) -> Result<u32, Error> {
    match plane {
        0 => Ok(image_format.bytes_per_row),
        1 => match image_format.pixel_format.type_ {
            fsysmem::PixelFormatType::Nv12 => Ok(image_format.bytes_per_row),
            fsysmem::PixelFormatType::I420 | fsysmem::PixelFormatType::Yv12 => {
                Ok(image_format.bytes_per_row / 2)
            }
            _ => Err(anyhow!("Invalid pixel format for plane 1.")),
        },
        2 => match image_format.pixel_format.type_ {
            fsysmem::PixelFormatType::I420 | fsysmem::PixelFormatType::Yv12 => {
                Ok(image_format.bytes_per_row / 2)
            }
            _ => Err(anyhow!("Invalid pixel format for plane 2.")),
        },
        _ => Err(anyhow!("Invalid plane.")),
    }
}

/// Returns the byte offset for the given plane.
///
/// Returns an error if the `plane` is unsupported or a valid offset can't be generated from
/// `image_format`.
pub fn image_format_plane_byte_offset_2(
    image_format: &fimages2::ImageFormat,
    plane: u32,
) -> Result<u32, Error> {
    match plane {
        0 => Ok(0),
        1 => match image_format.pixel_format.as_ref().expect("pixel_format") {
            fimages2::PixelFormat::Nv12
            | fimages2::PixelFormat::I420
            | fimages2::PixelFormat::Yv12 => Ok(image_format.size.as_ref().expect("size").height
                * image_format.bytes_per_row.as_ref().expect("bytes_per_row")),
            _ => Err(anyhow!("Invalid pixelformat for plane 1.")),
        },
        2 => match image_format.pixel_format.as_ref().expect("pixel_format") {
            fimages2::PixelFormat::I420 | fimages2::PixelFormat::Yv12 => {
                let size = image_format.size.as_ref().expect("size");
                let bytes_per_row = image_format.bytes_per_row.as_ref().expect("bytes_per_row");
                Ok(size.height * bytes_per_row + size.height / 2 * bytes_per_row / 2)
            }
            _ => Err(anyhow!("Invalid pixelformat for plane 2.")),
        },
        _ => Err(anyhow!("Invalid plane.")),
    }
}

/// Returns the byte offset for the given plane.
///
/// Returns an error if the `plane` is unsupported or a valid offset can't be generated from
/// `image_format`.
pub fn image_format_plane_byte_offset(
    image_format: &fsysmem::ImageFormat2,
    plane: u32,
) -> Result<u32, Error> {
    match plane {
        0 => Ok(0),
        1 => match image_format.pixel_format.type_ {
            fsysmem::PixelFormatType::Nv12
            | fsysmem::PixelFormatType::I420
            | fsysmem::PixelFormatType::Yv12 => {
                Ok(image_format.coded_height * image_format.bytes_per_row)
            }
            _ => Err(anyhow!("Invalid pixelformat for plane 1.")),
        },
        2 => match image_format.pixel_format.type_ {
            fsysmem::PixelFormatType::I420 | fsysmem::PixelFormatType::Yv12 => {
                Ok(image_format.coded_height * image_format.bytes_per_row
                    + image_format.coded_height / 2 * image_format.bytes_per_row / 2)
            }
            _ => Err(anyhow!("Invalid pixelformat for plane 2.")),
        },
        _ => Err(anyhow!("Invalid plane.")),
    }
}

/// Returns the linear size for the given `type_`.
///
/// Returns an error if `type_` is unsupported.
pub fn linear_size(
    coded_height: u32,
    bytes_per_row: u32,
    type_: &fsysmem::PixelFormatType,
) -> Result<u32, Error> {
    match type_ {
        fsysmem::PixelFormatType::R8G8B8A8
        | fsysmem::PixelFormatType::Bgra32
        | fsysmem::PixelFormatType::Bgr24
        | fsysmem::PixelFormatType::Rgb565
        | fsysmem::PixelFormatType::Rgb332
        | fsysmem::PixelFormatType::Rgb2220
        | fsysmem::PixelFormatType::L8
        | fsysmem::PixelFormatType::R8
        | fsysmem::PixelFormatType::R8G8
        | fsysmem::PixelFormatType::A2B10G10R10
        | fsysmem::PixelFormatType::A2R10G10B10 => Ok(coded_height * bytes_per_row),
        fsysmem::PixelFormatType::I420 => Ok(coded_height * bytes_per_row * 3 / 2),
        fsysmem::PixelFormatType::M420 => Ok(coded_height * bytes_per_row * 3 / 2),
        fsysmem::PixelFormatType::Nv12 => Ok(coded_height * bytes_per_row * 3 / 2),
        fsysmem::PixelFormatType::Yuy2 => Ok(coded_height * bytes_per_row),
        fsysmem::PixelFormatType::Yv12 => Ok(coded_height * bytes_per_row * 3 / 2),
        _ => Err(anyhow!("Invalid pixel format.")),
    }
}

/// Converts a `fsysmem::ImageFormatConstraints` to an `fimages2::ImageFormat`.
pub fn constraints_to_image_format(
    constraints: &fsysmem2::ImageFormatConstraints,
    coded_width: u32,
    coded_height: u32,
) -> Result<fimages2::ImageFormat, Error> {
    if let Some(min_size) = &constraints.min_size {
        if coded_width < min_size.width {
            return Err(anyhow!("Coded width < min_size.width"));
        }
        if coded_height < min_size.height {
            return Err(anyhow!("Coded height < min_size.height"));
        }
    }
    if let Some(max_size) = &constraints.max_size {
        if coded_width > max_size.width {
            return Err(anyhow!("Coded width > max_size.width"));
        }
        if coded_height > max_size.height {
            return Err(anyhow!("Coded height > max_size.height"));
        }
    }

    let pixel_format_and_modifier = first_pixel_format_and_modifier_from_constraints(constraints)?;

    let color_space = if constraints.color_spaces.is_some()
        && constraints.color_spaces.as_ref().unwrap().len() > 0
    {
        Some(constraints.color_spaces.as_ref().unwrap()[0])
    } else {
        None
    };

    let format = fimages2::ImageFormat {
        pixel_format: Some(pixel_format_and_modifier.pixel_format),
        pixel_format_modifier: Some(pixel_format_and_modifier.pixel_format_modifier),
        size: Some(SizeU { width: coded_width, height: coded_height }),
        bytes_per_row: image_format_minimum_row_bytes_2(constraints, coded_width).ok(),
        color_space,
        ..Default::default()
    };

    Ok(format)
}

/// Converts a `fsysmem::ImageFormatConstraints` to an `fsysmem::ImageFormat2`.
pub fn constraints_to_format(
    constraints: &fsysmem::ImageFormatConstraints,
    coded_width: u32,
    coded_height: u32,
) -> Result<fsysmem::ImageFormat2, Error> {
    if coded_width < constraints.min_coded_width
        || (constraints.max_coded_width > 0 && coded_width > constraints.max_coded_width)
    {
        return Err(anyhow!("Coded width not within constraint bounds."));
    }
    if coded_height < constraints.min_coded_height
        || (constraints.max_coded_height > 0 && coded_height > constraints.max_coded_height)
    {
        return Err(anyhow!("Coded height not within constraint bounds."));
    }

    let format = fsysmem::ImageFormat2 {
        pixel_format: constraints.pixel_format,
        coded_width,
        coded_height,
        bytes_per_row: image_format_minimum_row_bytes(constraints, coded_width).unwrap_or(0),
        display_width: coded_width,
        display_height: coded_height,
        layers: 0,
        color_space: if constraints.color_spaces_count > 0 {
            constraints.color_space[0]
        } else {
            fsysmem::ColorSpace { type_: fsysmem::ColorSpaceType::Invalid }
        },
        has_pixel_aspect_ratio: false,
        pixel_aspect_ratio_width: 0,
        pixel_aspect_ratio_height: 0,
    };

    Ok(format)
}

fn first_pixel_format_and_modifier_from_constraints(
    constraints: &fsysmem2::ImageFormatConstraints,
) -> Result<fsysmem2::PixelFormatAndModifier, Error> {
    Ok(if let Some(pixel_format) = constraints.pixel_format {
        fsysmem2::PixelFormatAndModifier {
            pixel_format,
            pixel_format_modifier: *constraints
                .pixel_format_modifier
                .as_ref()
                .unwrap_or(&fimages2::PixelFormatModifier::Linear),
        }
    } else {
        constraints
            .pixel_format_and_modifiers
            .as_ref()
            .ok_or_else(|| format_err!("missing pixel_format"))?
            .get(0)
            .ok_or_else(|| format_err!("missing pixel_format"))?
            .clone()
    })
}

/// Returns the minimum row bytes for the given constraints and width.
///
/// Returns an error if the width is invalid given the constraints, or the constraint pixel format
/// modifier is invalid.
pub fn image_format_minimum_row_bytes_2(
    constraints: &fsysmem2::ImageFormatConstraints,
    width: u32,
) -> Result<u32, Error> {
    if let Some(pixel_format_modifier) = constraints.pixel_format_modifier {
        if pixel_format_modifier != fimages2::PixelFormatModifier::Linear {
            return Err(anyhow!("Non-linear format modifier."));
        }
    }
    if let Some(min_size) = constraints.min_size {
        if width < min_size.width {
            return Err(anyhow!("width < min_size.width"));
        }
    }
    if let Some(max_size) = constraints.max_size {
        if width > max_size.width {
            return Err(anyhow!("width > max_size.width"));
        }
    }

    let constraints_min_bytes_per_row = constraints.min_bytes_per_row.unwrap_or(0);

    let maybe_stride_bytes_per_width_pixel = image_format_stride_bytes_per_width_pixel_2(
        *constraints.pixel_format.as_ref().expect("pixel_format"),
    )
    .ok();

    let mut bytes_per_row_divisor = constraints.bytes_per_row_divisor.unwrap_or(1);
    if *constraints.require_bytes_per_row_at_pixel_boundary.as_ref().unwrap_or(&false) {
        let stride_bytes_per_width_pixel = *maybe_stride_bytes_per_width_pixel.as_ref().ok_or_else(|| format_err!("stride_bytes_per_width_pixel required when require_bytes_per_row_at_pixel_boundary true"))?;
        bytes_per_row_divisor =
            num::integer::lcm(bytes_per_row_divisor, stride_bytes_per_width_pixel);
    }
    let bytes_per_row_divisor = bytes_per_row_divisor;

    round_up_to_increment(
        std::cmp::max(
            maybe_stride_bytes_per_width_pixel.unwrap_or(0) * width,
            constraints_min_bytes_per_row,
        ) as usize,
        bytes_per_row_divisor as usize,
    )
    .map(|bytes| bytes as u32)
}

/// Returns the minimum row bytes for the given constraints and width.
///
/// Returns an error if the width is invalid given the constraints, or the constraint pixel format
/// modifier is invalid.
pub fn image_format_minimum_row_bytes(
    constraints: &fsysmem::ImageFormatConstraints,
    width: u32,
) -> Result<u32, Error> {
    if constraints.pixel_format.format_modifier.value != DRM_FORMAT_MOD_LINEAR {
        return Err(anyhow!("Non-linear format modifier."));
    }
    if width < constraints.min_coded_width
        || (constraints.max_coded_width > 0 && width > constraints.max_coded_width)
    {
        return Err(anyhow!("Width outside of constraints."));
    }

    let constraints_min_bytes_per_row = constraints.min_bytes_per_row;
    let constraints_bytes_per_row_divisor = constraints.bytes_per_row_divisor;

    round_up_to_increment(
        std::cmp::max(
            image_format_stride_bytes_per_width_pixel(&constraints.pixel_format) * width,
            constraints_min_bytes_per_row,
        ) as usize,
        constraints_bytes_per_row_divisor as usize,
    )
    .map(|bytes| bytes as u32)
}

pub fn image_format_stride_bytes_per_width_pixel_2(
    pixel_format: fimages2::PixelFormat,
) -> Result<u32, Error> {
    match pixel_format {
        fimages2::PixelFormat::R8G8B8A8 => Ok(4),
        fimages2::PixelFormat::R8G8B8X8 => Ok(4),
        fimages2::PixelFormat::B8G8R8A8 => Ok(4),
        fimages2::PixelFormat::B8G8R8X8 => Ok(4),
        fimages2::PixelFormat::B8G8R8 => Ok(3),
        fimages2::PixelFormat::R8G8B8 => Ok(3),
        fimages2::PixelFormat::I420 => Ok(1),
        fimages2::PixelFormat::M420 => Ok(1),
        fimages2::PixelFormat::Nv12 => Ok(1),
        fimages2::PixelFormat::Yuy2 => Ok(2),
        fimages2::PixelFormat::Yv12 => Ok(1),
        fimages2::PixelFormat::R5G6B5 => Ok(2),
        fimages2::PixelFormat::R3G3B2 => Ok(1),
        fimages2::PixelFormat::R2G2B2X2 => Ok(1),
        fimages2::PixelFormat::L8 => Ok(1),
        fimages2::PixelFormat::R8 => Ok(1),
        fimages2::PixelFormat::R8G8 => Ok(2),
        fimages2::PixelFormat::A2B10G10R10 => Ok(4),
        fimages2::PixelFormat::A2R10G10B10 => Ok(4),
        fimages2::PixelFormat::P010 => Ok(2),
        _ => Err(format_err!(
            "stride_bytes_per_width_pixel not available for pixel_format: {:?}",
            pixel_format
        )),
    }
}

pub fn image_format_stride_bytes_per_width_pixel(pixel_format: &fsysmem::PixelFormat) -> u32 {
    match pixel_format.type_ {
        fsysmem::PixelFormatType::Invalid
        | fsysmem::PixelFormatType::Mjpeg
        | fsysmem::PixelFormatType::DoNotCare => 0,
        fsysmem::PixelFormatType::R8G8B8A8 => 4,
        fsysmem::PixelFormatType::Bgra32 => 4,
        fsysmem::PixelFormatType::Bgr24 => 3,
        fsysmem::PixelFormatType::I420 => 1,
        fsysmem::PixelFormatType::M420 => 1,
        fsysmem::PixelFormatType::Nv12 => 1,
        fsysmem::PixelFormatType::Yuy2 => 2,
        fsysmem::PixelFormatType::Yv12 => 1,
        fsysmem::PixelFormatType::Rgb565 => 2,
        fsysmem::PixelFormatType::Rgb332 => 1,
        fsysmem::PixelFormatType::Rgb2220 => 1,
        fsysmem::PixelFormatType::L8 => 1,
        fsysmem::PixelFormatType::R8 => 1,
        fsysmem::PixelFormatType::R8G8 => 2,
        fsysmem::PixelFormatType::A2B10G10R10 => 4,
        fsysmem::PixelFormatType::A2R10G10B10 => 4,
    }
}

pub fn sysmem1_pixel_format_type_from_images2_pixel_format(
    pixel_format: fimages2::PixelFormat,
) -> Result<fsysmem::PixelFormatType, Error> {
    fsysmem::PixelFormatType::from_primitive(pixel_format.into_primitive())
        .ok_or_else(|| format_err!("pixel_format not convertible to sysmem1: {:?}", pixel_format))
}

pub fn images2_pixel_format_from_sysmem_pixel_format_type(
    pixel_format: fsysmem::PixelFormatType,
) -> Result<fimages2::PixelFormat, Error> {
    fimages2::PixelFormat::from_primitive(pixel_format.into_primitive())
        .ok_or_else(|| format_err!("pixel_format not convertible to images2: {:?}", pixel_format))
}

pub fn sysmem1_pixel_format_modifier_from_images2_pixel_format_modifier(
    pixel_format_modifier: fimages2::PixelFormatModifier,
) -> u64 {
    if pixel_format_modifier == fimages2::PixelFormatModifier::GoogleGoldfishOptimal {
        return fsysmem::FORMAT_MODIFIER_GOOGLE_GOLDFISH_OPTIMAL;
    }
    pixel_format_modifier.into_primitive()
}

pub fn images2_pixel_format_modifier_from_sysmem_pixel_format_modifier(
    pixel_format_modifier: u64,
) -> Result<fimages2::PixelFormatModifier, Error> {
    if pixel_format_modifier == fsysmem::FORMAT_MODIFIER_GOOGLE_GOLDFISH_OPTIMAL {
        return Ok(fimages2::PixelFormatModifier::GoogleGoldfishOptimal);
    }
    fimages2::PixelFormatModifier::from_primitive(pixel_format_modifier).ok_or_else(|| {
        format_err!("pixel_format_modifier not convertible to images2: {:?}", pixel_format_modifier)
    })
}

pub fn sysmem1_color_space_type_from_images2_color_space(
    color_space: fimages2::ColorSpace,
) -> Result<fsysmem::ColorSpaceType, Error> {
    fsysmem::ColorSpaceType::from_primitive(color_space.into_primitive())
        .ok_or_else(|| format_err!("color_space not convertible to sysmem1: {:?}", color_space))
}

pub fn images2_color_space_from_sysmem_color_space_type(
    color_space: fsysmem::ColorSpaceType,
) -> fimages2::ColorSpace {
    // This can't fail because sysmem(1) ColorSpaceType isn't flexible and all primitive values also
    // exist in images2::ColorSpace.
    fimages2::ColorSpace::from_primitive(color_space.into_primitive()).unwrap()
}

pub fn sysmem1_image_format_from_images2_image_format(
    image_format: &fimages2::ImageFormat,
) -> Result<fsysmem::ImageFormat2, Error> {
    let coded_size = image_format.size.as_ref().ok_or_else(|| format_err!("missing size"))?;
    let format_modifier = sysmem1_pixel_format_modifier_from_images2_pixel_format_modifier(
        *image_format
            .pixel_format_modifier
            .as_ref()
            .unwrap_or(&fimages2::PixelFormatModifier::Linear),
    );
    // bytes_per_row 0 is only to be used by tiled formats; not intended to be optional for
    // PixelFormatModifier::Linear
    let bytes_per_row = match format_modifier {
        fsysmem::FORMAT_MODIFIER_LINEAR => {
            // bytes_per_row required
            *image_format
                .bytes_per_row
                .as_ref()
                .ok_or_else(|| format_err!("missing bytes_per_row (required when Linear)"))?
        }
        _ => {
            // bytes_per_row can be un-set for tiled formats (un-set is preferred over 0 in images2, but we also allow 0 here)
            *image_format.bytes_per_row.as_ref().unwrap_or(&0)
        }
    };
    let display_rect = image_format.display_rect.as_ref();
    let color_space_type = sysmem1_color_space_type_from_images2_color_space(
        *image_format.color_space.as_ref().ok_or_else(|| format_err!("missing color_space"))?,
    )?;
    let pixel_aspect_ratio = image_format.pixel_aspect_ratio.as_ref();
    Ok(fsysmem::ImageFormat2 {
        pixel_format: fsysmem::PixelFormat {
            type_: sysmem1_pixel_format_type_from_images2_pixel_format(
                *image_format
                    .pixel_format
                    .as_ref()
                    .ok_or_else(|| format_err!("missing pixel_format"))?,
            )?,
            // Semantically it should be safe to always set has_format_modifier true, but we
            // preserve the "has" aspect here just in case any client code / test code is expecting
            // to see false.
            has_format_modifier: image_format.pixel_format_modifier.is_some(),
            format_modifier: fsysmem::FormatModifier { value: format_modifier },
        },
        coded_width: coded_size.width,
        coded_height: coded_size.height,
        bytes_per_row,
        display_width: display_rect.map(|rect| rect.width).unwrap_or(0),
        display_height: display_rect.map(|rect| rect.height).unwrap_or(0),
        layers: 1,
        color_space: fsysmem::ColorSpace { type_: color_space_type },
        has_pixel_aspect_ratio: pixel_aspect_ratio.is_some(),
        pixel_aspect_ratio_width: pixel_aspect_ratio.map(|size| size.width).unwrap_or(0),
        pixel_aspect_ratio_height: pixel_aspect_ratio.map(|size| size.height).unwrap_or(0),
    })
}

pub fn images2_image_format_from_sysmem_image_format(
    image_format: &fsysmem::ImageFormat2,
) -> Result<fimages2::ImageFormat, Error> {
    let pixel_format_modifier = if image_format.pixel_format.has_format_modifier {
        Some(images2_pixel_format_modifier_from_sysmem_pixel_format_modifier(
            image_format.pixel_format.format_modifier.value,
        )?)
    } else {
        None
    };
    let display_rect = if image_format.display_width != 0 || image_format.display_height != 0 {
        let display_width = if image_format.display_width != 0 {
            image_format.display_width
        } else {
            image_format.coded_width
        };
        let display_height = if image_format.display_height != 0 {
            image_format.display_height
        } else {
            image_format.coded_height
        };
        Some(RectU { x: 0, y: 0, width: display_width, height: display_height })
    } else {
        None
    };
    let pixel_aspect_ratio = if image_format.has_pixel_aspect_ratio {
        Some(SizeU {
            width: image_format.pixel_aspect_ratio_width,
            height: image_format.pixel_aspect_ratio_height,
        })
    } else {
        None
    };
    Ok(fimages2::ImageFormat {
        pixel_format: Some(images2_pixel_format_from_sysmem_pixel_format_type(
            image_format.pixel_format.type_,
        )?),
        pixel_format_modifier,
        color_space: Some(images2_color_space_from_sysmem_color_space_type(
            image_format.color_space.type_,
        )),
        size: Some(SizeU { width: image_format.coded_width, height: image_format.coded_height }),
        bytes_per_row: Some(image_format.bytes_per_row),
        display_rect,
        pixel_aspect_ratio,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_math as fmath;

    #[test]
    fn test_linear_row_bytes_2() {
        let constraints = fsysmem2::ImageFormatConstraints {
            pixel_format: Some(fimages2::PixelFormat::B8G8R8A8),
            pixel_format_modifier: Some(fimages2::PixelFormatModifier::Linear),
            min_size: Some(SizeU { width: 12, height: 12 }),
            max_size: Some(SizeU { width: 100, height: 100 }),
            bytes_per_row_divisor: Some(4 * 8),
            max_bytes_per_row: Some(100000),
            ..Default::default()
        };

        assert_eq!(image_format_minimum_row_bytes_2(&constraints, 17).unwrap(), 4 * 24);

        assert!(image_format_minimum_row_bytes_2(&constraints, 11).is_err());
        assert!(image_format_minimum_row_bytes_2(&constraints, 101).is_err());
    }

    #[test]
    fn test_linear_row_bytes() {
        let linear = fsysmem::PixelFormat {
            type_: fsysmem::PixelFormatType::Bgra32,
            has_format_modifier: true,
            format_modifier: fsysmem::FormatModifier { value: fsysmem::FORMAT_MODIFIER_LINEAR },
        };

        let constraints = fsysmem::ImageFormatConstraints {
            pixel_format: linear,
            min_coded_width: 12,
            max_coded_width: 100,
            bytes_per_row_divisor: 4 * 8,
            max_bytes_per_row: 100000,
            ..IMAGE_FORMAT_CONSTRAINTS_DEFAULT
        };

        assert_eq!(image_format_minimum_row_bytes(&constraints, 17).unwrap(), 4 * 24);

        assert!(image_format_minimum_row_bytes(&constraints, 11).is_err());
        assert!(image_format_minimum_row_bytes(&constraints, 101).is_err());
    }

    #[test]
    fn plane_byte_offset_2() {
        let constraints = fsysmem2::ImageFormatConstraints {
            pixel_format: Some(fimages2::PixelFormat::B8G8R8A8),
            pixel_format_modifier: Some(fimages2::PixelFormatModifier::Linear),
            min_size: Some(SizeU { width: 12, height: 12 }),
            max_size: Some(SizeU { width: 100, height: 100 }),
            bytes_per_row_divisor: Some(4 * 8),
            max_bytes_per_row: Some(100000),
            ..Default::default()
        };

        let image_format = constraints_to_image_format(&constraints, 18, 17).unwrap();
        // The raw size would be 72 without bytes_per_row_divisor of 32.
        assert_eq!(*image_format.bytes_per_row.as_ref().unwrap(), 96);

        assert_eq!(image_format_plane_byte_offset_2(&image_format, 0).unwrap(), 0);
        assert!(image_format_plane_byte_offset_2(&image_format, 1).is_err());

        let constraints = fsysmem2::ImageFormatConstraints {
            pixel_format: Some(fimages2::PixelFormat::I420),
            pixel_format_modifier: Some(fimages2::PixelFormatModifier::Linear),
            min_size: Some(SizeU { width: 12, height: 12 }),
            max_size: Some(SizeU { width: 100, height: 100 }),
            bytes_per_row_divisor: Some(4 * 8),
            max_bytes_per_row: Some(100000),
            ..Default::default()
        };

        const BYTES_PER_ROW: u32 = 32;
        let image_format = constraints_to_image_format(&constraints, 18, 20).unwrap();
        assert_eq!(*image_format.bytes_per_row.as_ref().unwrap(), BYTES_PER_ROW);

        assert_eq!(image_format_plane_byte_offset_2(&image_format, 0).unwrap(), 0);
        assert_eq!(image_format_plane_byte_offset_2(&image_format, 1).unwrap(), BYTES_PER_ROW * 20);
        assert_eq!(
            image_format_plane_byte_offset_2(&image_format, 2).unwrap(),
            BYTES_PER_ROW * 20 + BYTES_PER_ROW / 2 * 20 / 2
        );
        assert!(image_format_plane_byte_offset_2(&image_format, 3).is_err());

        assert_eq!(get_plane_row_bytes_2(&image_format, 0).unwrap(), BYTES_PER_ROW);
        assert_eq!(get_plane_row_bytes_2(&image_format, 1).unwrap(), BYTES_PER_ROW / 2);
        assert_eq!(get_plane_row_bytes_2(&image_format, 2).unwrap(), BYTES_PER_ROW / 2);
        assert!(get_plane_row_bytes_2(&image_format, 3).is_err())
    }

    #[test]
    fn plane_byte_offset() {
        let constraints = fsysmem2::ImageFormatConstraints {
            pixel_format: Some(fimages2::PixelFormat::B8G8R8A8),
            pixel_format_modifier: Some(fimages2::PixelFormatModifier::Linear),
            min_size: Some(SizeU { width: 12, height: 12 }),
            max_size: Some(SizeU { width: 100, height: 100 }),
            bytes_per_row_divisor: Some(4 * 8),
            max_bytes_per_row: Some(100000),
            ..Default::default()
        };

        let image_format = constraints_to_image_format(&constraints, 18, 17).unwrap();
        // The raw size would be 72 without bytes_per_row_divisor of 32.
        assert_eq!(*image_format.bytes_per_row.as_ref().unwrap(), 96);

        assert_eq!(image_format_plane_byte_offset_2(&image_format, 0).unwrap(), 0);
        assert!(image_format_plane_byte_offset_2(&image_format, 1).is_err());

        let constraints = fsysmem2::ImageFormatConstraints {
            pixel_format: Some(fimages2::PixelFormat::I420),
            pixel_format_modifier: Some(fimages2::PixelFormatModifier::Linear),
            min_size: Some(SizeU { width: 12, height: 12 }),
            max_size: Some(SizeU { width: 100, height: 100 }),
            bytes_per_row_divisor: Some(4 * 8),
            max_bytes_per_row: Some(100000),
            ..Default::default()
        };

        const BYTES_PER_ROW: u32 = 32;
        let image_format = constraints_to_image_format(&constraints, 18, 20).unwrap();
        assert_eq!(*image_format.bytes_per_row.as_ref().unwrap(), BYTES_PER_ROW);

        assert_eq!(image_format_plane_byte_offset_2(&image_format, 0).unwrap(), 0);
        assert_eq!(image_format_plane_byte_offset_2(&image_format, 1).unwrap(), BYTES_PER_ROW * 20);
        assert_eq!(
            image_format_plane_byte_offset_2(&image_format, 2).unwrap(),
            BYTES_PER_ROW * 20 + BYTES_PER_ROW / 2 * 20 / 2
        );
        assert!(image_format_plane_byte_offset_2(&image_format, 3).is_err());

        assert_eq!(get_plane_row_bytes_2(&image_format, 0).unwrap(), BYTES_PER_ROW);
        assert_eq!(get_plane_row_bytes_2(&image_format, 1).unwrap(), BYTES_PER_ROW / 2);
        assert_eq!(get_plane_row_bytes_2(&image_format, 2).unwrap(), BYTES_PER_ROW / 2);
        assert!(get_plane_row_bytes_2(&image_format, 3).is_err())
    }

    #[test]
    fn test_constraints_to_image_format() {
        // The fields set to 37 are intentionally checking that those fields are ignored by the
        // conversion, at least for now.
        let constraints = fsysmem2::ImageFormatConstraints {
            pixel_format: Some(fimages2::PixelFormat::R8G8B8),
            pixel_format_modifier: Some(fimages2::PixelFormatModifier::Linear),
            min_size: Some(SizeU { width: 12, height: 12 }),
            max_size: Some(SizeU { width: 100, height: 100 }),
            bytes_per_row_divisor: Some(4 * 8),
            max_bytes_per_row: Some(100000),
            color_spaces: Some(vec![fimages2::ColorSpace::Rec709]),
            min_bytes_per_row: Some(24),
            max_width_times_height: Some(12 * 12),
            size_alignment: Some(fmath::SizeU { width: 37, height: 1 }),
            display_rect_alignment: Some(fmath::SizeU { width: 37, height: 37 }),
            required_min_size: Some(fmath::SizeU { width: 37, height: 37 }),
            required_max_size: Some(fmath::SizeU { width: 37, height: 37 }),
            start_offset_divisor: Some(37),
            pixel_format_and_modifiers: Some(vec![fsysmem2::PixelFormatAndModifier {
                pixel_format: fimages2::PixelFormat::Yv12,
                pixel_format_modifier: fimages2::PixelFormatModifier::GoogleGoldfishOptimal,
            }]),
            require_bytes_per_row_at_pixel_boundary: Some(true),
            ..Default::default()
        };

        let image_format = constraints_to_image_format(&constraints, 18, 17).unwrap();

        assert_eq!(image_format.pixel_format, Some(fimages2::PixelFormat::R8G8B8));
        assert_eq!(image_format.pixel_format_modifier, Some(fimages2::PixelFormatModifier::Linear));
        assert_eq!(image_format.color_space, Some(fimages2::ColorSpace::Rec709));
        assert_eq!(image_format.size, Some(fmath::SizeU { width: 18, height: 17 }));
        assert_eq!(image_format.bytes_per_row, Some(96));

        // We intentionally want these to be None when we don't have any explicit info for them.
        assert_eq!(image_format.display_rect, None);
        assert_eq!(image_format.valid_size, None);
        assert_eq!(image_format.pixel_aspect_ratio, None);
    }

    #[test]
    fn test_image_format_stride_bytes_per_width_pixel_2() {
        assert_eq!(
            4,
            image_format_stride_bytes_per_width_pixel_2(fimages2::PixelFormat::R8G8B8A8).unwrap()
        );
        assert!(
            image_format_stride_bytes_per_width_pixel_2(fimages2::PixelFormat::DoNotCare).is_err()
        );
    }

    #[test]
    fn test_sysmem1_pixel_format_type_from_images2_pixel_format() {
        assert_eq!(
            fsysmem::PixelFormatType::Bgra32,
            sysmem1_pixel_format_type_from_images2_pixel_format(fimages2::PixelFormat::B8G8R8A8)
                .unwrap()
        );
        assert!(sysmem1_pixel_format_type_from_images2_pixel_format(
            fimages2::PixelFormat::from_primitive_allow_unknown(1189673091)
        )
        .is_err());
    }

    #[test]
    fn test_sysmem1_pixel_format_modifier_from_images2_pixel_format_modifier() {
        assert_eq!(
            fsysmem::FORMAT_MODIFIER_ARM_AFBC_16_X16_SPLIT_BLOCK_SPARSE_YUV_TE_TILED_HEADER,
            sysmem1_pixel_format_modifier_from_images2_pixel_format_modifier(
                fimages2::PixelFormatModifier::ArmAfbc16X16SplitBlockSparseYuvTeTiledHeader
            )
        );
        assert_eq!(
            fsysmem::FORMAT_MODIFIER_GOOGLE_GOLDFISH_OPTIMAL,
            sysmem1_pixel_format_modifier_from_images2_pixel_format_modifier(
                fimages2::PixelFormatModifier::GoogleGoldfishOptimal
            )
        );
        assert_eq!(
            fsysmem::FORMAT_MODIFIER_GOOGLE_GOLDFISH_OPTIMAL,
            sysmem1_pixel_format_modifier_from_images2_pixel_format_modifier(
                fimages2::PixelFormatModifier::from_primitive_allow_unknown(
                    fsysmem::FORMAT_MODIFIER_GOOGLE_GOLDFISH_OPTIMAL
                )
            )
        );
        // We intentionally allow too-new values to convert.
        assert_eq!(
            1189673091,
            sysmem1_pixel_format_modifier_from_images2_pixel_format_modifier(
                fimages2::PixelFormatModifier::from_primitive_allow_unknown(1189673091)
            )
        );
    }

    #[test]
    fn test_images2_pixel_format_from_sysmem_pixel_format_type() {
        assert_eq!(
            fimages2::PixelFormat::B8G8R8A8,
            images2_pixel_format_from_sysmem_pixel_format_type(fsysmem::PixelFormatType::Bgra32)
                .unwrap()
        );
        assert_eq!(
            fimages2::PixelFormat::Yv12,
            images2_pixel_format_from_sysmem_pixel_format_type(fsysmem::PixelFormatType::Yv12)
                .unwrap()
        );
    }

    #[test]
    fn test_images2_pixel_format_modifier_from_sysmem_pixel_format_modifier() {
        assert_eq!(
            fimages2::PixelFormatModifier::ArmAfbc16X16SplitBlockSparseYuvTeTiledHeader,
            images2_pixel_format_modifier_from_sysmem_pixel_format_modifier(
                fsysmem::FORMAT_MODIFIER_ARM_AFBC_16_X16_SPLIT_BLOCK_SPARSE_YUV_TE_TILED_HEADER
            )
            .unwrap()
        );
        assert_eq!(
            fimages2::PixelFormatModifier::GoogleGoldfishOptimal,
            images2_pixel_format_modifier_from_sysmem_pixel_format_modifier(
                fsysmem::FORMAT_MODIFIER_GOOGLE_GOLDFISH_OPTIMAL
            )
            .unwrap()
        );
        assert_eq!(
            fimages2::PixelFormatModifier::GoogleGoldfishOptimal,
            images2_pixel_format_modifier_from_sysmem_pixel_format_modifier(
                fimages2::PixelFormatModifier::GoogleGoldfishOptimal.into_primitive()
            )
            .unwrap()
        );
        assert!(
            images2_pixel_format_modifier_from_sysmem_pixel_format_modifier(1189673091).is_err()
        );
    }

    #[test]
    fn test_sysmem1_color_space_type_from_images2_color_space() {
        assert_eq!(
            fsysmem::ColorSpaceType::Rec709,
            sysmem1_color_space_type_from_images2_color_space(fimages2::ColorSpace::Rec709)
                .unwrap()
        );
        assert!(sysmem1_color_space_type_from_images2_color_space(
            fimages2::ColorSpace::from_primitive_allow_unknown(1189673091)
        )
        .is_err());
    }

    #[test]
    fn test_images2_color_space_from_sysmem_color_space_type() {
        assert_eq!(
            fimages2::ColorSpace::Rec709,
            images2_color_space_from_sysmem_color_space_type(fsysmem::ColorSpaceType::Rec709)
        );
        assert_eq!(
            fimages2::ColorSpace::Srgb,
            images2_color_space_from_sysmem_color_space_type(fsysmem::ColorSpaceType::Srgb)
        );
    }

    #[test]
    fn test_sysmem1_image_format_from_images2_image_format() {
        {
            let images2_format = fimages2::ImageFormat {
                pixel_format: Some(fimages2::PixelFormat::Yv12),
                pixel_format_modifier: None,
                color_space: Some(fimages2::ColorSpace::Rec709),
                size: Some(fmath::SizeU { width: 17, height: 13 }),
                bytes_per_row: Some(32),
                display_rect: Some(fmath::RectU { x: 0, y: 0, width: 15, height: 11 }),
                valid_size: Some(fmath::SizeU { width: 12, height: 10 }),
                pixel_aspect_ratio: Some(fmath::SizeU { width: 2, height: 3 }),
                ..Default::default()
            };

            let sysmem_format =
                sysmem1_image_format_from_images2_image_format(&images2_format).unwrap();

            assert_eq!(fsysmem::PixelFormatType::Yv12, sysmem_format.pixel_format.type_);
            assert_eq!(false, sysmem_format.pixel_format.has_format_modifier);
            assert_eq!(17, sysmem_format.coded_width);
            assert_eq!(13, sysmem_format.coded_height);
            assert_eq!(32, sysmem_format.bytes_per_row);
            assert_eq!(15, sysmem_format.display_width);
            assert_eq!(11, sysmem_format.display_height);
            assert_eq!(1, sysmem_format.layers);
            assert_eq!(fsysmem::ColorSpaceType::Rec709, sysmem_format.color_space.type_);
            assert_eq!(true, sysmem_format.has_pixel_aspect_ratio);
            assert_eq!(2, sysmem_format.pixel_aspect_ratio_width);
            assert_eq!(3, sysmem_format.pixel_aspect_ratio_height);
        }

        {
            let images2_format = fimages2::ImageFormat {
                pixel_format: Some(fimages2::PixelFormat::Yv12),
                pixel_format_modifier: Some(
                    fimages2::PixelFormatModifier::ArmAfbc16X16SplitBlockSparseYuvTeTiledHeader,
                ),
                color_space: Some(fimages2::ColorSpace::Rec709),
                size: Some(fmath::SizeU { width: 17, height: 13 }),
                bytes_per_row: Some(32),
                display_rect: Some(fmath::RectU { x: 0, y: 0, width: 15, height: 11 }),
                valid_size: Some(fmath::SizeU { width: 12, height: 10 }),
                pixel_aspect_ratio: Some(fmath::SizeU { width: 2, height: 3 }),
                ..Default::default()
            };

            let sysmem_format =
                sysmem1_image_format_from_images2_image_format(&images2_format).unwrap();

            assert_eq!(fsysmem::PixelFormatType::Yv12, sysmem_format.pixel_format.type_);
            assert_eq!(true, sysmem_format.pixel_format.has_format_modifier);
            assert_eq!(
                fsysmem::FORMAT_MODIFIER_ARM_AFBC_16_X16_SPLIT_BLOCK_SPARSE_YUV_TE_TILED_HEADER,
                sysmem_format.pixel_format.format_modifier.value
            );
            assert_eq!(17, sysmem_format.coded_width);
            assert_eq!(13, sysmem_format.coded_height);
            assert_eq!(32, sysmem_format.bytes_per_row);
            assert_eq!(15, sysmem_format.display_width);
            assert_eq!(11, sysmem_format.display_height);
            assert_eq!(1, sysmem_format.layers);
            assert_eq!(fsysmem::ColorSpaceType::Rec709, sysmem_format.color_space.type_);
            assert_eq!(true, sysmem_format.has_pixel_aspect_ratio);
            assert_eq!(2, sysmem_format.pixel_aspect_ratio_width);
            assert_eq!(3, sysmem_format.pixel_aspect_ratio_height);
        }
    }

    #[test]
    fn test_images2_image_format_from_sysmem_image_format() {
        let sysmem_format = fsysmem::ImageFormat2 {
            pixel_format: fsysmem::PixelFormat {
                type_: fsysmem::PixelFormatType::Yv12,
                has_format_modifier: true,
                format_modifier: fsysmem::FormatModifier {
                    value: fsysmem::FORMAT_MODIFIER_GOOGLE_GOLDFISH_OPTIMAL,
                },
            },
            coded_width: 17,
            coded_height: 13,
            bytes_per_row: 32,
            display_width: 15,
            display_height: 11,
            layers: 1,
            color_space: fsysmem::ColorSpace { type_: fsysmem::ColorSpaceType::Rec709 },
            has_pixel_aspect_ratio: true,
            pixel_aspect_ratio_width: 2,
            pixel_aspect_ratio_height: 3,
        };

        let images2_format = images2_image_format_from_sysmem_image_format(&sysmem_format).unwrap();

        assert_eq!(Some(fimages2::PixelFormat::Yv12), images2_format.pixel_format);
        assert_eq!(
            Some(fimages2::PixelFormatModifier::GoogleGoldfishOptimal),
            images2_format.pixel_format_modifier
        );
        assert_eq!(Some(fimages2::ColorSpace::Rec709), images2_format.color_space);
        assert_eq!(Some(fmath::SizeU { width: 17, height: 13 }), images2_format.size);
        assert_eq!(Some(32), images2_format.bytes_per_row);
        assert_eq!(
            Some(fmath::RectU { x: 0, y: 0, width: 15, height: 11 }),
            images2_format.display_rect
        );
        assert_eq!(None, images2_format.valid_size);
        assert_eq!(Some(fmath::SizeU { width: 2, height: 3 }), images2_format.pixel_aspect_ratio);
    }
}
