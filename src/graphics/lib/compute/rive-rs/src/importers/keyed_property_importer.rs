// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::animation::{KeyFrame, KeyedProperty, LinearAnimation};
use crate::core::Object;
use crate::importers::ImportStackObject;

#[derive(Debug)]
pub struct KeyedPropertyImporter {
    animation: Object<LinearAnimation>,
    keyed_property: Object<KeyedProperty>,
}

impl KeyedPropertyImporter {
    pub fn new(animation: Object<LinearAnimation>, keyed_property: Object<KeyedProperty>) -> Self {
        Self { animation, keyed_property }
    }

    pub fn push_key_frame(&self, key_frame: Object<KeyFrame>) {
        key_frame.as_ref().compute_seconds(self.animation.as_ref().fps());
        self.keyed_property.as_ref().push_key_frame(key_frame);
    }
}

impl ImportStackObject for KeyedPropertyImporter {}
