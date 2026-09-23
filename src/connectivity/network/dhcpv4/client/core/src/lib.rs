// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]
//! Defines the platform-agnostic "core" of a DHCP client, including a state
//! machine and abstractions for sockets and time.

/// Defines the client core state machine.
pub mod client;

/// Defines abstractions for platform dependencies such as sockets and time.
pub mod deps;

/// Defines helpers for providing inspect / diagnostic signal.
pub mod inspect;

/// Defines types and utilities for parsing DHCP messages received by the
/// client and serializing DHCP messages sent by the client.
pub mod parse;
