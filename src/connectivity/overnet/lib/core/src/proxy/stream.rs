// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::handle::{Message, Proxyable, ProxyableHandle, RouterHolder, Serializer};
use crate::coding::{decode_fidl, encode_fidl};
use crate::labels::{NodeId, TransferKey};
use crate::peer::{
    FrameType, FramedStreamReadResult, FramedStreamReader, FramedStreamWriter, PeerConn,
    PeerConnRef,
};
use crate::router::Router;
use anyhow::{Context as _, Error, format_err};
use fidl_fuchsia_overnet_protocol::{BeginTransfer, Empty, SignalUpdate, StreamControl};
use futures::future::{BoxFuture, poll_fn};
use futures::prelude::*;
use futures::ready;
use std::pin::Pin;
use std::sync::Weak;
use std::task::{Context, Poll};
use zx_status;

pub(crate) struct StreamWriter<Msg: Message> {
    stream: FramedStreamWriter,
    send_buffer: Vec<u8>,
    router: Weak<Router>,
    closed: bool,
    _phantom_msg: std::marker::PhantomData<Msg>,
}

impl<Msg: Message> StreamWriter<Msg> {
    pub fn conn(&self) -> PeerConnRef<'_> {
        self.stream.conn()
    }

    pub fn id(&self) -> u64 {
        self.stream.id()
    }

    pub async fn send_data(&mut self, msg: &mut Msg) -> Result<(), Error> {
        assert_ne!(self.closed, true);
        let mut s = Msg::Serializer::new();
        let send_buffer = &mut self.send_buffer;
        let conn = self.stream.conn();
        let mut rh = RouterHolder::Unused(&self.router);
        poll_fn(|fut_ctx| s.poll_ser(msg, send_buffer, conn, &mut rh, fut_ctx))
            .await
            .with_context(|| format_err!("Serializing message {:?}", msg))?;
        self.stream
            .send(FrameType::Data, &self.send_buffer)
            .await
            .with_context(|| format_err!("sending data {:?} ser={:?}", msg, self.send_buffer))
    }

    async fn send_control(&mut self, mut msg: StreamControl, fin: bool) -> Result<(), Error> {
        assert_ne!(self.closed, true);
        let msg = encode_fidl(&mut msg)
            .with_context(|| format_err!("encoding control message {:?}", msg))?;
        if fin {
            self.closed = true;
        }
        self.stream
            .send(FrameType::Control, msg.as_slice())
            .await
            .with_context(|| format_err!("sending control message {:?}", msg))
    }

    pub async fn send_signal(&mut self, mut msg: SignalUpdate) -> Result<(), Error> {
        assert_ne!(self.closed, true);
        let msg = encode_fidl(&mut msg)
            .with_context(|| format_err!("encoding control message {:?}", msg))?;
        self.stream
            .send(FrameType::Signal, msg.as_slice())
            .await
            .with_context(|| format_err!("sending control message {:?}", msg))
    }

    pub async fn send_ack_transfer(mut self) -> Result<(), Error> {
        Ok(self.send_control(StreamControl::AckTransfer(Empty {}), true).await?)
    }

    pub async fn send_end_transfer(mut self) -> Result<(), Error> {
        Ok(self.send_control(StreamControl::EndTransfer(Empty {}), true).await?)
    }

    pub async fn send_begin_transfer(
        &mut self,
        new_destination_node: NodeId,
        transfer_key: TransferKey,
    ) -> Result<(), Error> {
        Ok(self
            .send_control(
                StreamControl::BeginTransfer(BeginTransfer {
                    new_destination_node: new_destination_node.into(),
                    transfer_key,
                }),
                false,
            )
            .await?)
    }

    pub async fn send_hello(&mut self) -> Result<(), Error> {
        self.stream.send(FrameType::Hello, &[]).await.with_context(|| format_err!("sending hello"))
    }

    pub async fn send_shutdown(mut self, r: Result<(), zx_status::Status>) -> Result<(), Error> {
        self.send_control(StreamControl::Shutdown(zx_status::Status::result_into_raw(r)), true)
            .await
    }
}

