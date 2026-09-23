// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_sysmem2::*;

pub fn buffer_collection_constraints_default() -> BufferCollectionConstraints {
    BufferCollectionConstraints {
        // Set `video` usage even though we aren't doing video, as otherwise sysmem complains about not having it set.
        usage: Some(BufferUsage {
            cpu: Some(CPU_USAGE_READ),
            video: Some(VIDEO_USAGE_HW_DECODER),
            ..Default::default()
        }),
        // Indicate we want at least one buffer available on our end at all times.
        min_buffer_count_for_camping: Some(1),
        ..Default::default()
    }
}
