// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::config::StreamSink;
use super::logger::{LogWriter, OutputLevel, SyslogWriter, create_namespace_logger};
use diagnostics_log::Publisher;
use fidl_fuchsia_process as fproc;
use fuchsia_async as fasync;
use fuchsia_runtime::{HandleInfo, HandleType};
use futures::{Future, StreamExt};
use log::warn;
use namespace::Namespace;
use socket_parsing::{NewlineChunker, NewlineChunkerError};
use vfs::ExecutionScope;

const STDOUT_FD: i32 = 1;
const STDERR_FD: i32 = 2;

/// Max size for message when draining input stream socket. This number is
/// slightly smaller than size allowed by Archivist (LogSink service implementation).
const MAX_MESSAGE_SIZE: usize = 30720;

/// Bind stdout or stderr streams to syslog. This function binds either or both
/// output streams to syslog depending on value provided for each streams'
/// StreamSink. If the value for an output stream is set to StreamSink::Log,
/// that stream's file descriptor will be bound to syslog. All writes on that
// fd will be forwarded to syslog and will register as log entries. For stdout,
// the messages will be tagged with severity INFO. For stderr, the messages
// will be tagged with severity WARN. A task is created to listen to writes on
// the appropriate file descriptor and forward the message to syslog. This
// function returns both the task for each file descriptor and its
// corresponding HandleInfo.
pub fn bind_streams_to_syslog(
    ns: &Namespace,
    stdout_sink: StreamSink,
    stderr_sink: StreamSink,
) -> (ExecutionScope, Vec<fproc::HandleInfo>) {
    let scope = ExecutionScope::new();
    let mut handles: Vec<fproc::HandleInfo> = Vec::new();

    let mut logger = None;
    let mut forward_stream = |sink, fd, level| {
        if matches!(sink, StreamSink::Log) {
            // create the handle before dealing with the logger so components still receive an inert
            // handle if connecting to LogSink fails
            let (socket, handle_info) =
                new_socket_bound_to_fd(fd).expect("failed to create socket");
            handles.push(handle_info);

            if let Some(logger) = logger.get_or_insert_with(|| create_namespace_logger(ns)) {
                scope.spawn(forward_socket_to_syslog(logger.clone(), socket, level));
            }
        }
    };

    forward_stream(stdout_sink, STDOUT_FD, OutputLevel::Info);
    forward_stream(stderr_sink, STDERR_FD, OutputLevel::Warn);

    (scope, handles)
}

fn forward_socket_to_syslog(
    logger: impl Future<Output = Option<Publisher>> + Send + 'static,
    socket: fasync::Socket,
    level: OutputLevel,
) -> impl Future<Output = ()> + 'static {
    async move {
        let Some(logger) = logger.await else { return };
        let mut writer = SyslogWriter::new(logger, level);
        if let Err(error) = drain_lines(socket, &mut writer).await {
            warn!(error:%; "Draining output stream failed");
        }
    }
}

fn new_socket_bound_to_fd(fd: i32) -> Result<(fasync::Socket, fproc::HandleInfo), zx::Status> {
    let (tx, rx) = zx::Socket::create_stream();
    let rx = fasync::Socket::from_socket(rx);
    Ok((
        rx,
        fproc::HandleInfo {
            handle: tx.into_handle(),
            id: HandleInfo::new(HandleType::FileDescriptor, fd as u16).as_raw(),
        },
    ))
}