pub(crate) trait StreamWriterBinder {
    fn bind<Msg: Message, H: Proxyable<Message = Msg>>(
        self,
        hdl: &ProxyableHandle<H>,
    ) -> StreamWriter<Msg>;
}

impl StreamWriterBinder for FramedStreamWriter {
    fn bind<Msg: Message, H: Proxyable<Message = Msg>>(
        self,
        hdl: &ProxyableHandle<H>,
    ) -> StreamWriter<Msg> {
        StreamWriter {
            stream: self,
            send_buffer: Vec::new(),
            router: hdl.router().clone(),
            closed: false,
            _phantom_msg: std::marker::PhantomData,
        }
    }
}

#[derive(PartialEq, Debug)]
pub(crate) enum Frame<'a, Msg: Message> {
    Hello,
    Data(&'a mut Msg),
    SignalUpdate(SignalUpdate),
    BeginTransfer(NodeId, TransferKey),
    AckTransfer,
    EndTransfer,
    Shutdown(Result<(), zx_status::Status>),
}

#[derive(Debug)]
pub(crate) struct StreamReader<Msg: Message> {
    stream: FramedStreamReader,
    incoming_message: Msg,
    router: Weak<Router>,
    state: ReadNextState<Msg::Parser>,
}

#[derive(Debug)]
pub(crate) struct ReadNext<'a, Msg: Message> {
    read_next_frame_or_peer_conn_ref: ReadNextFrameOrPeerConnRef<'a>,
    state: &'a mut ReadNextState<Msg::Parser>,
    conn: PeerConn,
    incoming_message: Option<&'a mut Msg>,
    router_holder: RouterHolder<'a>,
}

enum ReadNextFrameOrPeerConnRef<'a> {
    ReadNextFrame(BoxFuture<'a, Result<FramedStreamReadResult, Error>>),
    PeerConnRef(PeerConnRef<'a>),
}

impl std::fmt::Debug for ReadNextFrameOrPeerConnRef<'_> {
    fn fmt(&self, writer: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            Self::ReadNextFrame(_) => {
                write!(writer, "ReadNextFrameOrPeerConnRef::ReadNextFrame(...)")
            }
            Self::PeerConnRef(x) => {
                write!(writer, "ReadNextFrameOrPeerConnRef::PeerConnRef({x:?})")
            }
        }
    }
}

impl<'a> ReadNextFrameOrPeerConnRef<'a> {
    fn as_read_next_frame_mut(
        &mut self,
    ) -> Option<&mut BoxFuture<'a, Result<FramedStreamReadResult, Error>>> {
        match self {
            Self::ReadNextFrame(x) => Some(x),
            _ => None,
        }
    }
}

#[derive(Debug)]
enum ReadNextState<Parser> {
    Reading,
    DeserializingData(Vec<u8>, Parser),
}

