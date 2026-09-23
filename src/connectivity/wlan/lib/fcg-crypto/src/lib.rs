// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Library containing:
//! - Finite Cyclic Group (FCG) cryptographic operations.
//! - Implementations of WLAN security features that leverage FCG cryptographic operations,
//!   such as Simultaneous Authentication of Equals (SAE) and Opportunistic Wireless
//!   Encryption (OWE).
mod boringssl;
mod ecc;
mod fcg;
pub mod hmac_utils;
pub mod owe;
pub mod sae;
