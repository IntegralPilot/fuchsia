// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async as fasync;
use fuchsia_fs::file::{AsyncGetSize, AsyncReadAt};
use futures::FutureExt as _;
use futures::io::{AsyncReadExt as _, AsyncWriteExt as _};
use std::pin::Pin;
use std::task::{Context, Poll};

const SIDELOAD_DONE: &[u8; 8] = b"DONEDONE";

pub struct SideloadFile {
    file_size: u64,
    block_size: usize,
    read_at_state: ReadAtState,
}

enum ReadAtState {
    Idle {
        socket: fasync::Socket,
        data: Vec<u8>,
    },
    Reading {
        fut: futures::future::BoxFuture<'static, std::io::Result<(fasync::Socket, Vec<u8>)>>,
        block: u64,
    },
    Ready {
        socket: fasync::Socket,
        data: Vec<u8>,
        block: u64,
    },
}

impl SideloadFile {
    pub fn new(socket: fasync::Socket, file_size: u64, block_size: usize) -> Self {
        Self {
            file_size,
            block_size,
            read_at_state: ReadAtState::Idle { socket, data: vec![0; block_size] },
        }
    }

    pub async fn close(self) -> std::io::Result<()> {
        let mut socket = match self.read_at_state {
            ReadAtState::Idle { socket, .. } => socket,
            ReadAtState::Reading { fut, .. } => fut.await?.0,
            ReadAtState::Ready { socket, .. } => socket,
        };

        let () = socket.write_all(SIDELOAD_DONE).await?;
        let () = socket.close().await?;
        Ok(())
    }

    pub fn into_sub_file(self, offset: u64, length: u64) -> SubFile {
        SubFile { offset, length, file: self }
    }
}

impl AsyncGetSize for SideloadFile {
    fn poll_get_size(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        Poll::Ready(Ok(self.file_size))
    }
}

impl AsyncReadAt for SideloadFile {
    fn poll_read_at(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        offset: u64,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let file_size = self.file_size;
        let block_size = self.block_size;
        let read_block = offset / block_size as u64;
        let data_len = if read_block == file_size / block_size as u64 {
            (file_size % block_size as u64) as usize
        } else {
            block_size
        };
        loop {
            let ret =
                replace_with::replace_with_and(&mut self.read_at_state, |state| match state {
                    ReadAtState::Idle { mut socket, mut data } => {
                        if offset >= file_size {
                            return (ReadAtState::Idle { socket, data }, Some(Poll::Ready(Ok(0))));
                        }
                        let fut = async move {
                            // block number is always eight decimal digits.
                            // https://cs.android.com/android/platform/superproject/main/+/main:packages/modules/adb/client/commandline.cpp;drc=08a96199bf8ce0581c366fc9c725351ee127fd21;l=879
                            let msg = format!("{read_block:08}");
                            let () = socket.write_all(&msg.as_bytes()).await?;
                            let () = socket.read_exact(&mut data[..data_len]).await?;
                            Ok((socket, data))
                        }
                        .fuse()
                        .boxed();
                        (ReadAtState::Reading { fut, block: read_block }, None)
                    }
                    ReadAtState::Reading { mut fut, block } => match fut.poll_unpin(cx) {
                        Poll::Ready(Ok((socket, data))) => {
                            (ReadAtState::Ready { socket, data, block }, None)
                        }
                        Poll::Ready(Err(e)) => {
                            (ReadAtState::Reading { fut, block }, Some(Poll::Ready(Err(e))))
                        }
                        Poll::Pending => (ReadAtState::Reading { fut, block }, Some(Poll::Pending)),
                    },
                    ReadAtState::Ready { socket, data, block } => {
                        if read_block == block {
                            let start = (offset % block_size as u64) as usize;
                            let end = std::cmp::min(start + buf.len(), data_len);
                            buf[..end - start].copy_from_slice(&data[start..end]);
                            return (
                                ReadAtState::Ready { socket, data, block },
                                Some(Poll::Ready(Ok(end - start))),
                            );
                        }
                        (ReadAtState::Idle { socket, data }, None)
                    }
                });
            if let Some(ret) = ret {
                return ret;
            }
        }
    }
}

pub struct SubFile {
    offset: u64,
    length: u64,
    file: SideloadFile,
}

impl SubFile {
    pub async fn close(self) -> std::io::Result<()> {
        self.file.close().await
    }
}

impl AsyncGetSize for SubFile {
    fn poll_get_size(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        Poll::Ready(Ok(self.length))
    }
}

