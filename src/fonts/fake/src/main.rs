// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_fonts::{self as fonts};
use fidl_fuchsia_fonts_experimental as fonts_exp;
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use futures::stream::TryStreamExt;

enum ProviderRequestStream {
    Stable(fonts::ProviderRequestStream),
    Experimental(fonts_exp::ProviderRequestStream),
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    log::info!("started");
    let fs = {
        let mut fs = ServiceFs::new();
        fs.dir("svc")
            .add_fidl_service(ProviderRequestStream::Stable)
            .add_fidl_service(ProviderRequestStream::Experimental);
        fs.take_and_serve_directory_handle().context("take_and_serve_directory_handle")?;
        fs
    };

    fs.then(future::ok::<_, Error>)
        .try_for_each_concurrent(None, move |stream| handle_stream(stream))
        .await?;

    Ok(())
}

async fn handle_stream(stream: ProviderRequestStream) -> Result<(), Error> {
    match stream {
        ProviderRequestStream::Stable(stream) => {
            handle_stream_stable(stream).await?;
        }
        ProviderRequestStream::Experimental(stream) => {
            handle_stream_experimental(stream).await?;
        }
    }
    Ok(())
}

async fn handle_stream_stable(mut stream: fonts::ProviderRequestStream) -> Result<(), Error> {
    use fonts::ProviderRequest::*;
    while let Some(request) = stream.try_next().await.context("handle_stream_stable")? {
        log::debug!("request: {}", request.method_name());
        match request {
            GetFont { request: _, responder } => {
                responder.send(None).context("send GetFont")?;
            }
            GetFamilyInfo { family: _, responder } => {
                responder.send(None).context("send GetFamilyInfo")?;
            }
            GetTypeface { request: _, responder } => {
                responder.send(fonts::TypefaceResponse::default()).context("send GetTypeface")?;
            }
            GetFontFamilyInfo { family: _, responder } => {
                responder
                    .send(&fonts::FontFamilyInfo::default())
                    .context("send GetFontFamilyInfo")?;
            }
            RegisterFontSetEventListener { listener: _, responder: _ } => {
                // Not yet supported in the real font server
                unimplemented!()
            }
        }
    }
    Ok(())
}

async fn handle_stream_experimental(
    mut stream: fonts_exp::ProviderRequestStream,
) -> Result<(), Error> {
    use fonts_exp::ProviderRequest::*;
    while let Some(request) = stream.try_next().await.context("handle_stream_experimental")? {
        log::debug!("request: {}", request.method_name());
        match request {
            GetTypefaceById { id: _, responder } => {
                responder.send(Err(fonts_exp::Error::NotFound)).context("send GetTypefaceById")?;
            }
            ListTypefaces { request: _, iterator: _, responder } => {
                responder.send(Err(fonts_exp::Error::NotFound)).context("send ListTypefaces")?;
            }
            GetTypefacesByFamily { family: _, responder } => {
                responder
                    .send(Err(fonts_exp::Error::NotFound))
                    .context("send GetTypefacesByFamily")?;
            }
        }
    }
    Ok(())
}
