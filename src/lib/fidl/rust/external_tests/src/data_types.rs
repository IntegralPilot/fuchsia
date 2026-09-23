// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This file tests the public APIs of FIDL data types.

use assert_matches::assert_matches;
use fidl_test_external::{
    FlexibleAnimal, FlexibleButtons, FlexibleEmptyEnum, FlexibleResourceThing, FlexibleValueThing,
    ResourceRecord, StrictAnimal, StrictButtons, StrictResourceThing, StrictValueThing,
    ValueRecord,
};

#[test]
fn strict_bits() {
    assert_eq!(StrictButtons::empty(), StrictButtons::default());
    assert_eq!(StrictButtons::from_bits(0b001), Some(StrictButtons::PLAY));
    assert_eq!(StrictButtons::from_bits(0b1100), None);
    assert_eq!(StrictButtons::from_bits_truncate(0b1010), StrictButtons::PAUSE);
    assert_eq!(StrictButtons::from_bits_truncate(u32::MAX), StrictButtons::all());
    assert_eq!(StrictButtons::STOP.bits(), 0b100);
}

#[test]
fn flexible_bits() {
    assert_eq!(FlexibleButtons::empty(), FlexibleButtons::default());
    assert_eq!(FlexibleButtons::from_bits(0b001), Some(FlexibleButtons::PLAY));
    assert_eq!(FlexibleButtons::from_bits(0b1100), None);
    assert_eq!(
        FlexibleButtons::from_bits_allow_unknown(0b1010) & FlexibleButtons::all(),
        FlexibleButtons::PAUSE
    );
    assert_eq!(FlexibleButtons::STOP.bits(), 0b100);
    assert_eq!(FlexibleButtons::from_bits_allow_unknown(0b1010).bits(), 0b1010);
    assert_eq!(FlexibleButtons::from_bits_allow_unknown(u32::MAX).bits(), u32::MAX);

    assert_eq!(FlexibleButtons::PLAY.has_unknown_bits(), false);
    assert_eq!(FlexibleButtons::PLAY.get_unknown_bits(), 0);
    assert_eq!(FlexibleButtons::from_bits_allow_unknown(0b1010).has_unknown_bits(), true);
    assert_eq!(FlexibleButtons::from_bits_allow_unknown(0b1010).get_unknown_bits(), 0b1000);
    assert_eq!(FlexibleButtons::from_bits_allow_unknown(u32::MAX).has_unknown_bits(), true);
    assert_eq!(
        FlexibleButtons::from_bits_allow_unknown(u32::MAX).get_unknown_bits(),
        u32::MAX & !0b111
    );

    // Negation ANDs with the mask.
    assert_ne!(
        FlexibleButtons::from_bits_allow_unknown(0b101000101),
        FlexibleButtons::PLAY | FlexibleButtons::STOP
    );
    assert_eq!(!FlexibleButtons::from_bits_allow_unknown(0b101000101), FlexibleButtons::PAUSE);
    assert_eq!(
        !!FlexibleButtons::from_bits_allow_unknown(0b101000101),
        FlexibleButtons::PLAY | FlexibleButtons::STOP
    );
}

#[test]
fn strict_enum() {
    assert_eq!(StrictAnimal::from_primitive(0), Some(StrictAnimal::Dog));
    assert_eq!(StrictAnimal::from_primitive(3), None);
    assert_eq!(StrictAnimal::Cat.into_primitive(), 1);
}

#[test]
fn flexible_enum() {
    assert_eq!(FlexibleAnimal::from_primitive(0), Some(FlexibleAnimal::Dog));
    assert_eq!(FlexibleAnimal::from_primitive(3), None);
    assert_eq!(FlexibleAnimal::from_primitive_allow_unknown(0), FlexibleAnimal::Dog);

    let unknown3 = FlexibleAnimal::__SourceBreaking { unknown_ordinal: 3 };
    assert_eq!(FlexibleAnimal::from_primitive_allow_unknown(3), unknown3);

    assert_eq!(FlexibleAnimal::Cat.into_primitive(), 1);
    assert_eq!(FlexibleAnimal::from_primitive_allow_unknown(3).into_primitive(), 3);
    assert_eq!(FlexibleAnimal::unknown().into_primitive(), i32::MAX);

    assert_eq!(FlexibleAnimal::Cat.is_unknown(), false);
    assert_eq!(FlexibleAnimal::from_primitive_allow_unknown(3).is_unknown(), true);
    assert_eq!(FlexibleAnimal::unknown().is_unknown(), true);
}

