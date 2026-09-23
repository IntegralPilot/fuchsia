// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{anyhow, Error};
use {fidl_fuchsia_images2 as fimages2, fidl_fuchsia_sysmem as fsysmem, vk_sys as vk};

pub fn drm_modifier_to_sysmem_modifier_2(
    modifier: u64,
) -> Result<fimages2::PixelFormatModifier, Error> {
    match modifier {
        DRM_FORMAT_MOD_LINEAR => Ok(fimages2::PixelFormatModifier::Linear),
        I915_FORMAT_MOD_X_TILED => Ok(fimages2::PixelFormatModifier::IntelI915XTiled),
        I915_FORMAT_MOD_Y_TILED => Ok(fimages2::PixelFormatModifier::IntelI915YTiled),
        I915_FORMAT_MOD_YF_TILED => Ok(fimages2::PixelFormatModifier::IntelI915YfTiled),
        I915_FORMAT_MOD_Y_TILED_CCS => Ok(fimages2::PixelFormatModifier::IntelI915YTiledCcs),
        I915_FORMAT_MOD_YF_TILED_CCS => Ok(fimages2::PixelFormatModifier::IntelI915YfTiledCcs),
        _ => Err(anyhow!("Unsupported modifier.")),
    }
}

pub fn drm_modifier_to_sysmem_modifier(modifier: u64) -> Result<u64, Error> {
    match modifier {
        DRM_FORMAT_MOD_LINEAR => Ok(fsysmem::FORMAT_MODIFIER_LINEAR),
        I915_FORMAT_MOD_X_TILED => Ok(fsysmem::FORMAT_MODIFIER_INTEL_I915_X_TILED),
        I915_FORMAT_MOD_Y_TILED => Ok(fsysmem::FORMAT_MODIFIER_INTEL_I915_Y_TILED),
        I915_FORMAT_MOD_YF_TILED => Ok(fsysmem::FORMAT_MODIFIER_INTEL_I915_YF_TILED),
        I915_FORMAT_MOD_Y_TILED_CCS => Ok(fsysmem::FORMAT_MODIFIER_INTEL_I915_Y_TILED_CCS),
        I915_FORMAT_MOD_YF_TILED_CCS => Ok(fsysmem::FORMAT_MODIFIER_INTEL_I915_YF_TILED_CCS),
        _ => Err(anyhow!("Unsupported modifier.")),
    }
}

pub fn min_bytes_per_row(drm_format: u32, width: u32) -> Result<u32, Error> {
    match drm_format {
        format
            if format == DRM_FORMAT_ARGB8888
                || format == DRM_FORMAT_XRGB8888
                || format == DRM_FORMAT_ABGR8888
                || format == DRM_FORMAT_XBGR8888 =>
        {
            Ok(width * 4)
        }
        _ => Err(anyhow!("Unsupported format.")),
    }
}

pub fn drm_format_to_vulkan_format(drm_format: u32) -> Result<u32, Error> {
    match drm_format {
        DRM_FORMAT_ARGB8888 | DRM_FORMAT_XRGB8888 => Ok(vk::FORMAT_B8G8R8A8_UNORM),
        DRM_FORMAT_ABGR8888 | DRM_FORMAT_XBGR8888 => Ok(vk::FORMAT_R8G8B8A8_UNORM),
        DRM_FORMAT_RGB565 => Ok(vk::FORMAT_R5G6B5_UNORM_PACK16),
        _ => Err(anyhow!("Unsupported format.")),
    }
}

pub fn drm_format_to_sysmem_format(drm_format: u32) -> Result<fsysmem::PixelFormatType, Error> {
    match drm_format {
        DRM_FORMAT_ARGB8888 | DRM_FORMAT_XRGB8888 => Ok(fsysmem::PixelFormatType::Bgra32),
        DRM_FORMAT_ABGR8888 | DRM_FORMAT_XBGR8888 => Ok(fsysmem::PixelFormatType::R8G8B8A8),
        DRM_FORMAT_RGB565 => Ok(fsysmem::PixelFormatType::Rgb565),
        _ => Err(anyhow!("Unsupported format.")),
    }
}

pub fn drm_format_to_images2_format(drm_format: u32) -> Result<fimages2::PixelFormat, Error> {
    match drm_format {
        DRM_FORMAT_ARGB8888 | DRM_FORMAT_XRGB8888 => Ok(fimages2::PixelFormat::B8G8R8A8),
        DRM_FORMAT_ABGR8888 | DRM_FORMAT_XBGR8888 => Ok(fimages2::PixelFormat::R8G8B8A8),
        DRM_FORMAT_RGB565 => Ok(fimages2::PixelFormat::R5G6B5),
        _ => Err(anyhow!("Unsupported format.")),
    }
}

