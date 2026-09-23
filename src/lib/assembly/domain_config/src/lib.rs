// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Library for constructing Domain Config packages.
//!
//! A Domain Config package is a Fuchsia package that holds config files inside
//! directories, and a component manifest for routing those directories to other
//! components so they can read the config files.

mod domain_config;

pub use crate::domain_config::DomainConfigPackage;
