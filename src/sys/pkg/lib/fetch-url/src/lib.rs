// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_net_http::{self as http, Header};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use futures::AsyncReadExt as _;
use log::debug;

pub mod errors;
use errors::FetchUrlError;

const HTTP_PARTIAL_CONTENT_OK: u32 = 206;
const HTTP_OK: u32 = 200;

/// The byte range of the fetch request
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    /// The start offset in bytes, zero-indexed, inclusive
    pub start: u64,
    /// The end offset in bytes, inclusive
    pub end: Option<u64>,
}

pub async fn fetch_url(
    url: impl Into<String>,
    range: Option<Range>,
    mut headers: Vec<Header>,
) -> Result<Vec<u8>, FetchUrlError> {
    let http_svc = connect_to_protocol::<http::LoaderMarker>()
        .map_err(FetchUrlError::FidlHttpServiceConnectionError)?;

    let url_string = url.into();

    if let Some(r) = &range {
        const RANGE_HEADER_NAME: &[u8] = b"range";
        if headers.iter().any(|h| h.name.eq_ignore_ascii_case(RANGE_HEADER_NAME)) {
            return Err(FetchUrlError::DuplicateRangeHeader);
        }

        let range_string = if let Some(end) = r.end {
            format!("bytes={}-{}", r.start, end)
        } else {
            format!("bytes={}-", r.start)
        };
        headers.push(Header { name: RANGE_HEADER_NAME.into(), value: range_string.into() });
    }

    let url_request = http::Request {
        url: Some(url_string),
        method: Some(String::from("GET")),
        headers: if headers.is_empty() { None } else { Some(headers) },
        body: None,
        deadline: None,
        ..Default::default()
    };

    let response = http_svc.fetch(url_request).await.map_err(FetchUrlError::LoaderFIDLError)?;

    debug!("got HTTP status {:?} for final URL {:?}", response.status_code, response.final_url);

    if let Some(e) = response.error {
        return Err(FetchUrlError::LoaderFetchError(e));
    }

    let zx_socket = response.body.ok_or(FetchUrlError::UrlReadBodyError)?;
    let mut socket = fasync::Socket::from_socket(zx_socket);

    if let Some(range) = range {
        match response.status_code {
            Some(HTTP_PARTIAL_CONTENT_OK) => {
                let mut body = Vec::new();
                let bytes_received = socket
                    .read_to_end(&mut body)
                    .await
                    .map_err(FetchUrlError::ReadFromSocketError)?
                    as u64;
                let start = range.start;
                if let Some(end) = range.end {
                    let expected = end - start + 1;
                    if bytes_received != expected {
                        return Err(FetchUrlError::SizeReadMismatch(bytes_received, expected));
                    }
                }
                debug!(
                    "successfully fetched partial content starting from {}, {} bytes total",
                    start,
                    body.len()
                );
                Ok(body)
            }
            Some(code) => Err(FetchUrlError::UnexpectedHttpStatusCode(code)),
            None => Err(FetchUrlError::NoStatusResponse),
        }
    } else {
        match response.status_code {
            Some(HTTP_OK | HTTP_PARTIAL_CONTENT_OK) => {
                let mut body = Vec::new();
                let bytes_received = socket
                    .read_to_end(&mut body)
                    .await
                    .map_err(FetchUrlError::ReadFromSocketError)?;
                debug!("successfully fetched content, {} bytes total", bytes_received);
                Ok(body)
            }
            Some(code) => Err(FetchUrlError::UnexpectedHttpStatusCode(code)),
            None => Err(FetchUrlError::NoStatusResponse),
        }
    }
}
