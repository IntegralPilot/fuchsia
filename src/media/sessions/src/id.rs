// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Result, SessionId};

/// A universally unique id.
#[derive(Debug)]
pub struct Id {
    id: SessionId,
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    id_handle: zx::Event,
}

impl Id {
    pub fn new() -> Result<Self> {
        let id_handle = zx::Event::create();
        let id = id_handle.koid()?.raw_koid();

        Ok(Self { id, id_handle })
    }

    pub fn get(&self) -> SessionId {
        self.id
    }
}
