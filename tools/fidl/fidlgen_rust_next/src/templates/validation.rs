// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_ir::{Type, TypeKind};

pub fn constraint_for(ty: &Type) -> Option<String> {
    match &ty.kind {
        TypeKind::Vector { element_count, element_type, .. } => {
            let member_constraint =
                constraint_for(element_type).unwrap_or_else(|| "()".to_string());
            let element_count = element_count.unwrap_or(u32::MAX);
            Some(format!("({element_count}, {member_constraint})"))
        }

        TypeKind::String { element_count, .. } => {
            let element_count = element_count.unwrap_or(u32::MAX);
            Some(format!("{element_count}"))
        }
        TypeKind::Array { element_type, .. } => constraint_for(element_type.as_ref()),
        _ => None,
    }
}