impl<'a, Msg: Message> ReadNext<'a, Msg> {
    fn poll_inner(&mut self, ctx: &mut Context<'_>) -> Poll<Result<Frame<'a, Msg>, Error>> {
        loop {
            return Poll::Ready(Ok(match *self.state {
                ReadNextState::Reading => {
                    let (frame_type, mut bytes) = match ready!(
                        self.read_next_frame_or_peer_conn_ref
                            .as_read_next_frame_mut()
                            .unwrap()
                            .poll_unpin(ctx)
                    )? {
                        FramedStreamReadResult::Frame(frame_type, bytes) => (frame_type, bytes),
                        FramedStreamReadResult::Closed(Some(e)) => {
                            return Poll::Ready(Err(format_err!("unexpected end of stream ({e})")));
                        }
                        FramedStreamReadResult::Closed(None) => {
                            return Poll::Ready(Err(format_err!("unexpected end of stream")));
                        }
                    };

                    match frame_type {
                        FrameType::Hello => {
                            if bytes.len() != 0 {
                                return Poll::Ready(Err(format_err!("Hello frame must be empty")));
                            }
                            Frame::Hello
                        }
                        FrameType::Data => {
                            *self.state =
                                ReadNextState::DeserializingData(bytes, Msg::Parser::new());
                            continue;
                        }
                        FrameType::Signal => Frame::SignalUpdate(decode_fidl(&mut bytes)?),
                        FrameType::Control => match decode_fidl(&mut bytes)? {
                            StreamControl::AckTransfer(Empty {}) => Frame::AckTransfer,
                            StreamControl::EndTransfer(Empty {}) => Frame::EndTransfer,
                            StreamControl::Shutdown(status_code) => {
                                Frame::Shutdown(zx_status::Status::ok(status_code))
                            }

                            StreamControl::BeginTransfer(BeginTransfer {
                                new_destination_node,
                                transfer_key,
                            }) => Frame::BeginTransfer(new_destination_node.into(), transfer_key),
                        },
                    }
                }
                ReadNextState::DeserializingData(ref mut bytes, ref mut parser) => {
                    ready!(parser.poll_ser(
                        self.incoming_message.as_mut().unwrap(),
                        bytes,
                        self.conn.as_ref(),
                        &mut self.router_holder,
                        ctx,
                    ))?;
                    *self.state = ReadNextState::Reading;
                    Frame::Data(self.incoming_message.take().unwrap())
                }
            }));
        }
    }
}

impl<'a, Msg: Message> Future for ReadNext<'a, Msg> {
    type Output = Result<Frame<'a, Msg>, Error>;
    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::into_inner(self).poll_inner(ctx)
    }
}

impl<Msg: Message> StreamReader<Msg> {
    pub fn conn(&self) -> PeerConnRef<'_> {
        self.stream.conn()
    }

    pub fn is_initiator(&self) -> bool {
        self.stream.is_initiator()
    }

    pub fn next<'a>(&'a mut self) -> ReadNext<'a, Msg> {
        let conn = self.stream.conn().into_peer_conn();
        ReadNext {
            read_next_frame_or_peer_conn_ref: match self.state {
                ReadNextState::Reading => {
                    ReadNextFrameOrPeerConnRef::ReadNextFrame(self.stream.next().boxed())
                }
                ReadNextState::DeserializingData(_, _) => {
                    ReadNextFrameOrPeerConnRef::PeerConnRef(self.stream.conn())
                }
            },
            state: &mut self.state,
            conn,
            incoming_message: Some(&mut self.incoming_message),
            router_holder: RouterHolder::Unused(&self.router),
        }
    }

    async fn expect(&mut self, frame: Frame<'_, Msg>) -> Result<(), Error> {
        let received = self.next().await?;
        if received != frame {
            let msg = format_err!("Expected {:?} got {:?}", frame, received);
            self.stream.abandon().await;
            Err(msg)
        } else {
            Ok(())
        }
    }

    pub async fn expect_ack_transfer(mut self) -> Result<(), Error> {
        self.expect(Frame::AckTransfer).await
    }

    pub async fn expect_hello(&mut self) -> Result<(), Error> {
        self.expect(Frame::Hello).await
    }

    pub async fn expect_shutdown(
        mut self,
        result: Result<(), zx_status::Status>,
    ) -> Result<(), Error> {
        self.expect(Frame::Shutdown(result)).await
    }
}

pub(crate) trait StreamReaderBinder {
    fn bind<Msg: Message, H: Proxyable<Message = Msg>>(
        self,
        hdl: &ProxyableHandle<H>,
    ) -> StreamReader<Msg>;
}

impl StreamReaderBinder for FramedStreamReader {
    fn bind<Msg: Message, H: Proxyable<Message = Msg>>(
        self,
        hdl: &ProxyableHandle<H>,
    ) -> StreamReader<Msg> {
        StreamReader {
            stream: self,
            incoming_message: Default::default(),
            router: hdl.router().clone(),
            state: ReadNextState::Reading,
        }
    }
}
