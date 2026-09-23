// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs)]

//! Utilities for tests that interact with fidl.

use fidl::endpoints::{Proxy, Request, RequestStream, create_proxy_and_stream};
use fuchsia_async::Task;
use futures::{FutureExt, TryFutureExt, TryStreamExt};
use log::error;

/// FDomain versions of the utilities in this crate.
pub mod fdomain {
    use fdomain_client::Client;
    use fdomain_client::fidl::{Proxy, Request, RequestStream};
    use fuchsia_async::Task;
    use futures::{FutureExt, TryFutureExt, TryStreamExt};
    use log::error;
    use std::sync::Arc;

    /// Utility that spawns a new task to handle requests that require a
    /// singlethreaded executor. The requests are handled one at a time.
    pub fn spawn_local_stream_handler<P, F, Fut>(client: &Arc<Client>, f: F) -> P
    where
        P: Proxy,
        F: FnMut(Request<P::Protocol>) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        let (proxy, stream) = client.create_proxy_and_stream::<P::Protocol>();
        Task::local(for_each_or_log(Arc::clone(client), stream, f)).detach();
        proxy
    }

    /// Utility that spawns a new task to handle requests of a particular type. The request handler
    /// must be threadsafe. The requests are handled one at a time.
    pub fn spawn_stream_handler<P, F, Fut>(client: &Arc<Client>, f: F) -> P
    where
        P: Proxy,
        F: FnMut(Request<P::Protocol>) -> Fut + 'static + Send,
        Fut: Future<Output = ()> + 'static + Send,
    {
        let (proxy, stream) = client.create_proxy_and_stream::<P::Protocol>();
        Task::spawn(for_each_or_log(Arc::clone(client), stream, f)).detach();
        proxy
    }

    fn for_each_or_log<St, F, Fut>(
        client: Arc<Client>,
        stream: St,
        mut f: F,
    ) -> impl Future<Output = ()>
    where
        St: RequestStream,
        F: FnMut(St::Ok) -> Fut,
        Fut: Future<Output = ()>,
    {
        async move {
            // We just want to keep the client around so we don't have to handle a
            // transport error if it drops.
            let _client = client;

            stream
                .try_for_each(move |r| f(r).map(Ok))
                .unwrap_or_else(|e| error!("FIDL stream handler failed: {}", e))
                .await
        }
    }
}

/// Utility that spawns a new task to handle requests of a particular type, requiring a
/// singlethreaded executor. The requests are handled one at a time.
pub fn spawn_local_stream_handler<P, F, Fut>(f: F) -> P
where
    P: Proxy,
    F: FnMut(Request<P::Protocol>) -> Fut + 'static,
    Fut: Future<Output = ()> + 'static,
{
    let (proxy, stream) = create_proxy_and_stream::<P::Protocol>();
    Task::local(for_each_or_log(stream, f)).detach();
    proxy
}

/// Utility that spawns a new task to handle requests of a particular type. The request handler
/// must be threadsafe. The requests are handled one at a time.
pub fn spawn_stream_handler<P, F, Fut>(f: F) -> P
where
    P: Proxy,
    F: FnMut(Request<P::Protocol>) -> Fut + 'static + Send,
    Fut: Future<Output = ()> + 'static + Send,
{
    let (proxy, stream) = create_proxy_and_stream::<P::Protocol>();
    Task::spawn(for_each_or_log(stream, f)).detach();
    proxy
}

fn for_each_or_log<St, F, Fut>(stream: St, mut f: F) -> impl Future<Output = ()>
where
    St: RequestStream,
    F: FnMut(St::Ok) -> Fut,
    Fut: Future<Output = ()>,
{
    stream
        .try_for_each(move |r| f(r).map(Ok))
        .unwrap_or_else(|e| error!("FIDL stream handler failed: {}", e))
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_test_placeholders::{EchoProxy as FEchoProxy, EchoRequest as FEchoRequest};
    use fidl_test_placeholders::{EchoProxy, EchoRequest};

    #[fuchsia::test]
    async fn test_spawn_local_stream_handler() {
        let f = |req| {
            let EchoRequest::EchoString { value, responder } = req;
            async move {
                responder.send(Some(&value.unwrap())).expect("responder failed");
            }
        };
        let proxy: EchoProxy = spawn_local_stream_handler(f);
        let res = proxy.echo_string(Some("hello world")).await.expect("echo failed");
        assert_eq!(res, Some("hello world".to_string()));
        let res = proxy.echo_string(Some("goodbye world")).await.expect("echo failed");
        assert_eq!(res, Some("goodbye world".to_string()));
    }

    #[fuchsia::test(threads = 2)]
    async fn test_spawn_stream_handler() {
        let f = |req| {
            let EchoRequest::EchoString { value, responder } = req;
            async move {
                responder.send(Some(&value.unwrap())).expect("responder failed");
            }
        };
        let proxy: EchoProxy = spawn_stream_handler(f);
        let res = proxy.echo_string(Some("hello world")).await.expect("echo failed");
        assert_eq!(res, Some("hello world".to_string()));
        let res = proxy.echo_string(Some("goodbye world")).await.expect("echo failed");
        assert_eq!(res, Some("goodbye world".to_string()));
    }

    #[fuchsia::test]
    async fn test_spawn_local_stream_handler_fdomain() {
        let thread_unsafe = std::rc::Rc::new(());
        let client = fdomain_local::local_client_empty();
        let f = move |req| {
            let _thread_unsafe = thread_unsafe.clone();
            let FEchoRequest::EchoString { value, responder } = req;
            async move {
                responder.send(Some(&value.unwrap())).expect("responder failed");
            }
        };
        let proxy: FEchoProxy = fdomain::spawn_local_stream_handler(&client, f);
        let res = proxy.echo_string(Some("hello world")).await.expect("echo failed");
        assert_eq!(res, Some("hello world".to_string()));
        let res = proxy.echo_string(Some("goodbye world")).await.expect("echo failed");
        assert_eq!(res, Some("goodbye world".to_string()));
    }

    #[fuchsia::test(threads = 2)]
    async fn test_spawn_stream_handler_fdomain() {
        let client = fdomain_local::local_client_empty();
        let f = |req| {
            let FEchoRequest::EchoString { value, responder } = req;
            async move {
                responder.send(Some(&value.unwrap())).expect("responder failed");
            }
        };
        let proxy: FEchoProxy = fdomain::spawn_stream_handler(&client, f);
        let res = proxy.echo_string(Some("hello world")).await.expect("echo failed");
        assert_eq!(res, Some("hello world".to_string()));
        let res = proxy.echo_string(Some("goodbye world")).await.expect("echo failed");
        assert_eq!(res, Some("goodbye world".to_string()));
    }
}
