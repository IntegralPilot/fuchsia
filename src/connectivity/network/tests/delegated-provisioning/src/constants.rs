// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Constants required by both `bootstrap_dhcp.rs` and `test.rs`.

use fidl_fuchsia_net as fnet;
use net_declare::fidl_ip_v4;

pub const SERVER_STATIC_IP: fnet::Ipv4Address = fidl_ip_v4!("192.0.168.1");
pub const DHCP_DYNAMIC_IP: fnet::Ipv4Address = fidl_ip_v4!("192.0.168.2");

pub const CLIENT_IFACE_NAME: &'static str = "starnixethx1";
pub const SERVER_IFACE_NAME: &'static str = "server-ep";
