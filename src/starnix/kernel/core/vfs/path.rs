// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use starnix_types::string::{FsStr, FsString};

pub const SEPARATOR: u8 = b'/';

// Helper that can be used to build paths backwards, from the tail to head.
pub struct PathBuilder {
    // The path is kept in `data[pos..]`.
    data: FsString,
    pos: usize,
}

impl PathBuilder {
    const INITIAL_CAPACITY: usize = 32;

    pub fn new() -> Self {
        Self { data: Default::default(), pos: 0 }
    }

    pub fn prepend_element(&mut self, element: &FsStr) {
        self.ensure_capacity(element.len() + 1);
        let old_pos = self.pos;
        self.pos -= element.len() + 1;
        self.data[self.pos + 1..old_pos].copy_from_slice(element);
        self.data[self.pos] = b'/';
    }

    /// Build the absolute path string.
    pub fn build_absolute(mut self) -> FsString {
        if self.pos == self.data.len() {
            return "/".into();
        }
        self.data.drain(..self.pos);
        self.data
    }

    /// Build the relative path string.
    pub fn build_relative(self) -> FsString {
        let mut absolute = self.build_absolute();
        // Remove the prefix slash.
        absolute.remove(0);
        absolute
    }

    fn ensure_capacity(&mut self, capacity_needed: usize) {
        if capacity_needed > self.pos {
            let current_size = self.data.len();
            let len = current_size - self.pos;
            let min_size = len + capacity_needed;
            let mut new_size = std::cmp::max(current_size * 2, Self::INITIAL_CAPACITY);
            while new_size < min_size {
                new_size *= 2;
            }
            self.data.reserve(new_size - current_size);
            self.data.resize(new_size - len, 0);
            self.data.extend_from_within(self.pos..(self.pos + len));
            self.pos = new_size - len;
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[::fuchsia::test]
    fn test_path_builder() {
        let p = PathBuilder::new();
        assert_eq!(p.build_absolute(), "/");

        let p = PathBuilder::new();
        assert_eq!(p.build_relative(), "");

        let mut p = PathBuilder::new();
        p.prepend_element("foo".into());
        assert_eq!(p.build_absolute(), "/foo");

        let mut p = PathBuilder::new();
        p.prepend_element("foo".into());
        assert_eq!(p.build_relative(), "foo");

        let mut p = PathBuilder::new();
        p.prepend_element("foo".into());
        p.prepend_element("bar".into());
        assert_eq!(p.build_absolute(), "/bar/foo");

        let mut p = PathBuilder::new();
        p.prepend_element("foo".into());
        p.prepend_element("1234567890123456789012345678901234567890".into());
        p.prepend_element("bar".into());
        assert_eq!(p.build_absolute(), "/bar/1234567890123456789012345678901234567890/foo");
    }
}
