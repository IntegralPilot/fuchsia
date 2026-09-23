// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_net_http as fnet_http;
use fuchsia_async as fasync;
use fuchsia_async::TimeoutExt as _;
use futures::future::{FutureExt as _, TryFutureExt as _};
use http_uri_ext::HttpUriExt as _;

pub(crate) struct FuchsiaNetHttpRepository<D> {
    uri: http::Uri,
    loader: fnet_http::LoaderProxy,
    network_header_timeout: zx::BootDuration,
    pouf: std::marker::PhantomData<D>,
}

impl<D> FuchsiaNetHttpRepository<D>
where
    D: tuf::pouf::Pouf,
{
    pub(crate) fn new(
        uri: http::Uri,
        loader: fnet_http::LoaderProxy,
        network_header_timeout: zx::BootDuration,
    ) -> Self {
        Self { uri, loader, network_header_timeout, pouf: std::marker::PhantomData }
    }
}

impl<D> tuf::repository::RepositoryProvider<D> for FuchsiaNetHttpRepository<D>
where
    D: tuf::pouf::Pouf + Sync + Send,
{
    fn fetch_metadata<'a>(
        &'a self,
        meta_path: &tuf::metadata::MetadataPath,
        version: tuf::metadata::MetadataVersion,
    ) -> futures::future::BoxFuture<
        'a,
        tuf::Result<Box<dyn futures::io::AsyncRead + Send + Unpin + 'a>>,
    > {
        let meta_path = meta_path.clone();
        async move {
            let uri = self
                .uri
                .clone()
                .extend_dir_with_path(&meta_path.components::<D>(version).join("/"))
                .map_err(|e| {
                    tuf::error::Error::IllegalArgument(format!(
                        "failed to extend uri {} with path {} and version {}: {:#}",
                        self.uri,
                        meta_path,
                        version,
                        anyhow::anyhow!(e)
                    ))
                })?;
            let resp = self
                .loader
                .fetch(fnet_http::Request {
                    method: None,
                    url: Some(uri.to_string()),
                    headers: None,
                    body: None,
                    // Request a longer timeout than is used by the following on_timeout to avoid a
                    // race between the timeouts, so that our error handling and metric creation
                    // code does not have to handle both cases.
                    deadline: Some(
                        zx::BootInstant::after(
                            self.network_header_timeout + zx::BootDuration::from_seconds(5),
                        )
                        .into_nanos(),
                    ),
                    ..Default::default()
                })
                .map_err(|e| {
                    tuf::error::Error::Opaque(format!(
                        "failed to call fuchsia.net.http/Loader.Fetch with uri {uri}: {e:?}"
                    ))
                })
                // Use on_timeout instead of just Loader.Fetch's deadline so that a misbehaving
                // http-client can't hang us.
                .on_timeout(self.network_header_timeout, || {
                    Err(tuf::error::Error::Opaque(format!(
                        "timeout waiting for HTTP Response on {uri}"
                    )))
                })
                .await?;
            let socket = match resp {
                fnet_http::Response {
                    error: None,
                    body: Some(body),
                    status_code: Some(200),
                    ..
                } => body,
                fnet_http::Response { status_code: Some(404), .. } => {
                    return Err(tuf::Error::MetadataNotFound { path: meta_path, version });
                }
                fnet_http::Response { status_code: Some(code), .. } => {
                    if let Ok(code) = u16::try_from(code)
                        .map_err(|_| ())
                        .and_then(|c| http::StatusCode::from_u16(c).map_err(|_| ()))
                    {
                        return Err(tuf::Error::BadHttpStatus { uri: uri.to_string(), code });
                    }
                    return Err(tuf::Error::Opaque(format!(
                        "HTTP Get failed invalid status_code {code} {uri}"
                    )));
                }
                fnet_http::Response { error, status_code: None, status_line, .. } => {
                    return Err(tuf::Error::Opaque(format!(
                        "HTTP Get failed no status_code {error:?} {status_line:?} {uri}"
                    )));
                }
            };
            Ok(Box::new(fasync::Socket::from_socket(socket))
                as Box<dyn futures::io::AsyncRead + Send + Unpin + 'a>)
        }
        .boxed()
    }

    // Not implemented. Fuchsia uses TUF to get the hash of the package's meta.far and then
    // downloads it directly instead of using TUF's "targets" feature.
    fn fetch_target<'a>(
        &'a self,
        _target_path: &tuf::metadata::TargetPath,
    ) -> futures::future::BoxFuture<
        'a,
        tuf::Result<Box<dyn futures::io::AsyncRead + Send + Unpin + 'a>>,
    > {
        futures::future::ready(Err(tuf::Error::Opaque("fetch_target not implemented".to_owned())))
            .boxed()
    }
}
