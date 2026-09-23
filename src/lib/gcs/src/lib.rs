// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A GCS Client may be given an initial access token up front or it may be
//! added later.
//!
//! E.g.
//! ```
//! let client = Client.initial();
//! let access_token = new_access_token();
//! client.set_access_token(access_token).await;
//! let res = client.download("some-bucket", "some-object").await?;
//! if res.status() == StatusCode::OK {
//!     let stdout = io::stdout();
//!     let mut handle = stdout.lock();
//!     while let Some(next) = res.data().await {
//!         let chunk = next.expect("next chunk");
//!         handle.write_all(&chunk).expect("write chunk");
//!     }
//! }
//! ```
//!
//! After creating an initial client, it's a good idea to keep using that client
//! repeatedly and/or create clones (.clone()) if multiple are desired. This
//! practices shares an access token which provides a better experience.

pub mod auth;
pub mod client;
pub mod error;
pub mod exponential_backoff;
pub mod gs_url;
pub mod mock_https_client;
pub mod token_store;