pub fn sysmem_modifier_to_drm_modifier_2(
    modifier: fimages2::PixelFormatModifier,
) -> Result<u64, Error> {
    match modifier {
        fimages2::PixelFormatModifier::Linear => Ok(DRM_FORMAT_MOD_LINEAR),
        fimages2::PixelFormatModifier::IntelI915XTiled => Ok(I915_FORMAT_MOD_X_TILED),
        fimages2::PixelFormatModifier::IntelI915YTiled => Ok(I915_FORMAT_MOD_Y_TILED),
        fimages2::PixelFormatModifier::IntelI915YfTiled => Ok(I915_FORMAT_MOD_YF_TILED),
        fimages2::PixelFormatModifier::IntelI915YTiledCcs => Ok(I915_FORMAT_MOD_Y_TILED_CCS),
        fimages2::PixelFormatModifier::IntelI915YfTiledCcs => Ok(I915_FORMAT_MOD_YF_TILED_CCS),
        _ => Err(anyhow!("Unsupported modifier.")),
    }
}

pub fn sysmem_modifier_to_drm_modifier(modifier: u64) -> Result<u64, Error> {
    match modifier {
        fsysmem::FORMAT_MODIFIER_LINEAR => Ok(DRM_FORMAT_MOD_LINEAR),
        fsysmem::FORMAT_MODIFIER_INTEL_I915_X_TILED => Ok(I915_FORMAT_MOD_X_TILED),
        fsysmem::FORMAT_MODIFIER_INTEL_I915_Y_TILED => Ok(I915_FORMAT_MOD_Y_TILED),
        fsysmem::FORMAT_MODIFIER_INTEL_I915_YF_TILED => Ok(I915_FORMAT_MOD_YF_TILED),
        fsysmem::FORMAT_MODIFIER_INTEL_I915_Y_TILED_CCS => Ok(I915_FORMAT_MOD_Y_TILED_CCS),
        fsysmem::FORMAT_MODIFIER_INTEL_I915_YF_TILED_CCS => Ok(I915_FORMAT_MOD_YF_TILED_CCS),
        _ => Err(anyhow!("Unsupported modifier.")),
    }
}

const fn fourcc_code(a: u8, b: u8, c: u8, d: u8) -> u32 {
    let a32 = a as u32;
    let b32 = (b as u32) << 8;
    let c32 = (c as u32) << 16;
    let d32 = (d as u32) << 24;
    a32 | b32 | c32 | d32
}

const fn fourcc_mod_code(vendor: u64, value: u64) -> u64 {
    vendor << 56 | (value & 0x00ffffffffffffff)
}

pub const DRM_FORMAT_MOD_VENDOR_NONE: u64 = 0;
pub const DRM_FORMAT_MOD_VENDOR_INTEL: u64 = 0x01;

pub const DRM_FORMAT_MOD_INVALID: u64 =
    fourcc_mod_code(DRM_FORMAT_MOD_VENDOR_NONE, DRM_FORMAT_MOD_RESERVED);

pub const DRM_FORMAT_MOD_RESERVED: u64 = (1 << 56) - 1;
pub const DRM_FORMAT_MOD_LINEAR: u64 = fourcc_mod_code(DRM_FORMAT_MOD_VENDOR_NONE, 0);
pub const I915_FORMAT_MOD_X_TILED: u64 = fourcc_mod_code(DRM_FORMAT_MOD_VENDOR_INTEL, 1);
pub const I915_FORMAT_MOD_Y_TILED: u64 = fourcc_mod_code(DRM_FORMAT_MOD_VENDOR_INTEL, 2);
pub const I915_FORMAT_MOD_YF_TILED: u64 = fourcc_mod_code(DRM_FORMAT_MOD_VENDOR_INTEL, 3);
pub const I915_FORMAT_MOD_Y_TILED_CCS: u64 = fourcc_mod_code(DRM_FORMAT_MOD_VENDOR_INTEL, 4);
pub const I915_FORMAT_MOD_YF_TILED_CCS: u64 = fourcc_mod_code(DRM_FORMAT_MOD_VENDOR_INTEL, 5);

pub const DRM_FORMAT_XRGB8888: u32 = fourcc_code(b'X', b'R', b'2', b'4');
pub const DRM_FORMAT_XBGR8888: u32 = fourcc_code(b'X', b'B', b'2', b'4');
pub const DRM_FORMAT_ARGB8888: u32 = fourcc_code(b'A', b'R', b'2', b'4');
pub const DRM_FORMAT_ABGR8888: u32 = fourcc_code(b'A', b'B', b'2', b'4');
pub const DRM_FORMAT_RGB565: u32 = fourcc_code(b'R', b'G', b'1', b'6');
