// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_next_codec::AsDecoderExt as _;
use fuchsia_async::Task;

use crate::{
    Client, ClientDispatcher, ClientHandler, Flexibility, Message, NonBlockingTransport,
    ProtocolError, Responder, ServerDispatcher, ServerHandler, Transport,
};

mod wire {
    use core::mem::MaybeUninit;

    use fidl_next_codec::{
        Constrained, Decode, DecodeError, Decoder, Encode, EncodeError, Encoder, Slot,
        ValidationError, Wire, munge, wire,
    };

    #[repr(transparent)]
    pub struct TestMessage<'de>(wire::String<'de>);

    impl<'de> TestMessage<'de> {
        pub fn as_str(&'de self) -> &'de str {
            self.0.as_str()
        }
    }

    impl Constrained for TestMessage<'_> {
        type Constraint = ();

        fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
            Ok(())
        }
    }

    unsafe impl Wire for TestMessage<'static> {
        type Narrowed<'de> = TestMessage<'de>;

        fn zero_padding(out: &mut MaybeUninit<Self>) {
            munge!(let Self (s) = out);
            wire::String::zero_padding(s);
        }
    }

    unsafe impl<E: Encoder + ?Sized> Encode<TestMessage<'static>, E> for super::TestMessage {
        fn encode(
            self,
            encoder: &mut E,
            out: &mut MaybeUninit<TestMessage<'static>>,
            _: (),
        ) -> Result<(), EncodeError> {
            munge!(let TestMessage (s) = out);
            self.0.as_str().encode(encoder, s, 1000)
        }
    }

    unsafe impl<'de, D: Decoder<'de>> Decode<D> for TestMessage<'de> {
        fn decode(slot: Slot<'_, Self>, decoder: &mut D, _: ()) -> Result<(), DecodeError> {
            munge!(let Self (s) = slot);
            wire::String::decode(s, decoder, 1000)
        }
    }
}

struct TestMessage(String);
impl TestMessage {
    fn new(s: &str) -> Self {
        Self(s.to_string())
    }
}

struct IgnoreEvents;

impl<T: Transport> ClientHandler<T> for IgnoreEvents {
    async fn on_event(&mut self, _: Message<T>) -> Result<(), ProtocolError<T::Error>> {
        Ok(())
    }
}

pub async fn test_close_on_drop<T>(make_ends: impl FnOnce() -> (T, T))
where
    T: Transport + 'static,
{
    struct TestServer;

    impl<T: Transport> ServerHandler<T> for TestServer {
        async fn on_one_way(&mut self, _: Message<T>) -> Result<(), ProtocolError<T::Error>> {
            panic!("unexpected event");
        }

        async fn on_two_way(
            &mut self,
            mut message: Message<T>,
            responder: Responder<T>,
        ) -> Result<(), ProtocolError<T::Error>> {
            assert_eq!(*message.header().ordinal, 42);
            let request =
                message.into_decoded::<wire::TestMessage<'_>>().expect("failed to decode request");
            assert_eq!(request.as_str(), "Ping");

            responder
                .respond(42, Flexibility::Strict, TestMessage::new("Pong"))
                .expect("failed to encode response")
                .await
                .expect("failed to send response");

            Ok(())
        }
    }

    let (client_end, server_end) = make_ends();
    let client_dispatcher = ClientDispatcher::new(client_end);
    let client = client_dispatcher.client();
    let client_task = Task::spawn(client_dispatcher.run(IgnoreEvents));
    let server_task = Task::spawn(ServerDispatcher::new(server_end).run(TestServer));

    let message = client
        .send_two_way(42, Flexibility::Strict, TestMessage::new("Ping"))
        .expect("client failed to encode request")
        .await
        .expect("client failed to send request")
        .await
        .expect("client failed to receive response")
        .into_decoded::<wire::TestMessage<'_>>()
        .expect("failed to decode response");
    assert_eq!(message.as_str(), "Pong");

    // Dropping the last client should close the connection.
    drop(client);

    client_task.await.expect("client encountered an error");
    server_task.await.expect("server encountered an error");
}

