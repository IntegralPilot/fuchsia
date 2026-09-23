// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_ir::TypeShape;

pub trait TypeShapeExt {
    fn is_static(&self) -> bool;
}

impl TypeShapeExt for TypeShape {
    fn is_static(&self) -> bool {
        self.max_out_of_line == 0 && !self.has_flexible_envelope
    }
}
