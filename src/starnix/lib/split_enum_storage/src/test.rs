// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::rc::Rc;

#[derive(Debug, Eq, PartialEq, Clone, split_enum_storage::SplitStorage)]
enum DataEnum {
    None,
    VariantWithAlloc(Rc<u8>),
}

#[split_enum_storage::container]
struct Container {
    #[split_enum_storage::decomposed]
    data_enum: DataEnum,
}

#[::fuchsia::test]
fn test_set_drops_old_payload() {
    let alloc = Rc::new(1);
    let weak = Rc::downgrade(&alloc);
    let mut container =
        ContainerUnsplit { data_enum: DataEnum::VariantWithAlloc(alloc.clone()) }.decompose();
    assert_eq!(container.data_enum(), DataEnumRef::VariantWithAlloc(&alloc));
    assert!(container.data_enum().is_variantwithalloc());

    // Test PartialEq between owned and ref
    assert_eq!(container.data_enum(), DataEnum::VariantWithAlloc(alloc.clone()));
    assert_eq!(DataEnum::VariantWithAlloc(alloc.clone()), container.data_enum());

    // This consumes `alloc`.
    assert_eq!(container.data_enum().to_owned(), DataEnum::VariantWithAlloc(alloc));

    container.set_data_enum(DataEnum::None);

    assert!(weak.upgrade().is_none());
}