impl AsyncReadAt for SubFile {
    fn poll_read_at(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        offset: u64,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        if offset >= self.length {
            return Poll::Ready(Ok(0));
        }
        let len = std::cmp::min(buf.len() as u64, self.length - offset);
        let real_offset = self.offset + offset;
        Pin::new(&mut self.file).poll_read_at(cx, real_offset, &mut buf[..len as usize])
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fuchsia_fs::file::{AsyncGetSizeExt as _, AsyncReadAtExt as _};
    use test_case::test_case;

    fn spawn_sideload_server(
        mut socket: fasync::Socket,
        data: &[u8],
        block_size: usize,
    ) -> fasync::Task<()> {
        let data = data.to_owned();
        fasync::Task::spawn(async move {
            loop {
                let mut block_num = [0u8; 8];
                let () = socket.read_exact(&mut block_num).await.unwrap();
                if &block_num == SIDELOAD_DONE {
                    break;
                }
                let block_num = str::from_utf8(&block_num).unwrap();
                let block_num = block_num.parse::<usize>().unwrap();
                let start = block_num * block_size;
                let end = std::cmp::min(start + block_size, data.len());
                let () = socket.write_all(&data[start..end]).await.unwrap();
            }
        })
    }

    #[test_case(100 ; "exact multiple of block size")]
    #[test_case(103 ; "not exact multiple of block size")]
    #[fuchsia::test]
    async fn test_sideload_file(file_size: u64) {
        let (s1, s2) = fidl::Socket::create_stream();
        let block_size = 10;
        let data: Vec<u8> = (0..file_size as u8).collect();
        let mut file = SideloadFile::new(fasync::Socket::from_socket(s1), file_size, block_size);
        let server = spawn_sideload_server(fasync::Socket::from_socket(s2), &data, block_size);

        assert_eq!(file.get_size().await.unwrap(), file_size);

        let mut buf = [0u8; 10];
        assert_eq!(file.read_at(0, &mut buf).await.unwrap(), 10);
        assert_eq!(&buf, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(file.read_at(5, &mut buf[..5]).await.unwrap(), 5);
        assert_eq!(&buf[..5], &[5, 6, 7, 8, 9]);
        assert_eq!(file.read_at(9, &mut buf[..5]).await.unwrap(), 1);
        assert_eq!(&buf[..1], &[9]);
        assert_eq!(file.read_at(10, &mut buf).await.unwrap(), 10);
        assert_eq!(&buf, &[10, 11, 12, 13, 14, 15, 16, 17, 18, 19]);
        assert_eq!(file.read_at(95, &mut buf).await.unwrap(), 5);
        assert_eq!(&buf[..5], &[95, 96, 97, 98, 99]);
        assert_eq!(file.read_at(file_size, &mut buf).await.unwrap(), 0);
        assert_eq!(file.read_at(1000, &mut buf).await.unwrap(), 0);
        assert_eq!(file.read_at(0, &mut buf[..0]).await.unwrap(), 0);

        file.read_at_exact(9, &mut buf).await.unwrap();
        assert_eq!(&buf, &[9, 10, 11, 12, 13, 14, 15, 16, 17, 18]);
        file.read_at_exact(file_size - 5, &mut buf[..5]).await.unwrap();
        assert_eq!(&buf[..5], &data[file_size as usize - 5..]);

        let mut buf = vec![];
        assert_eq!(file.read_to_end(&mut buf).await.unwrap(), file_size as usize);
        assert_eq!(buf, data);

        let () = file.close().await.unwrap();
        server.await;
    }

    #[test_case(100 ; "exact multiple of block size")]
    #[test_case(103 ; "not exact multiple of block size")]
    #[fuchsia::test]
    async fn test_sub_file(file_size: u64) {
        let (s1, s2) = fidl::Socket::create_stream();
        let block_size = 10;
        let data: Vec<u8> = (0..file_size as u8).collect();
        let file = SideloadFile::new(fasync::Socket::from_socket(s1), file_size, block_size);
        let server = spawn_sideload_server(fasync::Socket::from_socket(s2), &data, block_size);
        let mut sub_file = file.into_sub_file(22, 30);

        assert_eq!(sub_file.get_size().await.unwrap(), 30);

        let mut buf = [0u8; 10];
        assert_eq!(sub_file.read_at(0, &mut buf).await.unwrap(), 8);
        assert_eq!(&buf[..8], &[22, 23, 24, 25, 26, 27, 28, 29]);
        assert_eq!(sub_file.read_at(5, &mut buf[..5]).await.unwrap(), 3);
        assert_eq!(&buf[..3], &[27, 28, 29]);
        assert_eq!(sub_file.read_at(8, &mut buf).await.unwrap(), 10);
        assert_eq!(&buf, &[30, 31, 32, 33, 34, 35, 36, 37, 38, 39]);
        assert_eq!(sub_file.read_at(9, &mut buf[..5]).await.unwrap(), 5);
        assert_eq!(&buf[..5], &[31, 32, 33, 34, 35]);
        assert_eq!(sub_file.read_at(27, &mut buf).await.unwrap(), 1);
        assert_eq!(&buf[..1], &[49]);
        assert_eq!(sub_file.read_at(28, &mut buf).await.unwrap(), 2);
        assert_eq!(&buf[..2], &[50, 51]);
        assert_eq!(sub_file.read_at(30, &mut buf).await.unwrap(), 0);
        assert_eq!(sub_file.read_at(1000, &mut buf).await.unwrap(), 0);
        assert_eq!(sub_file.read_at(0, &mut buf[..0]).await.unwrap(), 0);

        sub_file.read_at_exact(2, &mut buf).await.unwrap();
        assert_eq!(&buf, &[24, 25, 26, 27, 28, 29, 30, 31, 32, 33]);
        sub_file.read_at_exact(25, &mut buf[..5]).await.unwrap();
        assert_eq!(&buf[..5], &[47, 48, 49, 50, 51]);

        let mut buf = vec![];
        assert_eq!(sub_file.read_to_end(&mut buf).await.unwrap(), 30);
        assert_eq!(buf, &data[22..52]);

        let () = sub_file.close().await.unwrap();
        server.await;
    }
}
