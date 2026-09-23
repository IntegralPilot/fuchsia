// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod log;
mod log_flush;
mod log_freeze;
mod log_settings;
mod log_stream;

pub use log::LogServer;
pub use log_flush::LogFlushServer;
pub use log_freeze::LogFreezeServer;
pub use log_settings::LogSettingsServer;
pub use log_stream::{LogStreamServer, StreamError};

pub(crate) use log_stream::{ExtendRecordOpts, extend_fxt_record};