/// Drains all bytes from socket and writes messages to writer. Bytes read
/// are split into lines and separated into chunks no greater than
/// MAX_MESSAGE_SIZE.
async fn drain_lines(
    socket: fasync::Socket,
    writer: &mut impl LogWriter,
) -> Result<(), NewlineChunkerError> {
    let chunker = NewlineChunker::new(socket, MAX_MESSAGE_SIZE);
    futures::pin_mut!(chunker);

    while let Some(chunk_or_line) = chunker.next().await {
        writer.write(&chunk_or_line?).await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Error, format_err};
    use fuchsia_async::Task;
    use futures::channel::mpsc;
    use futures::{FutureExt, SinkExt, try_join};
    use rand::distr::{Alphanumeric, SampleString as _};
    use rand::rng;

    impl LogWriter for mpsc::Sender<String> {
        async fn write(&mut self, bytes: &[u8]) {
            let message =
                std::str::from_utf8(&bytes).expect("Failed to decode bytes to utf8.").to_owned();
            let () =
                self.send(message).await.expect("Failed to send message to other end of mpsc.");
        }
    }

    #[fuchsia::test]
    async fn drain_lines_splits_into_max_size_chunks() -> Result<(), Error> {
        let (tx, rx) = zx::Socket::create_stream();
        let rx = fasync::Socket::from_socket(rx);
        let (mut sender, recv) = create_mock_logger();
        let msg = get_random_string(MAX_MESSAGE_SIZE * 4);

        let () = take_and_write_to_socket(tx, &msg)?;
        let (actual, ()) =
            try_join!(recv.collect().map(Result::<Vec<String>, Error>::Ok), async move {
                drain_lines(rx, &mut sender).await.map_err(Into::into)
            })?;

        assert_eq!(
            actual,
            msg.as_bytes()
                .chunks(MAX_MESSAGE_SIZE)
                .map(|bytes| std::str::from_utf8(bytes).expect("Bytes are not utf8.").to_owned())
                .collect::<Vec<String>>()
        );

        Ok(())
    }

    #[fuchsia::test]
    async fn drain_lines_splits_at_newline() -> Result<(), Error> {
        let (tx, rx) = zx::Socket::create_stream();
        let rx = fasync::Socket::from_socket(rx);
        let (mut sender, recv) = create_mock_logger();
        let msg =
            std::iter::repeat_with(|| Alphanumeric.sample_string(&mut rng(), MAX_MESSAGE_SIZE - 1))
                .take(3)
                .collect::<Vec<_>>()
                .join("\n");

        let () = take_and_write_to_socket(tx, &msg)?;
        let (actual, ()) =
            try_join!(recv.collect().map(Result::<Vec<String>, Error>::Ok), async move {
                drain_lines(rx, &mut sender).await.map_err(Into::into)
            })?;

        assert_eq!(actual, msg.split("\n").map(str::to_owned).collect::<Vec<String>>());
        Ok(())
    }

    #[fuchsia::test]
    async fn drain_lines_writes_when_message_is_received() -> Result<(), Error> {
        let (tx, rx) = zx::Socket::create_stream();
        let rx = fasync::Socket::from_socket(rx);
        let (mut sender, mut recv) = create_mock_logger();
        let messages: Vec<String> = vec!["Hello!\n".to_owned(), "World!\n".to_owned()];

        let ((), ()) = try_join!(
            async move { drain_lines(rx, &mut sender).await.map_err(Error::from) },
            async move {
                for mut message in messages.into_iter() {
                    let () = write_to_socket(&tx, &message)?;
                    let logged_messaged =
                        recv.next().await.context("Receiver channel closed. Got no message.")?;
                    // Logged message should strip '\n' so we need to do the same before assertion.
                    message.pop();
                    assert_eq!(logged_messaged, message);
                }

                Ok(())
            }
        )?;

        Ok(())
    }

    #[fuchsia::test]
    async fn drain_lines_waits_for_entire_lines() -> Result<(), Error> {
        let (tx, rx) = zx::Socket::create_stream();
        let rx = fasync::Socket::from_socket(rx);
        let (mut sender, mut recv) = create_mock_logger();

        let ((), ()) = try_join!(
            async move { drain_lines(rx, &mut sender).await.map_err(Error::from) },
            async move {
                let () = write_to_socket(&tx, "Hello\nWorld")?;
                let logged_messaged =
                    recv.next().await.context("Receiver channel closed. Got no message.")?;
                assert_eq!(logged_messaged, "Hello");
                let () = write_to_socket(&tx, "Hello\nAgain")?;
                std::mem::drop(tx);
                let logged_messaged =
                    recv.next().await.context("Receiver channel closed. Got no message.")?;
                assert_eq!(logged_messaged, "WorldHello");
                let logged_messaged =
                    recv.next().await.context("Receiver channel closed. Got no message.")?;
                assert_eq!(logged_messaged, "Again");
                Ok(())
            }
        )?;

        Ok(())
    }

    #[fuchsia::test]
    async fn drain_lines_collapses_repeated_newlines() -> Result<(), Error> {
        let (tx, rx) = zx::Socket::create_stream();
        let rx = fasync::Socket::from_socket(rx);
        let (mut sender, mut recv) = create_mock_logger();

        let drainer = Task::spawn(async move { drain_lines(rx, &mut sender).await });

        write_to_socket(&tx, "Hello\n\nWorld\n")?;
        assert_eq!(recv.next().await.unwrap(), "Hello");
        assert_eq!(recv.next().await.unwrap(), "World");

        drop(tx);
        drainer.await?;
        assert_eq!(recv.next().await, None);

        Ok(())
    }

    fn take_and_write_to_socket(socket: zx::Socket, message: &str) -> Result<(), Error> {
        write_to_socket(&socket, &message)
    }

    fn write_to_socket(socket: &zx::Socket, message: &str) -> Result<(), Error> {
        let bytes_written =
            socket.write(message.as_bytes()).context("Failed to write to socket")?;
        match bytes_written == message.len() {
            true => Ok(()),
            false => Err(format_err!(
                "Bytes written to socket doesn't match len of message. Message len = {}. Bytes written = {}",
                message.len(),
                bytes_written
            )),
        }
    }

    fn create_mock_logger() -> (mpsc::Sender<String>, mpsc::Receiver<String>) {
        mpsc::channel::<String>(20)
    }

    fn get_random_string(size: usize) -> String {
        Alphanumeric.sample_string(&mut rng(), size)
    }
}
