// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::reply::Reply;

use async_trait::async_trait;
use chrono::Duration;
use command::Command;
use fuchsia_async::TimeoutExt;
use futures::io::{AsyncRead, AsyncWrite};
use futures::lock::Mutex;
use futures::{AsyncReadExt, AsyncWriteExt};
use std::io::Read;
use std::sync::Arc;
use thiserror::Error;

pub mod command;
pub mod reply;
pub mod test_transport;

pub use crate::command::MAX_COMMAND_LENGTH;

pub const BUFFER_SIZE: usize = 4 * 1024 * 1024; // 4 MB

const MAX_PACKET_SIZE: usize = 64;
const DEFAULT_READ_TIMEOUT_SECS: i64 = 30;

#[derive(Debug, Clone)]
pub struct FastbootContext {
    send_lock: Arc<Mutex<()>>,
    transfer_lock: Arc<Mutex<()>>,
}

impl FastbootContext {
    pub fn new() -> Self {
        Self { send_lock: Arc::new(Mutex::new(())), transfer_lock: Arc::new(Mutex::new(())) }
    }
}

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(#[from] crate::reply::ParseReplyError),
    #[error("Timed out")]
    Timeout,
}

#[derive(Debug, Error)]
pub enum FastbootError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Read error: {0}")]
    Read(#[from] ReadError),
    #[error("Download error: {0}")]
    Download(#[from] DownloadError),
    #[error("Upload error: {0}")]
    Upload(#[from] UploadError),
    #[error("Parse error: {0}")]
    Parse(#[from] crate::reply::ParseReplyError),
    #[error("Serialize command error: {0}")]
    Serialize(#[from] crate::command::SerializeCommandError),
    #[error("Invalid file size: {0}")]
    InvalidFileSize(#[from] std::num::TryFromIntError),
}

#[derive(Debug, Error)]
pub enum SendError {
    #[error("timed out reading a reply from device")]
    Timeout,
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("Did not get expected Data reply: {:?}", reply)]
    UnexpectedReply { reply: Reply },
    #[error("Could not verify download: {0}")]
    CouldNotVerifyDownload(#[source] ReadError),
    #[error("Could not read to interface")]
    CouldNotReadToInterface(#[source] std::io::Error),
}

#[derive(Debug, Error)]
pub enum UploadError {
    #[error("Target responded with wrong data size - received:{} expected:{}", received, expected)]
    WrongSizeResponse { received: u32, expected: u32 },
    #[error("Could not read bytes to upload")]
    CouldNotReadBytesToUpload { source: std::io::Error },
    #[error("Could not write to interface")]
    CouldNotWriteToInterface(#[source] std::io::Error),
    #[error("Could not verify upload: {0}")]
    CouldNotVerifyUpload(#[source] ReadError),
    #[error("Did not get expected Data reply: {:?}", reply)]
    UnexpectedReply { reply: Reply },
}

#[async_trait]
pub trait InfoListener {
    async fn on_info(&self, info: String) -> Result<(), ReadError> {
        log::info!("Fastboot Info: \"{}\"", info);
        Ok(())
    }
}

struct LogInfoListener {}
impl InfoListener for LogInfoListener {}

#[async_trait]
pub trait UploadProgressListener {
    async fn on_started(&self, size: usize) -> Result<(), UploadError>;
    async fn on_progress(&self, bytes_written: u64) -> Result<(), UploadError>;
    async fn on_error(&self, error: &UploadError) -> Result<(), UploadError>;
    async fn on_finished(&self) -> Result<(), UploadError>;
}

async fn read_from_interface<T: AsyncRead + Unpin>(interface: &mut T) -> Result<Reply, ReadError> {
    let mut buf: [u8; MAX_PACKET_SIZE] = [0; MAX_PACKET_SIZE];
    let size = interface.read(&mut buf).await?;
    let (trimmed, _) = buf.split_at(size);
    let trimmed = trimmed.to_vec();
    let reply = Reply::try_from(trimmed.as_slice()).map_err(|e| {
        log::debug!("fastboot: could not parse reply: {}", String::from_utf8_lossy(&trimmed));
        ReadError::Parse(e)
    })?;
    log::debug!("fastboot: received {reply:?}: {}", String::from_utf8_lossy(&trimmed));
    Ok(reply)
}

async fn read<T: AsyncRead + Unpin>(
    interface: &mut T,
    listener: &(impl InfoListener + Sync),
) -> Result<Reply, ReadError> {
    read_with_timeout(interface, listener, Duration::seconds(DEFAULT_READ_TIMEOUT_SECS)).await
}

async fn read_and_log_info<T: AsyncRead + Unpin>(interface: &mut T) -> Result<Reply, ReadError> {
    read_and_log_info_with_timeout(interface, Duration::seconds(DEFAULT_READ_TIMEOUT_SECS)).await
}

pub async fn read_and_log_info_with_timeout<T: AsyncRead + Unpin>(
    interface: &mut T,
    duration: Duration,
) -> Result<Reply, ReadError> {
    read_with_timeout(interface, &LogInfoListener {}, duration).await
}

async fn read_with_timeout<T: AsyncRead + Unpin>(
    interface: &mut T,
    listener: &(impl InfoListener + Sync),
    timeout: Duration,
) -> Result<Reply, ReadError> {
    let std_timeout = timeout.to_std().expect("converting chrono Duration to std");
    let end_time = std::time::Instant::now() + std_timeout;
    loop {
        match read_from_interface(interface).on_timeout(end_time, || Err(ReadError::Timeout)).await
        {
            Ok(Reply::Info(msg)) => listener.on_info(msg).await?,
            #[cfg(target_os = "linux")]
            Err(e) => {
                // If we get a TIMEDOUT response, keep reading -- that's just the usb_bulk crate
                // not willing to spend more than 800ms waiting for a result
                // Desired code:
                // if let Some(ioe) = e.downcast_ref::<std::io::Error>() {
                //     if ioe.kind() != std::io::ErrorKind::TimedOut {
                //         ...
                //     }
                // }
                // Unfortunately usb_bulk does not try to interpret the
                // type of the error, but instead always sets the kind to
                // ErrorKind::Other.  So we can't check if the kind is
                // Timeout.  So instead, let's just read the text of
                // the error, ugh.
                if e.to_string() != "Read error: -110" {
                    return Err(e);
                }
            }
            #[cfg(target_os = "macos")]
            Err(_) => {
                // usb_bulk returns different values on mac vs. linux. On Linux it
                // returns ETIMEDOUT, but on the Mac it's just a generic -1. (And
                // Apple doesn't actually document how to determine whether a read
                // has timed out.)  So on Mac, we'll ignore _all_ errors, and cross
                // our fingers.
            }
            other => return other,
        }
        // We can't actually rely on `on_timeout()` to time out, because while
        // `usb_bulk` claims that it implements `AsyncRead`, it's not actually
        // async.  As a result, on_timeout() doesn't work.  We'll leave it in
        // to avoid problems in the future, and so our unit tests can remain
        // asynchronous.
        if std::time::Instant::now() > end_time {
            return Err(ReadError::Timeout);
        }
    }
}

pub async fn send_with_listener<T: AsyncRead + AsyncWrite + Unpin>(
    ctx: FastbootContext,
    cmd: Command,
    interface: &mut T,
    listener: &(impl InfoListener + Sync),
) -> Result<Reply, FastbootError> {
    let _lock = ctx.send_lock.lock().await;
    let bytes = Vec::<u8>::try_from(&cmd)?;
    log::debug!("Fastboot: writing command {cmd:?}: {}", String::from_utf8_lossy(&bytes));
    interface.write_all(&bytes).await?;
    Ok(read(interface, listener).await?)
}

pub async fn send<T: AsyncRead + AsyncWrite + Unpin>(
    ctx: FastbootContext,
    cmd: Command,
    interface: &mut T,
) -> Result<Reply, FastbootError> {
    let _lock = ctx.send_lock.lock().await;
    let bytes = Vec::<u8>::try_from(&cmd)?;
    log::debug!("Fastboot: writing command {cmd:?}: {}", String::from_utf8_lossy(&bytes));
    interface.write_all(&bytes).await?;
    Ok(read_and_log_info(interface).await?)
}

pub async fn send_with_timeout<T: AsyncRead + AsyncWrite + Unpin>(
    ctx: FastbootContext,
    cmd: Command,
    interface: &mut T,
    timeout: Duration,
) -> Result<Reply, FastbootError> {
    let _lock = ctx.send_lock.lock().await;
    let bytes = Vec::<u8>::try_from(&cmd)?;
    log::debug!("Fastboot: writing command {cmd:?}: {}", String::from_utf8_lossy(&bytes));
    interface.write_all(&bytes).await?;
    Ok(read_with_timeout(interface, &LogInfoListener {}, timeout).await?)
}

const PIPELINE_BUFFER_CHUNKS: usize = 2;

pub async fn upload<T: AsyncRead + AsyncWrite + Unpin, R: Read + Send + 'static>(
    ctx: FastbootContext,
    size: u32,
    buf: R,
    interface: &mut T,
    listener: &impl UploadProgressListener,
) -> Result<Reply, FastbootError> {
    upload_with_read_timeout(
        ctx,
        size,
        buf,
        interface,
        listener,
        Duration::seconds(DEFAULT_READ_TIMEOUT_SECS),
    )
    .await
}

pub async fn upload_with_read_timeout<
    T: AsyncRead + AsyncWrite + Unpin,
    R: Read + Send + 'static,
>(
    ctx: FastbootContext,
    size: u32,
    mut buf: R,
    interface: &mut T,
    listener: &impl UploadProgressListener,
    timeout: Duration,
) -> Result<Reply, FastbootError> {
    let _lock = ctx.transfer_lock.lock().await;
    // We are sending "Download" in our "upload" function because we are the
    // host -- from the device's point of view, it is a download
    let reply = send(ctx.clone(), Command::Download(size), interface).await?;
    let Reply::Data(s) = reply else {
        return Err(FastbootError::Upload(UploadError::UnexpectedReply { reply }));
    };

    if s != size {
        let err = UploadError::WrongSizeResponse { received: s, expected: size };
        log::error!("{}", err);
        listener.on_error(&err).await?;
        return Err(FastbootError::Upload(err));
    }
    listener.on_started(size.try_into().unwrap()).await?;
    log::debug!("fastboot: writing {} bytes", size);

    let (tx, mut rx) = tokio::sync::mpsc::channel(PIPELINE_BUFFER_CHUNKS);

    let reader_thread = std::thread::spawn(move || {
        let mut remaining = size as usize;
        while remaining > 0 {
            let to_read = std::cmp::min(remaining, BUFFER_SIZE);
            let mut chunk = vec![0; to_read];
            match buf.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    chunk.truncate(n);
                    remaining -= n;
                    if tx.blocking_send(Ok(chunk)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    break;
                }
            }
        }
    });

    let mut upload_err = None;
    while let Some(chunk_res) = rx.recv().await {
        match chunk_res {
            Ok(chunk) => {
                let n = chunk.len();
                match interface.write_all(&chunk).await {
                    Err(e) => {
                        let err = UploadError::CouldNotWriteToInterface(e);
                        log::error!("{}", err);
                        let _ = listener.on_error(&err).await;
                        upload_err = Some(FastbootError::Upload(err));
                        break;
                    }
                    Ok(()) => {
                        listener.on_progress(n.try_into().unwrap()).await?;
                        log::trace!("fastboot: wrote {} bytes", n);
                    }
                }
            }
            Err(e) => {
                let err = UploadError::CouldNotReadBytesToUpload { source: e };
                log::error!("{}", err);
                let _ = listener.on_error(&err).await;
                upload_err = Some(FastbootError::Upload(err));
                break;
            }
        }
    }

    // Close the receiver so the reader thread unblocks if waiting on a full channel,
    // then join the thread before checking errors.
    drop(rx);
    let _ = reader_thread.join();

    if let Some(err) = upload_err {
        return Err(err);
    }

    log::debug!("fastboot: completed writing {} bytes", size);

    match read_and_log_info_with_timeout(interface, timeout).await {
        Ok(reply) => {
            listener.on_finished().await?;
            Ok(reply)
        }
        Err(e) => {
            let err = UploadError::CouldNotVerifyUpload(e);
            log::error!("{}", err);
            listener.on_error(&err).await?;
            Err(FastbootError::Upload(err))
        }
    }
}

pub async fn upload_from_reader<T: AsyncRead + AsyncWrite + Unpin, R: Read + ?Sized>(
    ctx: FastbootContext,
    size: u32,
    buf: &mut R,
    interface: &mut T,
    listener: &impl UploadProgressListener,
    timeout: Duration,
) -> Result<Reply, FastbootError> {
    let _lock = ctx.transfer_lock.lock().await;
    let reply = send(ctx.clone(), Command::Download(size), interface).await?;
    let Reply::Data(s) = reply else {
        return Err(FastbootError::Upload(UploadError::UnexpectedReply { reply }));
    };

    if s != size {
        let err = UploadError::WrongSizeResponse { received: s, expected: size };
        log::error!("{}", err);
        listener.on_error(&err).await?;
        return Err(FastbootError::Upload(err));
    }
    listener.on_started(size.try_into().unwrap()).await?;
    log::debug!("fastboot: writing {} bytes", size);

    let mut remaining = size as usize;
    let chunk_size = std::cmp::min(size as usize, BUFFER_SIZE);
    let mut bytes = vec![0; chunk_size];
    loop {
        let to_read = std::cmp::min(remaining, BUFFER_SIZE);
        if to_read == 0 {
            break;
        }
        match buf.read(&mut bytes[..to_read]) {
            Ok(0) => break,
            Ok(n) => {
                remaining -= n;
                match interface.write_all(&bytes[..n]).await {
                    Err(e) => {
                        let err = UploadError::CouldNotWriteToInterface(e);
                        log::error!("{}", err);
                        listener.on_error(&err).await?;
                        return Err(FastbootError::Upload(err));
                    }
                    Ok(()) => {
                        listener.on_progress(n.try_into().unwrap()).await?;
                        log::trace!("fastboot: wrote {} bytes", n);
                    }
                }
            }
            Err(e) => {
                let err = UploadError::CouldNotReadBytesToUpload { source: e };
                log::error!("{}", err);
                listener.on_error(&err).await?;
                return Err(FastbootError::Upload(err));
            }
        }
    }

    log::debug!("fastboot: completed writing {} bytes", size);

    match read_and_log_info_with_timeout(interface, timeout).await {
        Ok(reply) => {
            listener.on_finished().await?;
            Ok(reply)
        }
        Err(e) => {
            let err = UploadError::CouldNotVerifyUpload(e);
            log::error!("{}", err);
            listener.on_error(&err).await?;
            Err(FastbootError::Upload(err))
        }
    }
}

pub async fn download<T: AsyncRead + AsyncWrite + Unpin>(
    ctx: FastbootContext,
    path: &String,
    interface: &mut T,
) -> Result<Reply, FastbootError> {
    let _lock = ctx.transfer_lock.lock().await;
    // We are sending "Upload" in our "download" function because we are the
    // host -- from the device's point of view, it is an upload
    let reply = send(ctx.clone(), Command::Upload, interface).await?;
    log::debug!("got reply from upload command: {:?}", reply);
    match reply {
        Reply::Data(s) => {
            let size = usize::try_from(s)?;
            let mut buffer: [u8; 100] = [0; 100];
            let mut bytes_read: usize = 0;
            let mut file = async_fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)
                .await?;
            while bytes_read < size {
                match interface.read(&mut buffer[..]).await {
                    Err(e) => {
                        return Err(FastbootError::Download(
                            DownloadError::CouldNotReadToInterface(e),
                        ));
                    }
                    Ok(len) => {
                        let len = if (bytes_read + len) > size { size - bytes_read } else { len };
                        bytes_read += len;
                        log::debug!("fastboot: upload got {bytes_read}/{size} bytes. Len {len}");
                        file.write_all(&buffer[..len]).await?;
                    }
                }
            }
            file.flush().await?;
            Ok(read_and_log_info(interface)
                .await
                .map_err(|e| FastbootError::Download(DownloadError::CouldNotVerifyDownload(e)))?)
        }
        rep @ _ => {
            return Err(FastbootError::Download(DownloadError::UnexpectedReply { reply: rep }));
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use crate::command::ClientVariable;
    use crate::test_transport::TestTransport;
    use std::io::Cursor;
    use std::sync::Arc;

    #[derive(Debug, PartialEq)]
    enum UploadEvent {
        OnStarted(usize),
        OnProgress(u64),
        OnError(String),
        OnFinished,
    }

    struct PushEventsUploadProgressListener {
        event_queue: Arc<Mutex<Vec<UploadEvent>>>,
    }

    #[async_trait]
    impl UploadProgressListener for PushEventsUploadProgressListener {
        async fn on_started(&self, size: usize) -> Result<(), UploadError> {
            let mut queue = self.event_queue.lock().await;
            queue.push(UploadEvent::OnStarted(size));
            Ok(())
        }
        async fn on_progress(&self, bytes_written: u64) -> Result<(), UploadError> {
            let mut queue = self.event_queue.lock().await;
            queue.push(UploadEvent::OnProgress(bytes_written));
            Ok(())
        }
        async fn on_error(&self, error: &UploadError) -> Result<(), UploadError> {
            let mut queue = self.event_queue.lock().await;
            queue.push(UploadEvent::OnError(error.to_string()));
            Ok(())
        }
        async fn on_finished(&self) -> Result<(), UploadError> {
            let mut queue = self.event_queue.lock().await;
            queue.push(UploadEvent::OnFinished);
            Ok(())
        }
    }

    #[fuchsia::test]
    async fn test_send_does_not_return_info_replies() {
        let mut test_transport = TestTransport::new();
        let ctx = FastbootContext::new();
        test_transport.push(Reply::Okay("0.4".to_string()));
        let response =
            send(ctx.clone(), Command::GetVar(ClientVariable::Version), &mut test_transport).await;
        assert!(!response.is_err());
        assert_eq!(response.unwrap(), Reply::Okay("0.4".to_string()));

        test_transport.extend([Reply::Info("Test".to_string()), Reply::Okay("0.4".to_string())]);
        let response_with_info =
            send(ctx.clone(), Command::GetVar(ClientVariable::Version), &mut test_transport).await;
        assert!(!response_with_info.is_err());
        assert_eq!(response_with_info.unwrap(), Reply::Okay("0.4".to_string()));

        test_transport.extend((0..10).map(|i| Reply::Info(format!("Test {i}"))));
        test_transport.push(Reply::Okay("0.4".to_string()));
        let response_with_info =
            send(ctx.clone(), Command::GetVar(ClientVariable::Version), &mut test_transport).await;
        assert!(!response_with_info.is_err());
        assert_eq!(response_with_info.unwrap(), Reply::Okay("0.4".to_string()));
    }

    #[fuchsia::test]
    #[allow(clippy::large_futures)]
    async fn test_uploading_data_to_partition() {
        let data: [u8; 14336] = [0; 14336];
        let mut test_transport = TestTransport::new();
        test_transport.extend([
            Reply::Data(14336),
            Reply::Info("Writing".to_string()),
            Reply::Okay("Done Writing".to_string()),
        ]);

        let events = Arc::new(Mutex::new(Vec::<UploadEvent>::new()));
        let listener = PushEventsUploadProgressListener { event_queue: events.clone() };

        let data_len = u32::try_from(data.len()).unwrap();
        let ctx = FastbootContext::new();
        let response =
            upload(ctx, data_len, Cursor::new(data), &mut test_transport, &listener).await;
        assert!(!response.is_err());
        assert_eq!(response.unwrap(), Reply::Okay("Done Writing".to_string()));

        let queue = events.lock().await;
        assert_eq!(
            *queue,
            vec![
                UploadEvent::OnStarted(14336),
                UploadEvent::OnProgress(14336),
                UploadEvent::OnFinished,
            ]
        );
    }

    #[fuchsia::test]
    async fn test_upload_from_reader_to_partition() {
        let data: [u8; 14336] = [0; 14336];
        let mut test_transport = TestTransport::new();
        test_transport.extend([
            Reply::Data(14336),
            Reply::Info("Writing".to_string()),
            Reply::Okay("Done Writing".to_string()),
        ]);

        let events = Arc::new(Mutex::new(Vec::<UploadEvent>::new()));
        let listener = PushEventsUploadProgressListener { event_queue: events.clone() };

        let data_len = u32::try_from(data.len()).unwrap();
        let ctx = FastbootContext::new();
        let mut reader = &data[..];
        let response = upload_from_reader(
            ctx,
            data_len,
            &mut reader,
            &mut test_transport,
            &listener,
            Duration::seconds(DEFAULT_READ_TIMEOUT_SECS),
        )
        .await;
        assert!(!response.is_err());
        assert_eq!(response.unwrap(), Reply::Okay("Done Writing".to_string()));

        let queue = events.lock().await;
        assert_eq!(
            *queue,
            vec![
                UploadEvent::OnStarted(14336),
                UploadEvent::OnProgress(14336),
                UploadEvent::OnFinished,
            ]
        );
    }

    #[fuchsia::test]
    async fn test_uploading_data_with_unexpected_reply() {
        let data: [u8; 1024] = [0; 1024];
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Info("Writing".to_string()));
        let ctx = FastbootContext::new();

        let events = Arc::new(Mutex::new(Vec::<UploadEvent>::new()));
        let listener = PushEventsUploadProgressListener { event_queue: events.clone() };
        let data_len = u32::try_from(data.len()).unwrap();
        let response =
            upload(ctx, data_len, Cursor::new(data), &mut test_transport, &listener).await;
        assert!(response.is_err());
        let queue = events.lock().await;
        assert_eq!(*queue, vec![]);
    }

    #[fuchsia::test]
    async fn test_uploading_data_with_unexpected_data_size_reply() {
        let data: [u8; 1024] = [0; 1024];
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Data(1000));
        let ctx = FastbootContext::new();

        let events = Arc::new(Mutex::new(Vec::<UploadEvent>::new()));
        let listener = PushEventsUploadProgressListener { event_queue: events.clone() };
        let data_len = u32::try_from(data.len()).unwrap();
        let response =
            upload(ctx, data_len, Cursor::new(data), &mut test_transport, &listener).await;
        assert!(response.is_err());
        let queue = events.lock().await;
        assert_eq!(
            *queue,
            vec![UploadEvent::OnError(
                "Target responded with wrong data size - received:1000 expected:1024".to_string()
            ),]
        );
    }
}
