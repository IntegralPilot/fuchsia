// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_fdomain as proto;
use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::task::{Context, Poll, Waker};

use proto::f_domain_ordinals as ordinals;

/// Wraps an [`FDomain`] and provides an interface that operates on
/// binary-encoded FIDL messages, as opposed to the FIDL request/response
/// structs [`FDomain`] itself deals with.
///
/// Request messages are passed in with the [`message`] method, and polling the
/// `FDomainCodec` as a stream will yield the responses.
#[pin_project::pin_project]
pub struct FDomainCodec {
    #[pin]
    fdomain: crate::FDomain,
    outgoing: VecDeque<Box<[u8]>>,
    wakers: Vec<Waker>,
}

impl FDomainCodec {
    /// Construct a new [`FDomainCodec`] around the given [`FDomain`]
    pub fn new(fdomain: crate::FDomain) -> FDomainCodec {
        FDomainCodec { fdomain, outgoing: VecDeque::new(), wakers: Vec::new() }
    }

    /// Process an incoming message.
    pub fn message(&mut self, data: &[u8]) -> fidl::Result<()> {
        let (header, rest) = fidl_message::decode_transaction_header(data)?;
        let Some(tx_id) = NonZeroU32::new(header.tx_id) else {
            return Err(fidl::Error::UnknownOrdinal {
                ordinal: header.ordinal,
                protocol_name:
                    <proto::FDomainMarker as fidl::endpoints::ProtocolMarker>::DEBUG_NAME,
            });
        };

        match header.ordinal {
            ordinals::GET_NAMESPACE => {
                let request = fidl_message::decode_message::<proto::FDomainGetNamespaceRequest>(
                    header, rest,
                )?;
                let result = self.fdomain.get_namespace(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::CREATE_CHANNEL => {
                let request = fidl_message::decode_message::<proto::ChannelCreateChannelRequest>(
                    header, rest,
                )?;
                let result = self.fdomain.create_channel(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::CREATE_SOCKET => {
                let request =
                    fidl_message::decode_message::<proto::SocketCreateSocketRequest>(header, rest)?;
                let result = self.fdomain.create_socket(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::CREATE_EVENT_PAIR => {
                let request = fidl_message::decode_message::<proto::EventPairCreateEventPairRequest>(
                    header, rest,
                )?;
                let result = self.fdomain.create_event_pair(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::CREATE_EVENT => {
                let request =
                    fidl_message::decode_message::<proto::EventCreateEventRequest>(header, rest)?;
                let result = self.fdomain.create_event(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::SET_SOCKET_DISPOSITION => {
                let request = fidl_message::decode_message::<
                    proto::SocketSetSocketDispositionRequest,
                >(header, rest)?;
                self.fdomain.set_socket_disposition(tx_id, request);
            }
            ordinals::READ_SOCKET => {
                let request =
                    fidl_message::decode_message::<proto::SocketReadSocketRequest>(header, rest)?;
                self.fdomain.read_socket(tx_id, request);
            }
            ordinals::READ_CHANNEL => {
                let request =
                    fidl_message::decode_message::<proto::ChannelReadChannelRequest>(header, rest)?;
                self.fdomain.read_channel(tx_id, request);
            }
            ordinals::WRITE_SOCKET => {
                let request =
                    fidl_message::decode_message::<proto::SocketWriteSocketRequest>(header, rest)?;
                self.fdomain.write_socket(tx_id, request);
            }
            ordinals::WRITE_CHANNEL => {
                let request = fidl_message::decode_message::<proto::ChannelWriteChannelRequest>(
                    header, rest,
                )?;
                self.fdomain.write_channel(tx_id, request);
            }
            ordinals::WAIT_FOR_SIGNALS => {
                let request = fidl_message::decode_message::<proto::FDomainWaitForSignalsRequest>(
                    header, rest,
                )?;
                self.fdomain.wait_for_signals(tx_id, request);
            }
            ordinals::CLOSE => {
                let request =
                    fidl_message::decode_message::<proto::FDomainCloseRequest>(header, rest)?;
                self.fdomain.close(tx_id, request);
            }
            ordinals::DUPLICATE => {
                let request =
                    fidl_message::decode_message::<proto::FDomainDuplicateRequest>(header, rest)?;
                let result = self.fdomain.duplicate(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::REPLACE => {
                let request =
                    fidl_message::decode_message::<proto::FDomainReplaceRequest>(header, rest)?;
                let result = self.fdomain.replace(tx_id, request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::SIGNAL => {
                let request =
                    fidl_message::decode_message::<proto::FDomainSignalRequest>(header, rest)?;
                let result = self.fdomain.signal(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::SIGNAL_PEER => {
                let request =
                    fidl_message::decode_message::<proto::FDomainSignalPeerRequest>(header, rest)?;
                let result = self.fdomain.signal_peer(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::READ_CHANNEL_STREAMING_START => {
                let request = fidl_message::decode_message::<
                    proto::ChannelReadChannelStreamingStartRequest,
                >(header, rest)?;
                self.fdomain.read_channel_streaming_start(tx_id, request);
            }
            ordinals::READ_CHANNEL_STREAMING_STOP => {
                let request = fidl_message::decode_message::<
                    proto::ChannelReadChannelStreamingStopRequest,
                >(header, rest)?;
                self.fdomain.read_channel_streaming_stop(tx_id, request);
            }
            ordinals::READ_SOCKET_STREAMING_START => {
                let request = fidl_message::decode_message::<
                    proto::SocketReadSocketStreamingStartRequest,
                >(header, rest)?;
                self.fdomain.read_socket_streaming_start(tx_id, request);
            }
            ordinals::READ_SOCKET_STREAMING_STOP => {
                let request = fidl_message::decode_message::<
                    proto::SocketReadSocketStreamingStopRequest,
                >(header, rest)?;
                self.fdomain.read_socket_streaming_stop(tx_id, request);
            }
            ordinals::GET_KOID => {
                let request =
                    fidl_message::decode_message::<proto::FDomainGetKoidRequest>(header, rest)?;
                let result = self.fdomain.get_koid(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::CREATE_VMO => {
                let request =
                    fidl_message::decode_message::<proto::VmoCreateVmoRequest>(header, rest)?;
                let result = self.fdomain.create_vmo(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::READ_VMO => {
                let request =
                    fidl_message::decode_message::<proto::VmoReadVmoRequest>(header, rest)?;
                let result = self.fdomain.read_vmo(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::WRITE_VMO => {
                let request =
                    fidl_message::decode_message::<proto::VmoWriteVmoRequest>(header, rest)?;
                let result = self.fdomain.write_vmo(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::GET_VMO_SIZE => {
                let request =
                    fidl_message::decode_message::<proto::VmoGetVmoSizeRequest>(header, rest)?;
                let result = self.fdomain.get_vmo_size(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::SET_VMO_SIZE => {
                let request =
                    fidl_message::decode_message::<proto::VmoSetVmoSizeRequest>(header, rest)?;
                let result = self.fdomain.set_vmo_size(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::GET_VMO_STREAM_SIZE => {
                let request = fidl_message::decode_message::<proto::VmoGetVmoStreamSizeRequest>(
                    header, rest,
                )?;
                let result = self.fdomain.get_vmo_stream_size(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            ordinals::SET_VMO_STREAM_SIZE => {
                let request = fidl_message::decode_message::<proto::VmoSetVmoStreamSizeRequest>(
                    header, rest,
                )?;
                let result = self.fdomain.set_vmo_stream_size(request);
                self.send_response(tx_id, header.ordinal, result)?;
            }
            unknown if header.dynamic_flags().contains(fidl_message::DynamicFlags::FLEXIBLE) => {
                if header.tx_id != 0 {
                    let header = fidl_message::TransactionHeader::new(
                        header.tx_id,
                        unknown,
                        fidl_message::DynamicFlags::FLEXIBLE,
                    );
                    self.enqueue_outgoing::<Vec<u8>>(
                        fidl_message::encode_response_flexible_unknown(header)?.into(),
                    );
                }
            }
            _ => {
                return Err(fidl::Error::UnknownOrdinal {
                    ordinal: header.ordinal,
                    protocol_name:
                        <proto::FDomainMarker as fidl::endpoints::ProtocolMarker>::DEBUG_NAME,
                });
            }
        }

        Ok(())
    }

    /// Add an outgoing message to our outgoing queue and wake any wakers that
    /// were waiting for that.
    fn enqueue_outgoing<T: Into<Box<[u8]>>>(&mut self, msg: T) {
        self.outgoing.push_back(msg.into());
        self.wakers.drain(..).for_each(Waker::wake);
    }

    /// Encode and enqueue a fallible response message to the client. The
    /// `ordinal` field should correspond correctly to the type of message given
    /// by the type argument.
    fn send_response<T: fidl_message::Body, E: fidl_message::ErrorType>(
        &mut self,
        tx_id: NonZeroU32,
        ordinal: u64,
        body: Result<T, E>,
    ) -> fidl::Result<()>
where
    for<'a> <<T as fidl_message::Body>::MarkerInResultUnion as fidl::encoding::ValueTypeMarker>::Borrowed<'a>:
        fidl::encoding::Encode<T::MarkerInResultUnion, fidl::encoding::NoHandleResourceDialect>,
    for<'a> <<E as fidl_message::ErrorType>::Marker as fidl::encoding::ValueTypeMarker>::Borrowed<'a>:
        fidl::encoding::Encode<E::Marker, fidl::encoding::NoHandleResourceDialect>,
{
        let header = fidl_message::TransactionHeader::new(
            tx_id.into(),
            ordinal,
            fidl_message::DynamicFlags::FLEXIBLE,
        );
        self.enqueue_outgoing(fidl_message::encode_response_result::<T, E>(header, body)?);

        Ok(())
    }

    /// Encode and enqueue an event message to the client. The `ordinal` field
    /// should correspond correctly to the type of message given by the type
    /// argument.
    fn send_event<T: fidl_message::Body>(&mut self, ordinal: u64, body: T) -> fidl::Result<()>
    where
        for<'a> <<T as fidl_message::Body>::MarkerAtTopLevel as fidl::encoding::ValueTypeMarker>::Borrowed<
            'a,
        >: fidl::encoding::Encode<T::MarkerAtTopLevel, fidl::encoding::NoHandleResourceDialect>,
    {
        let header =
            fidl_message::TransactionHeader::new(0, ordinal, fidl_message::DynamicFlags::empty());
        self.enqueue_outgoing(fidl_message::encode_message(header, body)?);

        Ok(())
    }
}

impl futures::Stream for FDomainCodec {
    type Item = fidl::Result<Box<[u8]>>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        ctx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        while let Poll::Ready(Some(event)) = self.as_mut().project().fdomain.poll_next(ctx) {
            let result = match event {
                crate::FDomainEvent::ChannelStreamingReadStart(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::READ_CHANNEL_STREAMING_START, msg)
                }
                crate::FDomainEvent::ChannelStreamingReadStop(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::READ_CHANNEL_STREAMING_STOP, msg)
                }
                crate::FDomainEvent::SocketStreamingReadStart(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::READ_SOCKET_STREAMING_START, msg)
                }
                crate::FDomainEvent::SocketStreamingReadStop(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::READ_SOCKET_STREAMING_STOP, msg)
                }
                crate::FDomainEvent::WaitForSignals(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::WAIT_FOR_SIGNALS, msg)
                }
                crate::FDomainEvent::SocketData(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::READ_SOCKET, msg)
                }
                crate::FDomainEvent::SocketStreamingData(msg) => {
                    self.send_event(ordinals::ON_SOCKET_STREAMING_DATA, msg)
                }
                crate::FDomainEvent::SocketDispositionSet(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::SET_SOCKET_DISPOSITION, msg)
                }
                crate::FDomainEvent::WroteSocket(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::WRITE_SOCKET, msg)
                }
                crate::FDomainEvent::ChannelData(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::READ_CHANNEL, msg)
                }
                crate::FDomainEvent::ChannelStreamingData(msg) => {
                    self.send_event(ordinals::ON_CHANNEL_STREAMING_DATA, msg)
                }
                crate::FDomainEvent::WroteChannel(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::WRITE_CHANNEL, msg)
                }
                crate::FDomainEvent::ClosedHandle(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::CLOSE, msg)
                }
                crate::FDomainEvent::ReplacedHandle(tx_id, msg) => {
                    self.send_response(tx_id, ordinals::REPLACE, msg)
                }
            };

            if let Err(e) = result {
                return Poll::Ready(Some(Err(e)));
            }
        }

        if let Some(got) = self.outgoing.pop_front() {
            Poll::Ready(Some(Ok(got)))
        } else {
            self.wakers.push(ctx.waker().clone());
            Poll::Pending
        }
    }
}
