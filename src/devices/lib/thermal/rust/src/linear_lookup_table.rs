// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Error;

pub struct LookupTableEntry {
    pub x: f32,
    pub y: f32,
}

pub struct LinearLookupTable {
    table: Vec<LookupTableEntry>,
    y_decreasing: bool,
}

impl LinearLookupTable {
    pub fn new(mut table: Vec<LookupTableEntry>) -> Result<Self, Error> {
        if table.is_empty() {
            return Err(Error::EmptyProfile);
        }
        if table.iter().any(|entry| entry.x.is_nan() || entry.y.is_nan()) {
            return Err(Error::ProfileContainsNan);
        }
        // Sort profile table descending by x to ensure proper lookup
        table.sort_by(|a, b| b.x.partial_cmp(&a.x).unwrap());
        if table.windows(2).any(|w| w[0].x == w[1].x) {
            return Err(Error::ProfileContainsDuplicateKeys);
        }
        let is_increasing = table.windows(2).all(|w| w[0].y < w[1].y);
        let is_decreasing = table.windows(2).all(|w| w[0].y > w[1].y);
        if !is_increasing && !is_decreasing {
            return Err(Error::ProfileNotMonotonic);
        }
        let y_decreasing = is_decreasing;
        Ok(Self { table, y_decreasing })
    }

    // Always returns float because it may be linearly interpolated. x-axis is always decreasing.
    pub fn lookup_y(&self, x: f32) -> Result<f32, Error> {
        self.lookup(x, true, |entry| entry.x, |entry| entry.y)
    }

    // Always returns float because it may be linearly interpolated.
    pub fn lookup_x(&self, y: f32) -> Result<f32, Error> {
        self.lookup(y, self.y_decreasing, |entry| entry.y, |entry| entry.x)
    }

    fn lookup<FRef, FLook>(
        &self,
        reference_value: f32,
        decreasing: bool,
        ref_fn: FRef,
        look_fn: FLook,
    ) -> Result<f32, Error>
    where
        FRef: Fn(&LookupTableEntry) -> f32,
        FLook: Fn(&LookupTableEntry) -> f32,
    {
        if self.table.is_empty() {
            return Err(Error::EmptyProfile);
        }

        let min_idx = if decreasing { 0 } else { self.table.len() - 1 };
        let max_idx = if decreasing { self.table.len() - 1 } else { 0 };

        if reference_value >= ref_fn(&self.table[min_idx]) {
            return Ok(look_fn(&self.table[min_idx]));
        }
        if reference_value <= ref_fn(&self.table[max_idx]) {
            return Ok(look_fn(&self.table[max_idx]));
        }

        // Binary search for lower bound
        let mut low = 1;
        let mut high = self.table.len() - 1;
        let mut idx = 1;
        while low <= high {
            let mid = low + (high - low) / 2;
            let val = ref_fn(&self.table[mid]);
            if decreasing {
                if reference_value >= val {
                    idx = mid;
                    high = mid - 1;
                } else {
                    low = mid + 1;
                }
            } else {
                if reference_value <= val {
                    idx = mid;
                    high = mid - 1;
                } else {
                    low = mid + 1;
                }
            }
        }

        let x1 = ref_fn(&self.table[idx]);
        let y1 = look_fn(&self.table[idx]);
        let x2 = ref_fn(&self.table[idx - 1]);
        let y2 = look_fn(&self.table[idx - 1]);

        let scale = (reference_value - x1) / (x2 - x1);
        Ok(scale * (y2 - y1) + y1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! assert_float_eq {
        ($left:expr, $right:expr $(,)? ) => {
            let left = $left;
            let right = $right;
            assert!(
                (left - right).abs() < 1e-5,
                "assertion failed: `(left == right)`\n  left: `{:?}`,\n right: `{:?}`",
                left,
                right
            );
        };
    }

    fn get_ascending() -> LinearLookupTable {
        LinearLookupTable::new(vec![
            LookupTableEntry { x: 2.0, y: 10.0 },
            LookupTableEntry { x: 4.0, y: 8.0 },
            LookupTableEntry { x: 6.0, y: 6.0 },
            LookupTableEntry { x: 8.0, y: 4.0 },
        ])
        .unwrap()
    }

    fn get_descending() -> LinearLookupTable {
        LinearLookupTable::new(vec![
            LookupTableEntry { x: 2.0, y: 6.0 },
            LookupTableEntry { x: 4.0, y: 8.0 },
            LookupTableEntry { x: 6.0, y: 10.0 },
        ])
        .unwrap()
    }

    #[test]
    fn test_ascending_lookup_y() {
        let lut = get_ascending();
        assert_float_eq!(lut.lookup_y(3.0).unwrap(), 9.0);
    }

    #[test]
    fn test_ascending_lookup_x() {
        let lut = get_ascending();
        assert_float_eq!(lut.lookup_x(5.0).unwrap(), 7.0);
    }

    #[test]
    fn test_descending_lookup_y() {
        let lut = get_descending();
        assert_float_eq!(lut.lookup_y(3.0).unwrap(), 7.0);
    }

    #[test]
    fn test_descending_lookup_x() {
        let lut = get_descending();
        assert_float_eq!(lut.lookup_x(9.0).unwrap(), 5.0);
    }

    #[test]
    fn test_out_of_range() {
        let lut = get_ascending();
        assert_float_eq!(lut.lookup_y(1.0).unwrap(), 10.0);
        assert_float_eq!(lut.lookup_y(9.0).unwrap(), 4.0);
        assert_float_eq!(lut.lookup_x(11.0).unwrap(), 2.0);
        assert_float_eq!(lut.lookup_x(3.0).unwrap(), 8.0);
    }

    #[test]
    fn test_nan_profile() {
        let result = LinearLookupTable::new(vec![
            LookupTableEntry { x: 2.0, y: f32::NAN },
            LookupTableEntry { x: 4.0, y: 8.0 },
        ]);
        assert_eq!(result.err(), Some(Error::ProfileContainsNan));

        let result = LinearLookupTable::new(vec![
            LookupTableEntry { x: f32::NAN, y: 10.0 },
            LookupTableEntry { x: 4.0, y: 8.0 },
        ]);
        assert_eq!(result.err(), Some(Error::ProfileContainsNan));
    }

    #[test]
    fn test_duplicate_keys_profile() {
        let result = LinearLookupTable::new(vec![
            LookupTableEntry { x: 2.0, y: 10.0 },
            LookupTableEntry { x: 2.0, y: 8.0 },
        ]);
        assert_eq!(result.err(), Some(Error::ProfileContainsDuplicateKeys));
    }

    #[test]
    fn test_non_monotonic_y_profile() {
        let result = LinearLookupTable::new(vec![
            LookupTableEntry { x: 2.0, y: 10.0 },
            LookupTableEntry { x: 4.0, y: 12.0 },
            LookupTableEntry { x: 6.0, y: 8.0 },
        ]);
        assert_eq!(result.err(), Some(Error::ProfileNotMonotonic));
    }
}
