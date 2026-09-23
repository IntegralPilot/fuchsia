// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::private::InspectTypeInternal;
use crate::writer::{Error, InnerType, State};
use inspect_format::BlockIndex;

/// Trait implemented by properties.
pub trait Property<'t>: InspectTypeInternal {
    /// The type of the property.
    type Type;

    /// Set the property value to |value|.
    fn set(&self, value: Self::Type);

    /// Takes a function to execute as under a single lock of the Inspect VMO. This function
    /// receives a reference to the `Property` on which it is called.
    fn atomic_update<R, F: FnOnce(&Self) -> R>(&self, update_fn: F) -> R {
        self.atomic_access(update_fn)
    }
}

/// Trait implemented by numeric properties providing common operations.
pub trait NumericProperty<'t>: Property<'t> {
    /// Add the given |value| to the property current value.
    fn add(&self, value: <Self as Property<'t>>::Type) -> Option<<Self as Property<'t>>::Type>;

    /// Subtract the given |value| from the property current value.
    fn subtract(&self, value: <Self as Property<'t>>::Type)
    -> Option<<Self as Property<'t>>::Type>;
}

/// Get the usable length of a type.
pub trait Length {
    fn len(&self) -> Option<usize>;
    fn is_empty(&self) -> Option<bool> {
        self.len().map(|s| s == 0)
    }
}

impl<T: ArrayProperty + InspectTypeInternal> Length for T {
    fn len(&self) -> Option<usize> {
        if let Ok(state) = self.state()?.try_lock() {
            return Some(state.get_array_size(self.block_index()?));
        }
        None
    }
}

/// Trait implemented by all array properties providing common operations on arrays.
pub trait ArrayProperty: Length + InspectTypeInternal {
    /// The type of the array entries.
    type Type<'a>
    where
        Self: 'a;

    /// Sets the array value to `value` at the given `index`.
    fn set<'a>(&self, index: usize, value: impl Into<Self::Type<'a>>)
    where
        Self: 'a;

    /// Sets all slots of the array to 0 and releases any references.
    fn clear(&self);

    /// Takes a function to execute as under a single lock of the Inspect VMO. This function
    /// receives a reference to the `ArrayProperty` on which it is called.
    fn atomic_update<R, F: FnOnce(&Self) -> R>(&self, update_fn: F) -> R {
        self.atomic_access(update_fn)
    }
}

pub trait ArithmeticArrayProperty: ArrayProperty {
    /// Adds the given `value` to the property current value at the given `index`.
    fn add<'a>(&self, index: usize, value: Self::Type<'a>)
    where
        Self: 'a;

    /// Subtracts the given `value` to the property current value at the given `index`.
    fn subtract<'a>(&self, index: usize, value: Self::Type<'a>)
    where
        Self: 'a;
}

/// Trait implemented by all histogram properties providing common operations.
pub trait HistogramProperty {
    /// The type of each value added to the histogram.
    type Type;

    /// Inserts the given `value` in the histogram.
    fn insert(&self, value: Self::Type);

    /// Inserts the given `value` in the histogram `count` times.
    fn insert_multiple(&self, value: Self::Type, count: usize);

    /// Clears all buckets of the histogram.
    fn clear(&self);
}

#[derive(Default, Debug)]
pub(crate) struct InnerPropertyType;

impl InnerType for InnerPropertyType {
    type Data = ();
    fn free(state: &State, _: &Self::Data, block_index: BlockIndex) -> Result<(), Error> {
        let mut state_lock = state.try_lock()?;
        state_lock
            .free_string_or_bytes_buffer_property(block_index)
            .map_err(|err| Error::free("property", block_index, err))
    }
}
