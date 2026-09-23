// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ops::Deref;

pub(crate) trait SizeOfContents {
    fn size_of_contents_in_bytes(&self) -> usize;
}

impl<T, U> SizeOfContents for T
where
    T: Deref<Target = [U]>,
{
    fn size_of_contents_in_bytes(&self) -> usize {
        self.deref().len() * std::mem::size_of::<U>()
    }
}
