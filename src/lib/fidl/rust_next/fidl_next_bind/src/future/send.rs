// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, ready};

use fidl_next_codec::EncodeError;
use fidl_next_protocol::{NonBlockingTransport, Transport};
use pin_project::pin_project;

use crate::Error;

macro_rules! define_send_future {
    (
        $(#[$metas:meta])*
        $name:ident<$($lifetime:lifetime,)? T: Transport>($future:ty) => $encoded:ident {
            state = $state:ident,
            proj = $proj:ident,
            own = $own:ident,
        }
    ) => {
        #[pin_project(project = $proj, project_replace = $own)]
        enum $state<$($lifetime,)? T: Transport> {
            EncodeError(EncodeError),
            Sending(#[pin] $future),
            Finished,
        }

        impl<$($lifetime,)? T: Transport> $state<$($lifetime,)? T> {
            fn poll_state(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Result<(), Error<T::Error>>> {
                match self.as_mut().project() {
                    $proj::EncodeError(_) => {
                        let state = self.project_replace(Self::Finished);
                        let $own::EncodeError(error) = state else {
                            unreachable!();
                        };
                        Poll::Ready(Err(Error::Encode(error)))
                    }
                    $proj::Sending(future) => match ready!(future.poll(cx)) {
                        Ok(()) => Poll::Ready(Ok(())),
                        Err(error) => Poll::Ready(Err(Error::Protocol(error))),
                    },
                    $proj::Finished => panic!("State polled after completing"),
                }
            }

            fn send_immediately(self) -> Result<(), Error<T::Error>>
            where
                T: NonBlockingTransport,
            {
                match self {
                    Self::EncodeError(e) => return Err(e.into()),
                    Self::Sending(future) => future.send_immediately()?,
                    Self::Finished => unreachable!(),
                }

                Ok(())
            }
        }

        $(#[$metas])*
        #[must_use = "futures do nothing unless polled"]
        #[pin_project]
        pub struct $name<
            $($lifetime,)?
            T: Transport,
        > {
            #[pin]
            state: $state<$($lifetime,)? T>,
        }

        impl<$($lifetime,)? T: Transport> $name<$($lifetime,)? T> {
            #[doc = concat!("Returns a `", stringify!($name), "` wrapping the given result.")]
            pub fn from_untyped(result: Result<$future, EncodeError>) -> Self {
                Self {
                    state: match result {
                        Err(error) => $state::EncodeError(error),
                        Ok(future) => $state::Sending(future),
                    },
                }
            }

            /// Encodes the message.
            ///
            /// Returns a future which sends the message, or an error if it failed.
            pub fn encode(self) -> Result<$encoded<$($lifetime,)? T>, Error<T::Error>> {
                Ok($encoded {
                    state: match self.state {
                        $state::EncodeError(error) => return Err(Error::Encode(error)),
                        state => state,
                    },
                })
            }

            /// Completes the send operation synchronously and without blocking.
            ///
            /// Using this method prevents transports from applying
            /// backpressure. Prefer awaiting when possible to allow for
            /// backpressure.
            ///
            /// Because failed sends return immediately, `send_immediately` may
            /// observe transport closure prematurely. This can manifest as this
            /// method returning `Err(PeerClosed)` or `Err(Stopped)` when it
            /// should have returned `Err(PeerClosedWithEpitaph)`. Prefer
            /// awaiting when possible for correctness.
            pub fn send_immediately(self) -> Result<(), Error<T::Error>>
            where
                T: NonBlockingTransport,
            {
                self.state.send_immediately()
            }
        }

        impl<'a, T: Transport> Future for $name<$($lifetime,)? T> {
            type Output = Result<(), Error<T::Error>>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                self.project().state.poll_state(cx)
            }
        }

        #[doc = concat!("An encoded `", stringify!($name), "`.")]
        ///
        /// This future has already been successfully encoded. It still needs to be
        /// sent.
        #[must_use = "futures do nothing unless polled"]
        #[pin_project]
        pub struct $encoded<
            $($lifetime,)?
            T: Transport,
        > {
            #[pin]
            state: $state<$($lifetime,)? T>,
        }

        impl<$($lifetime,)? T: Transport> Future for $encoded<$($lifetime,)? T> {
            type Output = Result<(), Error<T::Error>>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                self.project().state.poll_state(cx)
            }
        }
    }
}

define_send_future! {
    /// A future which sends an encoded message to a connection.
    SendFuture<'a, T: Transport>(fidl_next_protocol::SendFuture<'a, T>) => EncodedSendFuture {
        state = SendFutureState,
        proj = SendFutureProj,
        own = SendFutureOwn,
    }
}

define_send_future! {
    /// A future which responds to a request with an encoded message.
    RespondFuture<T: Transport>(fidl_next_protocol::RespondFuture<T>) => EncodedRespondFuture {
        state = RespondFutureState,
        proj = RespondFutureProj,
        own = RespondFutureOwn,
    }
}
