// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Deref;

#[derive(Copy, Clone, Debug, Default, Deserialize, Hash, PartialEq, Eq, Serialize)]
pub struct ProjectId(pub u32);

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for ProjectId {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
pub struct MetricId(pub u32);

impl fmt::Display for MetricId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for MetricId {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
pub struct EventCode(pub u32);

impl fmt::Display for EventCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Deref for EventCode {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Supported Cobalt Metric types
#[derive(Deserialize, Debug, PartialEq, Eq, Hash, Copy, Clone, Serialize)]
pub enum MetricType {
    /// Maps cached diffs from Uint or Int Inspect types.
    /// NOTE: This does not use duration tracking. Durations are always set to 0.
    Occurrence,

    /// Maps raw Int Inspect types.
    Integer,

    /// Maps cached diffs from IntHistogram Inspect type.
    IntHistogram,

    /// Maps Inspect String type to StringValue (Cobalt 1.1 only).
    String,
}
