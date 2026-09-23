// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_net_name as fnet_name;
use fuchsia_component::server::{ServiceFs, ServiceFsDir};
use futures::{StreamExt as _, TryStreamExt as _};

// The maximum number of addresses that fdio can handle.
const MAXADDRS: usize = 1024;

#[fuchsia::main]
async fn main() {
    let mut fs = ServiceFs::new_local();
    let _: &mut ServiceFsDir<'_, _> =
        fs.dir("svc").add_fidl_service(|s: fnet_name::LookupRequestStream| s);
    let _ = fs.take_and_serve_directory_handle().expect("failed to get startup handle");

    fs.map(Ok)
        .try_for_each_concurrent(None, |stream| {
            stream.try_for_each_concurrent(None, handle_request)
        })
        .await
        .expect("server failed")
}

async fn handle_request(request: fnet_name::LookupRequest) -> Result<(), fidl::Error> {
    match request {
        fnet_name::LookupRequest::LookupIp { hostname, options, responder } => {
            let size = match hostname.as_str() {
                "example.com" => 1,
                "lotsofrecords.com" => MAXADDRS,
                "google.com" => return responder.send(Err(fnet_name::LookupError::NotFound)),
                hostname => panic!("unexpected hostname {}", hostname),
            };
            let fnet_name::LookupIpOptions { ipv4_lookup, ipv6_lookup, .. } = options;
            let ipv4_addresses = ipv4_lookup
                .unwrap_or(false)
                .then(|| std::iter::repeat(net_declare::fidl_ip!("192.0.2.1")).take(size))
                .into_iter()
                .flatten();
            let ipv6_addresses = ipv6_lookup
                .unwrap_or(false)
                .then(|| std::iter::repeat(net_declare::fidl_ip!("2001:db8::1")).take(size))
                .into_iter()
                .flatten();
            let addresses = Some(ipv4_addresses.chain(ipv6_addresses).collect());
            responder.send(Ok(&fnet_name::LookupResult { addresses, ..Default::default() }))
        }
        request => panic!("unexpected request: {:?}", request),
    }
}
