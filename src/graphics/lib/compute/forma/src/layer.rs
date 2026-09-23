// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::convert::TryFrom;
use std::error::Error;
use std::iter::FromIterator;
use std::{fmt, mem};

use rustc_hash::FxHashMap;
use surpass::painter::Props;
use surpass::{self, GeomPresTransform, LinesBuilder, Path, LAYER_LIMIT};

use crate::composition::{self, CompositionId};
use crate::LayerId;

const IDENTITY: &[f32; 6] = &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

#[derive(Debug, PartialEq)]
pub enum OrderError {
    ExceededLayerLimit,
}

impl fmt::Display for OrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "exceeded layer limit ({})", LAYER_LIMIT)
    }
}

impl Error for OrderError {}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Order(u32);

impl Order {
    pub const MAX: Self = Self(LAYER_LIMIT as u32);

    pub const fn as_u32(&self) -> u32 {
        self.0
    }

    pub const fn new(order: u32) -> Result<Self, OrderError> {
        if order > Self::MAX.as_u32() {
            Err(OrderError::ExceededLayerLimit)
        } else {
            Ok(Self(order))
        }
    }
}

impl TryFrom<u32> for Order {
    type Error = OrderError;

    fn try_from(order: u32) -> Result<Self, OrderError> {
        Self::new(order)
    }
}

impl TryFrom<usize> for Order {
    type Error = OrderError;

    fn try_from(order: usize) -> Result<Self, OrderError> {
        u32::try_from(order).map_err(|_| OrderError::ExceededLayerLimit).and_then(Self::try_from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_u32_order_value() {
        let order = Order::MAX.as_u32() + 1;

        assert_eq!(Order::try_from(order), Err(OrderError::ExceededLayerLimit));
    }

    #[test]
    fn wrong_usize_order_values() {
        let order = (Order::MAX.as_u32() + 1) as usize;

        assert_eq!(Order::try_from(order), Err(OrderError::ExceededLayerLimit));

        let order = u64::MAX as usize;

        assert_eq!(Order::try_from(order), Err(OrderError::ExceededLayerLimit));
    }

    #[test]
    fn correct_order_value() {
        let order_value = Order::MAX.as_u32();
        let order = Order::try_from(order_value);

        assert_eq!(order, Ok(Order(order_value as u32)));
    }
}
