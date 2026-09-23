// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_driver_development as fdd;
use futures::prelude::*;

pub(crate) struct DeviceInfoIterator {
    infos: std::vec::IntoIter<fdd::NodeInfo>,
}

impl DeviceInfoIterator {
    pub(crate) fn new(list: Vec<fdd::NodeInfo>) -> Self {
        Self { infos: list.into_iter() }
    }

    pub(crate) async fn serve(
        mut self,
        mut stream: fdd::NodeInfoIteratorRequestStream,
    ) -> Result<(), fidl::Error> {
        const MAX_ENTRIES: usize = 7;
        while let Some(request) = stream.try_next().await? {
            match request {
                fdd::NodeInfoIteratorRequest::GetNext { responder } => {
                    let chunk: Vec<fdd::NodeInfo> = self.infos.by_ref().take(MAX_ENTRIES).collect();
                    responder.send(&chunk)?;
                }
            }
        }
        Ok(())
    }
}

pub(crate) struct CompositeInfoIterator {
    infos: std::vec::IntoIter<fdd::CompositeNodeInfo>,
}

impl CompositeInfoIterator {
    pub(crate) fn new(list: Vec<fdd::CompositeNodeInfo>) -> Self {
        Self { infos: list.into_iter() }
    }

    pub(crate) async fn serve(
        mut self,
        mut stream: fdd::CompositeInfoIteratorRequestStream,
    ) -> Result<(), fidl::Error> {
        const MAX_ENTRIES: usize = 7;
        while let Some(request) = stream.try_next().await? {
            match request {
                fdd::CompositeInfoIteratorRequest::GetNext { responder } => {
                    let chunk: Vec<fdd::CompositeNodeInfo> =
                        self.infos.by_ref().take(MAX_ENTRIES).collect();
                    responder.send(&chunk)?;
                }
            }
        }
        Ok(())
    }
}

pub(crate) struct DriverHostInfoIterator {
    infos: std::vec::IntoIter<fdd::DriverHostInfo>,
}

impl DriverHostInfoIterator {
    pub(crate) fn new(infos: Vec<fdd::DriverHostInfo>) -> Self {
        Self { infos: infos.into_iter() }
    }

    pub(crate) async fn serve(
        mut self,
        mut stream: fdd::DriverHostInfoIteratorRequestStream,
    ) -> Result<(), fidl::Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                fdd::DriverHostInfoIteratorRequest::GetNext { responder } => {
                    let next_infos: Vec<fdd::DriverHostInfo> =
                        self.infos.by_ref().take(100).collect();
                    responder.send(&next_infos)?;
                }
            }
        }
        Ok(())
    }
}
