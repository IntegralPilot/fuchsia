// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::hash::Hash;
use core::marker::PhantomData;
use std::collections::HashMap;

use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

pub trait Index {
    type Key: Clone + Hash + Eq;

    fn key(&self) -> &Self::Key;
}

pub fn index<'de, D, T>(deserializer: D) -> Result<HashMap<T::Key, T>, D::Error>
where
    D: Deserializer<'de>,
    T: Index + Deserialize<'de>,
{
    struct IndexVisitor<T> {
        marker: PhantomData<T>,
    }

    impl<'de, T> Visitor<'de> for IndexVisitor<T>
    where
        T: Index + Deserialize<'de>,
    {
        type Value = HashMap<T::Key, T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a sequence")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut values = HashMap::new();

            while let Some(value) = seq.next_element::<T>()? {
                values.insert(value.key().clone(), value);
            }

            Ok(values)
        }
    }

    let visitor = IndexVisitor { marker: PhantomData };
    deserializer.deserialize_seq(visitor)
}
