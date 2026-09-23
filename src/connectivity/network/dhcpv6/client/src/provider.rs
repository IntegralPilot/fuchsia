// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_net_dhcpv6::{ClientMarker, ClientProviderRequest, ClientProviderRequestStream};
use fidl_fuchsia_net_dhcpv6_ext::NewClientParams;

use futures::{Future, StreamExt as _};

use anyhow::Result;
use log::warn;

/// Handles client provider requests from the input stream.
pub(crate) async fn run_client_provider<Fut, F>(
    stream: ClientProviderRequestStream,
    serve_client: F,
) where
    Fut: Future<Output = Result<()>>,
    F: Fn(NewClientParams, ServerEnd<ClientMarker>) -> Fut,
{
    stream
        .for_each_concurrent(None, |request| async {
            match request {
                Ok(ClientProviderRequest::NewClient { params, request, control_handle: _ }) => {
                    let params: NewClientParams = match params.try_into() {
                        Ok(params) => params,
                        Err(e) => {
                            warn!("NewClientParams validation error: {}", e);
                            // All param fields are required.
                            request
                                .close_with_epitaph(zx::Status::INVALID_ARGS)
                                .unwrap_or_else(|e| warn!("closing NewClient request channel with epitaph INVALID_ARGS: {}", e));
                            return;
                        }
                    };
                    // `NewClientParams` does not implement `Clone`. It is also non-trivial to pass
                    // a reference of `params` to `serve_client` because that would require adding
                    // lifetimes in quite a few places.
                    let params_str = format!("{:?}", params);
                    let () =
                        serve_client(params, request).await.unwrap_or_else(|e: anyhow::Error| {
                            // TODO(https://fxbug.dev/42069288): Return error through
                            // a terminal event.
                            warn!("error running client with params {}: {:?}", params_str, e);
                        });
                }
                Err(e) => warn!("client provider request FIDL error: {}", e),
            }
        })
        .await
}

#[cfg(test)]
mod tests {
    use fidl::endpoints::create_endpoints;
    use fidl_fuchsia_net_dhcpv6::ClientProviderMarker;
    use fidl_fuchsia_net_dhcpv6_ext::ClientConfig;
    use futures::join;

    use anyhow::{Error, anyhow};
    use assert_matches::assert_matches;
    use net_declare::fidl_socket_addr_v6;

    use super::*;

    async fn serve_client(
        _param: NewClientParams,
        _request: ServerEnd<ClientMarker>,
    ) -> Result<()> {
        Ok(())
    }

    async fn start_err_client(
        _param: NewClientParams,
        _request: ServerEnd<ClientMarker>,
    ) -> Result<()> {
        Err(anyhow!("fake test error"))
    }

    async fn test_client_provider<Fut, F>(serve_client: F)
    where
        Fut: Future<Output = Result<()>>,
        F: Fn(NewClientParams, ServerEnd<ClientMarker>) -> Fut,
    {
        let (client_end, server_end) = create_endpoints::<ClientProviderMarker>();
        let client_provider_proxy = client_end.into_proxy();
        let client_provider_stream = server_end.into_stream();

        let test_fut = async {
            for interface_id in 0..10 {
                let (_client_end, server_end) = create_endpoints::<ClientMarker>();
                client_provider_proxy
                    .new_client(
                        &NewClientParams {
                            interface_id: interface_id,
                            address: fidl_socket_addr_v6!("[fe01::1:2]:546"),
                            config: ClientConfig {
                                information_config: Default::default(),
                                non_temporary_address_config: Default::default(),
                                prefix_delegation_config: None,
                            },
                            duid: None,
                        }
                        .into(),
                        server_end,
                    )
                    .expect("failed to request new client");
            }
            drop(client_provider_proxy);
            Ok(())
        };
        let provider_fut = run_client_provider(client_provider_stream, serve_client);

        let (test_res, ()): (Result<_, Error>, ()) = join!(test_fut, provider_fut);
        assert_matches!(test_res, Ok(()));
    }

    #[fuchsia::test]
    async fn test_client_provider_serve_client_success() {
        test_client_provider(serve_client).await;
    }

    #[fuchsia::test]
    async fn test_client_provider_should_keep_running_on_client_err() {
        test_client_provider(start_err_client).await;
    }
}
