// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use fidl::endpoints::{ControlHandle as _, DiscoverableProtocolMarker as _, Responder as _};
use fidl_fuchsia_posix_socket as fposix_socket;
use fuchsia_async as fasync;
use futures::stream::StreamExt as _;
use tcp_stream_ext::TcpStreamExt as _;

const TCP_USER_TIMEOUT_OPTION_VALUE: i32 = -13;

fn with_tcp_stream(f: impl FnOnce(std::net::TcpStream) -> ()) {
    let (client, server) = fidl::endpoints::create_endpoints::<fposix_socket::StreamSocketMarker>();

    // fdio::create_fd isn't async, so we need a dedicated thread for FIDL dispatch.
    let handle = std::thread::spawn(|| {
        fasync::LocalExecutor::default().run_singlethreaded(server.into_stream().for_each(
            move |request| {
                futures::future::ready(match request.expect("stream socket request stream") {
                    fposix_socket::StreamSocketRequest::Close { responder } => {
                        let () = responder.control_handle().shutdown();
                        let () = responder.send(Ok(())).expect("send Close response");
                    }
                    fposix_socket::StreamSocketRequest::Query { responder } => {
                        let () = responder
                            .send(fposix_socket::StreamSocketMarker::PROTOCOL_NAME.as_bytes())
                            .expect("send Query response");
                    }
                    fposix_socket::StreamSocketRequest::Describe { responder } => {
                        let (s0, _s1) = zx::Socket::create_stream();
                        let () = responder
                            .send(fposix_socket::StreamSocketDescribeResponse {
                                socket: Some(s0),
                                ..Default::default()
                            })
                            .expect("send Describe response");
                    }
                    fposix_socket::StreamSocketRequest::GetTcpUserTimeout { responder } => {
                        let () = responder
                            .send(Ok(TCP_USER_TIMEOUT_OPTION_VALUE as u32))
                            .expect("send TcpUserTimeout response");
                    }
                    request => panic!("unhandled StreamSocketRequest: {:?}", request),
                })
            },
        ))
    });
    let () = f(fdio::create_fd(client.into_channel().into()).expect("endpoint into handle").into());
    handle.join().expect("thread join")
}

#[test]
fn user_timeout_errors_on_negative_duration() {
    with_tcp_stream(|stream| {
        assert_matches::assert_matches!(
            stream.user_timeout(),
            Err(tcp_stream_ext::Error::NegativeDuration(TCP_USER_TIMEOUT_OPTION_VALUE))
        )
    })
}
