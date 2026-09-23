// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::{NonZeroU32, TryFromIntError};
use std::time::{Duration, Instant};

use flex_fuchsia_developer_ffx_speedtest as fspeedtest;
use futures::AsyncReadExt;
#[cfg(not(feature = "fdomain"))]
use futures::AsyncWriteExt;
#[cfg(feature = "fdomain")]
use futures::StreamExt;
use thiserror::Error;

pub struct Transfer {
    pub socket: flex_client::Socket,
    pub params: TransferParams,
}

#[derive(Debug, Clone)]
pub struct TransferParams {
    pub data_len: NonZeroU32,
    pub buffer_len: NonZeroU32,
    #[cfg(feature = "fdomain")]
    pub fdomain_params: FDomainTransferParams,
}

#[cfg(feature = "fdomain")]
#[derive(Debug, Clone)]
pub struct FDomainTransferParams {
    pub streaming_read: bool,
    pub writes_in_flight: NonZeroU32,
}

impl TryFrom<fspeedtest::TransferParams> for TransferParams {
    type Error = TryFromIntError;
    fn try_from(value: fspeedtest::TransferParams) -> Result<Self, Self::Error> {
        let fspeedtest::TransferParams { len_bytes, buffer_bytes, __source_breaking } = value;
        Ok(Self {
            data_len: len_bytes.unwrap_or(fspeedtest::DEFAULT_TRANSFER_SIZE).try_into()?,
            buffer_len: buffer_bytes.unwrap_or(fspeedtest::DEFAULT_BUFFER_SIZE).try_into()?,
            #[cfg(feature = "fdomain")]
            fdomain_params: FDomainTransferParams {
                streaming_read: false,
                writes_in_flight: NonZeroU32::new(1).unwrap(),
            },
        })
    }
}

impl TryFrom<TransferParams> for fspeedtest::TransferParams {
    type Error = TryFromIntError;
    fn try_from(value: TransferParams) -> Result<Self, Self::Error> {
        let TransferParams { data_len, buffer_len, .. } = value;
        Ok(Self {
            len_bytes: Some(data_len.try_into()?),
            buffer_bytes: Some(buffer_len.try_into()?),
            __source_breaking: fidl::marker::SourceBreaking,
        })
    }
}

#[derive(Debug)]
pub struct Report {
    pub duration: Duration,
}

impl From<Report> for fspeedtest::TransferReport {
    fn from(value: Report) -> Self {
        let Report { duration } = value;
        Self {
            duration_nsec: Some(duration.as_nanos().try_into().unwrap_or(u64::MAX)),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

#[derive(Error, Debug)]
#[error("missing mandatory field")]
pub struct MissingFieldError;

impl TryFrom<fspeedtest::TransferReport> for Report {
    type Error = MissingFieldError;

    fn try_from(value: fspeedtest::TransferReport) -> Result<Self, Self::Error> {
        let fspeedtest::TransferReport { duration_nsec, __source_breaking } = value;
        Ok(Self { duration: Duration::from_nanos(duration_nsec.ok_or(MissingFieldError)?) })
    }
}

#[derive(Error, Debug)]
pub enum TransferError {
    #[error(transparent)]
    IntConversion(#[from] TryFromIntError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    FDomain(#[from] fdomain_client::Error),
    #[error("remote hung up before terminating transfer")]
    Hangup,
}

enum ReadSocket {
    Normal(flex_client::AsyncSocket),
    #[cfg(feature = "fdomain")]
    Stream(flex_client::SocketReadStream),
}

impl ReadSocket {
    fn from_socket(socket: flex_client::AsyncSocket, stream: bool) -> Result<Self, TransferError> {
        #[cfg(feature = "fdomain")]
        if stream {
            let (socket, _) = socket.stream()?;
            return Ok(ReadSocket::Stream(socket));
        }

        debug_assert!(!stream);
        Ok(ReadSocket::Normal(socket))
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransferError> {
        let bytes = match self {
            ReadSocket::Normal(s) => s.read(buf).await?,
            #[cfg(feature = "fdomain")]
            ReadSocket::Stream(s) => s.read(buf).await?,
        };
        Ok(bytes)
    }
}

impl Transfer {
    #[cfg(not(feature = "fdomain"))]
    pub async fn send(self) -> Result<Report, TransferError> {
        let Self { socket, params: TransferParams { data_len, buffer_len } } = self;
        let mut socket = flex_client::socket_to_async(socket);
        let buffer_len = usize::try_from(buffer_len.get())?;
        let mut data_len = usize::try_from(data_len.get())?;
        let buffer = vec![0xAA; buffer_len];
        let start = Instant::now();
        while data_len != 0 {
            let send = buffer_len.min(data_len);
            let written = socket.write(&buffer[..send]).await?;
            data_len -= written;
        }
        let end = Instant::now();
        Ok(Report { duration: end - start })
    }

    #[cfg(feature = "fdomain")]
    pub async fn send(self) -> Result<Report, TransferError> {
        let Self {
            socket,
            params:
                TransferParams {
                    data_len,
                    buffer_len,
                    fdomain_params: FDomainTransferParams { writes_in_flight, .. },
                },
        } = self;
        let buffer_len = usize::try_from(buffer_len.get())?;
        let mut data_len = usize::try_from(data_len.get())?;
        let buffer = vec![0xAA; buffer_len];
        let start = Instant::now();

        let mut stream = futures::stream::iter(std::iter::from_fn(|| {
            if data_len == 0 {
                return None;
            }

            let send = buffer_len.min(data_len);
            data_len -= send;
            Some(socket.fdomain_write_all(&buffer[..send]))
        }))
        .buffered(writes_in_flight.get() as usize);

        while let Some(res) = stream.next().await {
            let _: () = res?;
        }

        let end = Instant::now();
        Ok(Report { duration: end - start })
    }

    pub async fn receive(self) -> Result<Report, TransferError> {
        let Self {
            socket,
            params:
                TransferParams {
                    data_len,
                    buffer_len,
                    #[cfg(feature = "fdomain")]
                        fdomain_params: FDomainTransferParams { streaming_read, .. },
                },
        } = self;
        #[cfg(not(feature = "fdomain"))]
        let streaming_read = false;
        let mut socket =
            ReadSocket::from_socket(flex_client::socket_to_async(socket), streaming_read)?;
        let buffer_len = usize::try_from(buffer_len.get())?;
        let mut data_len = usize::try_from(data_len.get())?;
        let mut buffer = vec![0x00; buffer_len];
        let start = Instant::now();

        while data_len != 0 {
            let recv = buffer_len.min(data_len);
            let recv = socket.read(&mut buffer[..recv]).await?;
            if recv == 0 {
                return Err(TransferError::Hangup);
            }
            data_len -= recv;
        }
        let end = Instant::now();
        Ok(Report { duration: end - start })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use assert_matches::assert_matches;

    #[fuchsia::test]
    async fn receive_hangup() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        #[cfg(not(feature = "fdomain"))]
        let client = fidl::endpoints::ZirconClient;
        let (socket, _) = client.create_stream_socket();
        let result = Transfer {
            socket,
            params: TransferParams {
                data_len: NonZeroU32::new(10).unwrap(),
                buffer_len: NonZeroU32::new(100).unwrap(),
                #[cfg(feature = "fdomain")]
                fdomain_params: FDomainTransferParams {
                    streaming_read: false,
                    writes_in_flight: NonZeroU32::new(1).unwrap(),
                },
            },
        }
        .receive()
        .await;

        assert_matches!(result, Err(TransferError::Hangup));
    }
}
