// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::magma_system_connection::MagmaStatus;
use crate::traits;

pub struct MagmaSystemBuffer {
    vmo: zx::Vmo,
    msd_buffer: Box<dyn traits::Buffer>,
}

impl MagmaSystemBuffer {
    pub fn new(vmo: zx::Vmo, msd_buffer: Box<dyn traits::Buffer>) -> Self {
        MagmaSystemBuffer { vmo, msd_buffer }
    }

    pub fn size(&self) -> Result<u64, MagmaStatus> {
        self.vmo.get_size().map_err(|_| MagmaStatus::InternalError)
    }

    pub fn global_id(&self) -> Result<u64, MagmaStatus> {
        self.vmo.koid().map(|koid| koid.raw_koid()).map_err(|_| MagmaStatus::InternalError)
    }

    pub fn vmo(&self) -> &zx::Vmo {
        &self.vmo
    }

    pub fn msd_buffer(&self) -> &dyn traits::Buffer {
        &*self.msd_buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockConnection;
    use crate::traits::Connection;

    #[fuchsia::test]
    fn create() {
        let connection = MockConnection;
        let vmo = zx::Vmo::create(4096).unwrap();
        let duplicate_vmo = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap();
        let msd_buffer = connection.import_buffer(duplicate_vmo, 1, 0).unwrap();
        let buffer = MagmaSystemBuffer::new(vmo, msd_buffer);
        assert_eq!(buffer.size().unwrap(), 4096);
    }
}