pub async fn test_one_way<T>(make_ends: impl FnOnce() -> (T, T))
where
    T: Transport + 'static,
{
    struct TestServer;

    impl<T: Transport> ServerHandler<T> for TestServer {
        async fn on_one_way(
            &mut self,
            mut message: Message<T>,
        ) -> Result<(), ProtocolError<T::Error>> {
            assert_eq!(*message.header().ordinal, 42);
            let request =
                message.into_decoded::<wire::TestMessage<'_>>().expect("failed to decode request");
            assert_eq!(request.as_str(), "Hello world");

            Ok(())
        }

        async fn on_two_way(
            &mut self,
            _: Message<T>,
            _: Responder<T>,
        ) -> Result<(), ProtocolError<T::Error>> {
            panic!("unexpected two-way message");
        }
    }

    let (client_end, server_end) = make_ends();
    let client_dispatcher = ClientDispatcher::new(client_end);
    let client = client_dispatcher.client();
    let client_task = Task::spawn(client_dispatcher.run(IgnoreEvents));
    let server_task = Task::spawn(ServerDispatcher::new(server_end).run(TestServer));

    client
        .send_one_way(42, Flexibility::Strict, TestMessage::new("Hello world"))
        .expect("client failed to encode request")
        .await
        .expect("client failed to send request");

    client.close();

    client_task.await.expect("client encountered an error");
    server_task.await.expect("server encountered an error");
}

pub async fn test_two_way<T>(make_ends: impl FnOnce() -> (T, T))
where
    T: Transport + 'static,
{
    struct TestServer;

    impl<T: Transport> ServerHandler<T> for TestServer {
        async fn on_one_way(&mut self, _: Message<T>) -> Result<(), ProtocolError<T::Error>> {
            panic!("unexpected event");
        }

        async fn on_two_way(
            &mut self,
            mut message: Message<T>,
            responder: Responder<T>,
        ) -> Result<(), ProtocolError<T::Error>> {
            assert_eq!(*message.header().ordinal, 42);
            let request =
                message.into_decoded::<wire::TestMessage<'_>>().expect("failed to decode request");
            assert_eq!(request.as_str(), "Ping");

            responder
                .respond(42, Flexibility::Strict, TestMessage::new("Pong"))
                .expect("failed to encode response")
                .await
                .expect("failed to send response");

            Ok(())
        }
    }

    let (client_end, server_end) = make_ends();
    let client_dispatcher = ClientDispatcher::new(client_end);
    let client = client_dispatcher.client();
    let client_task = Task::spawn(client_dispatcher.run(IgnoreEvents));
    let server_task = Task::spawn(ServerDispatcher::new(server_end).run(TestServer));

    let message = client
        .send_two_way(42, Flexibility::Strict, TestMessage::new("Ping"))
        .expect("client failed to encode request")
        .await
        .expect("client failed to send request")
        .await
        .expect("client failed to receive response")
        .into_decoded::<wire::TestMessage<'_>>()
        .expect("failed to decode response");
    assert_eq!(message.as_str(), "Pong");

    client.close();

    client_task.await.expect("client encountered an error");
    server_task.await.expect("server encountered an error");
}

pub async fn test_multiple_two_way<T>(make_ends: impl FnOnce() -> (T, T))
where
    T: Transport + 'static,
{
    struct TestServer;

    impl<T: Transport> ServerHandler<T> for TestServer {
        async fn on_one_way(&mut self, _: Message<T>) -> Result<(), ProtocolError<T::Error>> {
            panic!("unexpected event");
        }

        async fn on_two_way(
            &mut self,
            mut message: Message<T>,
            responder: Responder<T>,
        ) -> Result<(), ProtocolError<T::Error>> {
            let ordinal = *message.header().ordinal;
            let request =
                message.into_decoded::<wire::TestMessage<'_>>().expect("failed to decode request");

            let response = match ordinal {
                1 => "One",
                2 => "Two",
                3 => "Three",
                x => panic!("unexpected request ordinal {x} from client"),
            };

            assert_eq!(request.as_str(), response);

            responder
                .respond(ordinal, Flexibility::Strict, TestMessage::new(response))
                .expect("server failed to encode response")
                .await
                .expect("server failed to send response");

            Ok(())
        }
    }

    let (client_end, server_end) = make_ends();
    let client_dispatcher = ClientDispatcher::new(client_end);
    let client = client_dispatcher.client();
    let client_task = Task::spawn(client_dispatcher.run(IgnoreEvents));
    let server_task = Task::spawn(ServerDispatcher::new(server_end).run(TestServer));

    let send_one = client
        .send_two_way(1, Flexibility::Strict, TestMessage::new("One"))
        .expect("client failed to encode request")
        .await
        .expect("client failed to send request");
    let send_two = client
        .send_two_way(2, Flexibility::Strict, TestMessage::new("Two"))
        .expect("client failed to encode request")
        .await
        .expect("client failed to send request");
    let send_three = client
        .send_two_way(3, Flexibility::Strict, TestMessage::new("Three"))
        .expect("client failed to encode request")
        .await
        .expect("client failed to send request");
    let (response_one, response_two, response_three) =
        futures::join!(send_one, send_two, send_three);

    let message_one = response_one
        .expect("client failed to receive response")
        .into_decoded::<wire::TestMessage<'_>>()
        .expect("failed to decode response");
    assert_eq!(message_one.as_str(), "One");

    let message_two = response_two
        .expect("client failed to receive response")
        .into_decoded::<wire::TestMessage<'_>>()
        .expect("failed to decode response");
    assert_eq!(message_two.as_str(), "Two");

    let message_three = response_three
        .expect("client failed to receive response")
        .into_decoded::<wire::TestMessage<'_>>()
        .expect("failed to decode response");
    assert_eq!(message_three.as_str(), "Three");

    client.close();

    client_task.await.expect("client encountered an error");
    server_task.await.expect("server encountered an error");
}

