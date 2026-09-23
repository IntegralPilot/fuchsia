// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_memory_attribution as fattribution;
use futures::TryStreamExt;
use refaults_vmo::PageRefaultCounter;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use zx::Status;

pub trait RefaultProvider: Clone + Send + Sync + 'static {
    // Returns the current number of page refaults of the system, since its start.
    fn get_count(&self) -> u64;
}

#[derive(Default, Clone)]
pub struct RefaultProviderImpl {
    inner: Arc<Mutex<Inner>>,
}

impl RefaultProviderImpl {
    pub async fn listen_to_page_refaults(
        &self,
        mut stream: fattribution::PageRefaultSinkRequestStream,
    ) -> Result<(), anyhow::Error> {
        loop {
            match stream.try_next().await {
                Ok(Some(request)) => match request {
                    fattribution::PageRefaultSinkRequest::SendPageRefaultCount {
                        page_refaults_vmo,
                        ..
                    } => {
                        self.inner.lock().unwrap().set_new_counter(page_refaults_vmo)?;
                    }
                    fattribution::PageRefaultSinkRequest::_UnknownMethod { .. } => unimplemented!(),
                },
                Ok(None) => {
                    return Ok(());
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        }
    }
}

impl RefaultProvider for RefaultProviderImpl {
    fn get_count(&self) -> u64 {
        self.inner.lock().unwrap().get_count()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct CounterIndex(usize);

#[derive(Default)]
struct Inner {
    counters: HashMap<CounterIndex, PageRefaultCounter>,
}

impl Inner {
    fn set_new_counter(&mut self, page_refaults_vmo: zx::Vmo) -> Result<(), Status> {
        let index = CounterIndex(self.counters.len());
        self.counters.insert(index, PageRefaultCounter::from_vmo_readonly(page_refaults_vmo)?);
        Ok(())
    }

    fn get_count(&self) -> u64 {
        self.counters.values().map(|c| c.read(Ordering::Relaxed)).sum::<u64>()
    }
}