#[test]
fn flexible_empty_enum() {
    assert_eq!(FlexibleEmptyEnum::from_primitive(3), None);

    let unknown3 = FlexibleEmptyEnum::__SourceBreaking { unknown_ordinal: 3 };
    assert_eq!(FlexibleEmptyEnum::from_primitive_allow_unknown(3), unknown3);

    assert_eq!(FlexibleEmptyEnum::unknown().into_primitive(), i32::MAX);

    assert_eq!(FlexibleEmptyEnum::from_primitive_allow_unknown(3).is_unknown(), true);
    assert_eq!(FlexibleEmptyEnum::unknown().is_unknown(), true);
}

#[test]
fn strict_value_union() {
    assert_eq!(StrictValueThing::Number(42).ordinal(), 1);
    assert_eq!(StrictValueThing::Name("hello".to_owned()).ordinal(), 2);
}

#[test]
fn flexible_value_union() {
    let number = FlexibleValueThing::Number(42);
    let name = FlexibleValueThing::Name("hello".to_owned());
    let unknown = FlexibleValueThing::unknown_variant_for_testing();

    assert_eq!(number.is_unknown(), false);
    assert_eq!(name.is_unknown(), false);
    assert_eq!(unknown.is_unknown(), true);

    assert_eq!(number.ordinal(), 1);
    assert_eq!(name.ordinal(), 2);
    assert_eq!(unknown.ordinal(), 0);

    assert_eq!(number, number);
    assert_eq!(name, name);
    assert_ne!(number, name);
    assert_ne!(name, number);

    // Unknowns are like NaN, not equal to anything, including themselves.
    assert_ne!(unknown, unknown);
    assert_ne!(number, unknown);
}

#[test]
fn strict_resource_union() {
    assert_eq!(StrictResourceThing::Number(42).ordinal(), 1);
    assert_eq!(StrictResourceThing::Name("hello".to_owned()).ordinal(), 2);
}

#[test]
fn flexible_resource_union() {
    assert_eq!(FlexibleResourceThing::Number(42).is_unknown(), false);
    assert_eq!(FlexibleResourceThing::Name("hello".to_owned()).is_unknown(), false);
    assert_eq!(FlexibleResourceThing::Number(42).ordinal(), 1);
    assert_eq!(FlexibleResourceThing::Name("hello".to_owned()).ordinal(), 2);
    assert_eq!(FlexibleResourceThing::unknown_variant_for_testing().is_unknown(), true);
    assert_eq!(FlexibleResourceThing::unknown_variant_for_testing().ordinal(), 0);
}

#[test]
fn value_table() {
    assert_matches!(ValueRecord::default(), ValueRecord { name: None, age: None, .. });

    let table = ValueRecord { age: Some(30), ..Default::default() };
    assert_eq!(table.name, None);
    assert_eq!(table.age, Some(30));

    let ValueRecord { name, .. } = table;
    assert_eq!(name, None);
    let ValueRecord { age, .. } = table;
    assert_eq!(age, Some(30));
}

#[test]
fn resource_table() {
    assert_matches!(ResourceRecord::default(), ResourceRecord { name: None, age: None, .. });

    let table = ResourceRecord { age: Some(30), ..Default::default() };
    assert_eq!(table.name, None);
    assert_eq!(table.age, Some(30));

    let ResourceRecord { name, .. } = table;
    assert_eq!(name, None);
    let ResourceRecord { age, .. } = table;
    assert_eq!(age, Some(30));
}
