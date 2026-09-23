// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {fidl_fuchsia_images2 as fimages2, fidl_fuchsia_sysmem2 as fsysmem2};

pub fn image_format_constraints_default() -> fsysmem2::ImageFormatConstraints {
    fsysmem2::ImageFormatConstraints {
        pixel_format: Some(fimages2::PixelFormat::Nv12),
        ..Default::default()
    }
}

pub fn buffer_memory_constraints_default() -> fsysmem2::BufferMemoryConstraints {
    fsysmem2::BufferMemoryConstraints { ..Default::default() }
}

pub fn buffer_collection_constraints_default() -> fsysmem2::BufferCollectionConstraints {
    fsysmem2::BufferCollectionConstraints {
        usage: Some(fsysmem2::BufferUsage {
            cpu: Some(fsysmem2::CPU_USAGE_READ),
            video: Some(fsysmem2::VIDEO_USAGE_HW_DECODER),
            ..Default::default()
        }),
        ..Default::default()
    }
}
