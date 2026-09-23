// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#![recursion_limit = "256"]

mod decode;
mod encode;
mod error;
mod handle;
mod transaction;
mod util;
mod value;

pub mod library;

pub use decode::{decode, decode_request, decode_response};
pub use encode::{encode, encode_request, encode_response};
pub use error::Error;
pub use handle::*;
pub use value::Value;

pub use fidl_data_zx as zx;
pub use fidl_data_zx::{ObjType as ObjectType, Rights};
