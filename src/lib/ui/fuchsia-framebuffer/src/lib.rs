// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeSet;

pub mod sysmem;

#[cfg(test)]
use std::ops::Range;

// TODO(https://fxbug.dev/42080389): Use display_utils structs instead.
pub type ImageId = u64;

/// A pool of reusable framebuffer image IDs allocated for a buffer collection.
#[derive(Debug)]
pub struct FrameSet {
    image_count: usize,
    available: BTreeSet<ImageId>,
    taken: BTreeSet<ImageId>,
}

impl FrameSet {
    pub fn new(available: BTreeSet<ImageId>) -> FrameSet {
        FrameSet { image_count: available.len(), available, taken: BTreeSet::new() }
    }

    #[cfg(test)]
    pub fn new_with_range(r: Range<ImageId>) -> FrameSet {
        let mut available = BTreeSet::new();
        for image_id in r {
            available.insert(image_id);
        }
        Self::new(available)
    }

    /// Removes and returns the first available image ID, or `None` if empty.
    pub fn take_image(&mut self) -> Option<ImageId> {
        if let Some(first) = self.available.pop_first() {
            self.taken.insert(first);
            Some(first)
        } else {
            None
        }
    }

    /// Inserts the returned image ID back into `self.available`.
    ///
    /// `image_id` must be one of the IDs previously returned by `take_image`.
    pub fn return_image(&mut self, image_id: ImageId) {
        assert!(
            self.taken.remove(&image_id),
            "Attempted to return image {} which was not currently taken",
            image_id
        );
        self.available.insert(image_id);
    }

    /// Returns `true` if all images allocated for this pool are available.
    pub fn no_images_in_use(&self) -> bool {
        self.taken.is_empty()
    }

    /// Returns the total number of images in the pool.
    pub fn image_count_for_testing(&self) -> usize {
        self.image_count
    }

    /// Returns the number of currently available images in the pool.
    pub fn available_count_for_testing(&self) -> usize {
        self.available.len()
    }
}

#[cfg(test)]
mod frameset_tests {
    use crate::{FrameSet, ImageId};
    use std::collections::BTreeSet;
    use std::ops::Range;

    const IMAGE_RANGE: Range<ImageId> = 200..203;

    #[fuchsia::test]
    fn test_initial_state() {
        let fs = FrameSet::new_with_range(IMAGE_RANGE);
        assert_eq!(fs.image_count_for_testing(), 3);
        assert_eq!(fs.available_count_for_testing(), 3);
        assert!(fs.no_images_in_use());
    }

    #[fuchsia::test]
    fn test_take_and_return_image() {
        let mut fs = FrameSet::new_with_range(IMAGE_RANGE);

        let img1 = fs.take_image();
        assert_eq!(img1, Some(200));
        assert_eq!(fs.available_count_for_testing(), 2);
        assert!(!fs.no_images_in_use());

        let img2 = fs.take_image();
        assert_eq!(img2, Some(201));
        assert_eq!(fs.available_count_for_testing(), 1);

        fs.return_image(img1.unwrap());
        assert_eq!(fs.available_count_for_testing(), 2);
        assert!(!fs.no_images_in_use());

        fs.return_image(img2.unwrap());
        assert_eq!(fs.available_count_for_testing(), 3);
        assert!(fs.no_images_in_use());
    }

    #[fuchsia::test]
    #[should_panic(expected = "Attempted to return image 100 which was not currently taken")]
    fn test_return_untracked_image_panics() {
        let mut fs = FrameSet::new_with_range(IMAGE_RANGE);
        fs.return_image(100);
    }

    #[fuchsia::test]
    #[should_panic(expected = "Attempted to return image 200 which was not currently taken")]
    fn test_return_already_available_image_panics() {
        let mut fs = FrameSet::new_with_range(IMAGE_RANGE);
        // Image 200 is available, not taken. Returning it should panic.
        fs.return_image(200);
    }

    #[fuchsia::test]
    #[should_panic(expected = "Attempted to return image 200 which was not currently taken")]
    fn test_double_return_panics() {
        let mut fs = FrameSet::new_with_range(IMAGE_RANGE);
        let img = fs.take_image().expect("image");
        fs.return_image(img);
        fs.return_image(img);
    }

    #[fuchsia::test]
    fn test_take_image_from_empty_pool() {
        let mut fs = FrameSet::new_with_range(200..201);

        let img = fs.take_image();
        assert_eq!(img, Some(200));
        // Pool is empty now.
        assert_eq!(fs.take_image(), None);

        fs.return_image(img.unwrap());
        let img_again = fs.take_image();
        assert_eq!(img_again, Some(200));
    }

    #[fuchsia::test]
    fn test_empty_initial_set() {
        let mut fs = FrameSet::new(BTreeSet::new());
        assert_eq!(fs.image_count_for_testing(), 0);
        assert_eq!(fs.available_count_for_testing(), 0);
        assert!(fs.no_images_in_use());
        assert_eq!(fs.take_image(), None);
    }
}

// TODO: this should eventually be removed in favor of client adding
// CPU access requirements if needed
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrameUsage {
    Cpu,
    Gpu,
}
