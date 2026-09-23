// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::TryFromIntError;
use std::sync::Arc;

pub mod refaults;

#[derive(Default)]
pub struct MemoryStallMetrics {
    pub some: std::time::Duration,
    pub full: std::time::Duration,
}

impl TryFrom<zx::MemoryStall> for MemoryStallMetrics {
    type Error = TryFromIntError;

    fn try_from(stall: zx::MemoryStall) -> Result<Self, Self::Error> {
        Ok(Self {
            some: std::time::Duration::from_nanos(stall.stall_time_some.try_into()?),
            full: std::time::Duration::from_nanos(stall.stall_time_full.try_into()?),
        })
    }
}

pub trait StallProvider: Clone + Sync + Send + 'static {
    /// Return the current memory stall values from the kernel.
    fn get_stall_info(&self) -> Result<MemoryStallMetrics, anyhow::Error>;
}

#[derive(Clone)]
pub struct StallProviderImpl {
    /// Memory stall kernel resource, for issuing queries.
    stall_resource: Arc<dyn StallResource>,
}

/// Trait for a resource exposing memory stall information. Used for dependency injection in unit
/// tests.
pub trait StallResource: Sync + Send {
    fn get_memory_stall(&self) -> Result<zx::MemoryStall, zx::Status>;
}

impl StallResource for zx::Resource {
    fn get_memory_stall(&self) -> Result<zx::MemoryStall, zx::Status> {
        self.memory_stall()
    }
}

impl StallProviderImpl {
    /// Create a new [StallProviderImpl], wrapping a [StallResource].
    pub fn new(stall_resource: Arc<dyn StallResource>) -> Result<StallProviderImpl, anyhow::Error> {
        Ok(StallProviderImpl { stall_resource: stall_resource })
    }
}

impl StallProvider for StallProviderImpl {
    fn get_stall_info(&self) -> Result<MemoryStallMetrics, anyhow::Error> {
        Ok(self.stall_resource.get_memory_stall()?.try_into()?)
    }
}
