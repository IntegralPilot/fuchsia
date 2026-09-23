// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_client::{Channel, Client, FDomainTransport};
use fdomain_container::FDomain;
use fdomain_container::wire::FDomainCodec;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_io as fio;
use futures::StreamExt;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, Weak};
use std::task::{Context, Poll};

/// An FDomain that is designed to be used in the same process it was created
/// in. I.e. no networking, just a bucket of handles right here where you can
/// use them.
struct LocalFDomain(FDomainCodec);

impl LocalFDomain {
    /// Create a new FDomain client that points to a new local FDomain.
    fn new_client(
        namespace: impl Fn() -> Result<ClientEnd<fio::DirectoryMarker>, fidl::Status> + Send + 'static,
    ) -> Arc<Client> {
        let (client, fut) = Client::new(LocalFDomain(FDomainCodec::new(FDomain::new(namespace))));
        fuchsia_async::Task::spawn(fut).detach();
        client
    }

    /// Create a new FDomain client that points to a new local FDomain with an
    /// FDomain channel callback for the namespace.
    fn new_client_fdomain(namespace: impl Fn(Channel) + Send + 'static) -> Arc<Client> {
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let client_holder = Arc::new(OnceLock::<Weak<Client>>::new());
        let client_holder_clone = Arc::clone(&client_holder);
        let fdomain = FDomain::new_with_namespace_channel(move |server_hid| {
            if let Some(client_weak) = client_holder_clone.get() {
                if let Some(client) = client_weak.upgrade() {
                    let channel = client.channel_from_handle_id(server_hid);
                    let _ = sender.unbounded_send(channel);
                }
            }
        });
        let (client, fut) = Client::new(LocalFDomain(FDomainCodec::new(fdomain)));
        let _ = client_holder.set(Arc::downgrade(&client));
        fuchsia_async::Task::spawn(fut).detach();
        fuchsia_async::Task::spawn(async move {
            while let Some(channel) = receiver.next().await {
                namespace(channel);
            }
        })
        .detach();
        client
    }
}

impl FDomainTransport for LocalFDomain {
    fn poll_send_message(
        mut self: Pin<&mut Self>,
        msg: &[u8],
        _ctx: &mut Context<'_>,
    ) -> Poll<Result<(), Option<std::io::Error>>> {
        Poll::Ready(self.0.message(msg).map_err(|x| Some(std::io::Error::other(x))))
    }
}

impl Stream for LocalFDomain {
    type Item = Result<Box<[u8]>, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0).poll_next(cx).map_err(std::io::Error::other)
    }
}

/// Create a new FDomain client that points to a new local FDomain.
pub fn local_client(
    namespace: impl Fn() -> Result<ClientEnd<fio::DirectoryMarker>, fidl::Status> + Send + 'static,
) -> Arc<Client> {
    LocalFDomain::new_client(namespace)
}

/// Create a new FDomain client that points to a new local FDomain using a pure
/// FDomain channel callback to serve the namespace.
pub fn local_client_fdomain(namespace: impl Fn(Channel) + Send + 'static) -> Arc<Client> {
    LocalFDomain::new_client_fdomain(namespace)
}

pub fn local_client_empty() -> Arc<Client> {
    local_client(|| Err(fidl::Status::NOT_SUPPORTED))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[fuchsia::test]
    async fn test_local_client_empty() {
        let client = local_client_empty();
        assert!(client.namespace().await.is_err());
    }

    #[fuchsia::test]
    async fn test_local_client_native() {
        let client = local_client(|| {
            let (client_end, server_end) =
                fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
            fuchsia_async::Task::spawn(async move {
                let mut stream = server_end.into_stream();
                while let Some(Ok(_)) = stream.next().await {}
            })
            .detach();
            Ok(client_end)
        });

        let ns = client.namespace().await.unwrap();
        assert!(!ns.is_invalid());
    }

    #[fuchsia::test]
    async fn test_local_client_fdomain() {
        let (send_tx, mut send_rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
        let client = local_client_fdomain(move |channel| {
            let send_tx = send_tx.clone();
            fuchsia_async::Task::spawn(async move {
                let (mut stream, writer) = channel.stream().unwrap();
                while let Some(Ok(msg)) = stream.next().await {
                    let _ = send_tx.unbounded_send(msg.bytes.clone());
                    let _ = writer.write(b"pong", vec![]);
                }
            })
            .detach();
        });

        let ns = client.namespace().await.unwrap();
        ns.write(b"ping", vec![]).unwrap();
        assert_eq!(send_rx.next().await.unwrap(), b"ping");
        let (mut stream, _) = ns.stream().unwrap();
        let reply = stream.next().await.unwrap().unwrap();
        assert_eq!(&reply.bytes, b"pong");
    }

    #[fuchsia::test]
    async fn test_local_client_fdomain_sync_callback() {
        let client = local_client_fdomain(|channel| {
            channel.write(b"hello", vec![]).unwrap();
        });

        let ns = client.namespace().await.unwrap();
        let (mut stream, _) = ns.stream().unwrap();
        let reply = stream.next().await.unwrap().unwrap();
        assert_eq!(&reply.bytes, b"hello");
    }
}
