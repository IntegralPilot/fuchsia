// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Physical and operational hardware limits for the Goodix GT6853 Touch IC.

/// Maximum number of simultaneous touch contact points supported by the GT6853 hardware.
//
// @cite(gt6853-hardware-description): sec=3.4 title="Coordinate Information Buffer"
// @cite(gt6853-programming-guide): sec=3.4 title="Coordinate Information"
// @cite(gt6853-datasheet): sec=1 title="Overview"
pub const MAX_CONTACTS: usize = 10;

/// Maximum physical X coordinate defined by the Nelson panel firmware.
pub const NELSON_MAX_CONTACT_X: i64 = 600;

/// Maximum physical Y coordinate defined by the Nelson panel firmware.
pub const NELSON_MAX_CONTACT_Y: i64 = 1024;