pub async fn test_event<T>(make_ends: impl FnOnce() -> (T, T))
where
    T: Transport + 'static,
{
    struct TestClient<T: Transport> {
        client: Client<T>,
    }

    impl<T: Transport> ClientHandler<T> for TestClient<T> {
        async fn on_event(
            &mut self,
            mut message: Message<T>,
        ) -> Result<(), ProtocolError<T::Error>> {
            assert_eq!(*message.header().ordinal, 10);
            let event =
                message.into_decoded::<wire::TestMessage<'_>>().expect("failed to decode request");
            assert_eq!(event.as_str(), "Surprise!");

            self.client.close();

            Ok(())
        }
    }

    pub struct TestServer;

    impl<T: Transport> ServerHandler<T> for TestServer {
        async fn on_one_way(&mut self, _: Message<T>) -> Result<(), ProtocolError<T::Error>> {
            Ok(())
        }

        async fn on_two_way(
            &mut self,
            _: Message<T>,
            _: Responder<T>,
        ) -> Result<(), ProtocolError<T::Error>> {
            Ok(())
        }
    }

    let (client_end, server_end) = make_ends();
    let client_dispatcher = ClientDispatcher::new(client_end);
    let client = client_dispatcher.client();
    let client_task = Task::spawn(client_dispatcher.run(TestClient { client }));
    let server_dispatcher = ServerDispatcher::new(server_end);
    let server = server_dispatcher.server();
    let server_task = Task::spawn(server_dispatcher.run(TestServer));

    server
        .send_event(10, Flexibility::Strict, TestMessage::new("Surprise!"))
        .expect("server failed to encode response")
        .await
        .expect("server failed to send response");

    client_task.await.expect("client encountered an error");
    server_task.await.expect("server encountered an error");
}

pub async fn test_one_way_nonblocking<T>(make_ends: impl FnOnce() -> (T, T))
where
    T: NonBlockingTransport + 'static,
{
    struct TestServer;

    impl<T: Transport> ServerHandler<T> for TestServer {
        async fn on_one_way(
            &mut self,
            mut message: Message<T>,
        ) -> Result<(), ProtocolError<T::Error>> {
            assert_eq!(*message.header().ordinal, 42);
            let request =
                message.into_decoded::<wire::TestMessage<'_>>().expect("failed to decode request");
            assert_eq!(request.as_str(), "Hello world");

            Ok(())
        }

        async fn on_two_way(
            &mut self,
            _: Message<T>,
            _: Responder<T>,
        ) -> Result<(), ProtocolError<T::Error>> {
            panic!("unexpected two-way message");
        }
    }

    let (client_end, server_end) = make_ends();
    let client_dispatcher = ClientDispatcher::new(client_end);
    let client = client_dispatcher.client();
    let client_task = Task::spawn(client_dispatcher.run(IgnoreEvents));
    let server_task = Task::spawn(ServerDispatcher::new(server_end).run(TestServer));

    client
        .send_one_way(42, Flexibility::Strict, TestMessage::new("Hello world"))
        .expect("client failed to encode request")
        .send_immediately()
        .expect("client failed to send request");

    client.close();

    client_task.await.expect("client encountered an error");
    server_task.await.expect("server encountered an error");
}
