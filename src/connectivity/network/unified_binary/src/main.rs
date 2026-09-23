// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

fn main() -> std::process::ExitCode {
    let binary = std::env::args().next().expect("missing first argument");
    let binary = std::path::Path::new(&binary)
        .file_name()
        .expect("missing binary file name")
        .to_str()
        .expect("failed to get file name str");
    match binary {
        "dhcp_client" => {
            dhcp_client::main();
            std::process::ExitCode::SUCCESS
        }
        "dhcpv4_server" => dhcpv4_server::main(),
        "dhcpv6_client" => dhcpv6_client::main(),
        "dns_resolver" => dns_resolver::main(),
        "http_client" => http_client::main(),
        "netcfg_basic" => {
            netcfg_basic::main();
            std::process::ExitCode::SUCCESS
        }
        "netstack_proxy" => netstack_proxy::main(),
        "netstack3" => {
            netstack3::main();
            std::process::ExitCode::SUCCESS
        }
        "reachability" => {
            reachability::main();
            std::process::ExitCode::SUCCESS
        }
        "stack_migration" => {
            stack_migration::main();
            std::process::ExitCode::SUCCESS
        }
        x => panic!("unknown binary {x}"),
    }
}
