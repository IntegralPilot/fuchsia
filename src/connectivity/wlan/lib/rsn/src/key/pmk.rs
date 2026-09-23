// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[derive(Debug, Clone, PartialEq)]
pub struct Pmk {
    pub pmk: Vec<u8>,
    pub pmkid: Option<Vec<u8>>,
}

impl Pmk {
    pub fn new(pmk: Vec<u8>, pmkid: Option<Vec<u8>>) -> Self {
        Self { pmk, pmkid }
    }

    pub fn from_pmk(pmk: Vec<u8>) -> Self {
        Self { pmk, pmkid: None }
    }
}

impl From<Vec<u8>> for Pmk {
    fn from(pmk: Vec<u8>) -> Self {
        Self::from_pmk(pmk)
    }
}
