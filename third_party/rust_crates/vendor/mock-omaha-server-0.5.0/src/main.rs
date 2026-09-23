// Copyright 2022 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

use argh::FromArgs;

use mock_omaha_server::{
    OmahaServer, OmahaServerBuilder, PrivateKeyAndId, PrivateKeys, ResponseAndMetadata,
};
use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};

#[derive(FromArgs)]
/// Arguments for mock-omaha-server.
struct Args {
    /// A hashmap from appid to response metadata struct.
    /// Example JSON argument (the minimal set of required fields per appid):
    ///     {
    ///         "appid_01": {
    ///             "response": "NoUpdate",
    ///             "check_assertion": "UpdatesEnabled",
    ///             "version": "0.1.2.3",
    ///             "codebase": "fuchsia-pkg://omaha.mock.fuchsia.com/",
    ///             "package_name": "update?hash=deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
    ///         },
    ///         ...
    ///     }
    #[argh(
        option,
        description = "responses and metadata keyed by appid",
        from_str_fn(parse_responses_by_appid),
        default = "parse_responses_by_appid(EXAMPLE_RESPONSES_BY_APPID).unwrap()"
    )]
    responses_by_appid: HashMap<String, ResponseAndMetadata>,

    #[argh(
        option,
        description = "private key ID",
        default = "DEFAULT_PRIVATE_KEY_ID.try_into().expect(\"key parse\")"
    )]
    key_id: u64,

    #[argh(
        option,
        description = "path to private key",
        default = "\"mock-omaha-server/src/testing_keys/test_private_key.pem\".to_string()"
    )]
    key_path: String,

    #[argh(option, description = "which port to serve on", default = "0")]
    port: u16,

    #[argh(
        option,
        description = "which IP address to listen on. One of '::', '::1', or anything Ipv6Addr::from_str() can interpret.",
        default = "Ipv6Addr::UNSPECIFIED"
    )]
    listen_on: Ipv6Addr,

    #[argh(switch, description = "if 'true', will only accept requests with CUP enabled.")]
    require_cup: bool,
}

fn parse_responses_by_appid(value: &str) -> Result<HashMap<String, ResponseAndMetadata>, String> {
    serde_json::from_str(value).map_err(|e| format!("Parsing failed: {e:?}"))
}

pub const DEFAULT_PRIVATE_KEY_ID: i32 = 42;
const EXAMPLE_RESPONSES_BY_APPID: &str = r#"
{
  "appid_01": {
    "response": "NoUpdate",
    "check_assertion": "UpdatesEnabled",
    "version": "14.20230831.4.72",
    "codebase": "fuchsia-pkg://omaha.mock.fuchsia.com/",
    "package_name": "update?hash=deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
  }
}
"#;

#[cfg(feature = "tokio")]
#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let args: Args = argh::from_env();

    let server = Arc::new(Mutex::new(
        OmahaServerBuilder::default()
            .responses_by_appid(args.responses_by_appid)
            .private_keys(PrivateKeys {
                latest: PrivateKeyAndId {
                    id: args.key_id,
                    key: std::fs::read_to_string(&args.key_path)
                        .unwrap_or_else(|_| {
                            panic!("read from key_path '{:#?}' failed", args.key_path)
                        })
                        .parse()
                        .expect("failed to parse key"),
                },
                historical: vec![],
            })
            .require_cup(args.require_cup)
            .build()
            .expect("omaha server build"),
    ));

    let addr = SocketAddr::new(args.listen_on.into(), args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let local_addr = listener.local_addr()?;

    println!("listening on http://{local_addr}/");
    OmahaServer::start(server, listener, mock_omaha_server::TokioExecutor).await?;
    Ok(())
}
