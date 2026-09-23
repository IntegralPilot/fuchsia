// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::progress_reader::ProgressReader;
use crate::{SessionManagerProxyType, TraceData};
use anyhow::Result;
use async_fs::File;
use errors::ffx_bail;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_tracing_controller::{
    CompressionType, RecordingError, TraceConfig, TraceOptions,
};
use ffx_config::EnvironmentContext;
use fho::{bug, return_bug, return_user_error};
use futures::io::{self, AsyncReadExt};
use std::io as std_io;
use std::time::{Duration, Instant};
use trace_task_fdomain::TraceTask;
use zstd::stream::raw::Operation;

const ZSTD_MAGIC_NUMBER: u32 = 0xFD2FB528;

pub(crate) async fn trace(
    proxy: SessionManagerProxyType,
    options: TraceOptions,
    trace_config: TraceConfig,
    background: bool,
) -> Result<Option<TraceTask>> {
    let duration = options.duration_ns.map(|d| Duration::from_nanos(d as u64));

    let legacy_task = match proxy {
        SessionManagerProxyType::Provisioner(provisioner_proxy) => {
            let task = TraceTask::new(
                "ffx-trace-direct".into(),
                trace_config.clone(),
                duration,
                options
                    .triggers
                    .map(|tv| {
                        tv.iter()
                            .map(|t| trace_task_fdomain::Trigger {
                                action: t
                                    .action
                                    .as_ref()
                                    .map(|_| trace_task_fdomain::TriggerAction::Terminate),
                                alert: t.alert.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or(vec![]),
                options.requested_categories,
                options.compression.unwrap_or(CompressionType::None),
                provisioner_proxy,
            )
            .await?;

            Some(task)
        }
        SessionManagerProxyType::SessionManager(session_manager_proxy) => {
            let r = session_manager_proxy.start_trace_session(&trace_config, &options).await?;
            match r {
                Ok(_task_id) => None,
                Err(
                    fdomain_fuchsia_tracing_controller::RecordingError::RecordingAlreadyStarted,
                ) => {
                    return Err(anyhow::anyhow!(
                        "Trace already started for target.\n\
                        If you want to stop the trace and discard the data, run `ffx trace stop --abort`."
                    ));
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Error starting trace: {e:?}"));
                }
            }
        }
    };

    if !background {
        if let Some(trace_duration) = duration {
            fuchsia_async::Timer::new(trace_duration).await;
        }
    }
    Ok(legacy_task)
}

pub(crate) async fn trace_on_reboot(
    proxy: SessionManagerProxyType,
    options: TraceOptions,
    trace_config: TraceConfig,
) -> Result<()> {
    if let SessionManagerProxyType::SessionManager(session_manager_proxy) = proxy {
        let r = session_manager_proxy.start_trace_session_on_boot(&trace_config, &options).await?;
        match r {
            Ok(_task_id) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("Error starting trace: {e:?}")),
        }
    } else {
        ffx_bail!("SessionManager proxy is not available on device.")
    }
}

pub(crate) async fn stop_tracing(
    context: &EnvironmentContext,
    trace_task: Option<TraceTask>,
    trace_proxy: SessionManagerProxyType,
    output_file: &str,
) -> fho::Result<TraceData> {
    if let Some(task) = trace_task {
        let output = File::create(output_file).await.map_err(|e| bug!(e))?;
        Ok(TraceData {
            output_file: output_file.to_string(),
            categories: task.config().categories.clone().unwrap_or(vec![]),
            stop_result: task.stop_and_receive_data(output).await.map_err(|e| bug!(e))?,
        })
    } else {
        if let SessionManagerProxyType::SessionManager(session_mgr_proxy) = trace_proxy {
            let (client, server) = session_mgr_proxy.domain().create_stream_socket();
            let join_result = futures::try_join!(download_trace(client, output_file), async {
                log::info!("Calling end_session.");
                // Always pass 0 for the session id, multiple sessions are not supported (yet).
                let r = session_mgr_proxy.end_trace_session(0, server).await;
                let r = r.map_err(|e| anyhow::anyhow!("FIDL error mapping: {:?}", e))?;
                log::debug!("Done.");
                Ok(r)
            });
            match join_result {
                Ok((copy_res, end_res)) => {
                    log::debug!("Copy res is {copy_res:?}");
                    match end_res {
                        Ok((options, stop_result)) => Ok(TraceData {
                            output_file: output_file.to_string(),
                            categories: options.requested_categories.unwrap_or(vec![]),
                            stop_result,
                        }),
                        Err(RecordingError::NoSuchTraceFile) => {
                            return_user_error!("No active traces")
                        }

                        Err(e) => return_bug!(anyhow::anyhow!(
                            "{}",
                            crate::handle_recording_error(context, e, &output_file).await
                        )),
                    }
                }
                Err(e) => return_bug!("join result is {e:?}"),
            }
        } else {
            return_bug!("Unexpected state with no TraceTask, and no SessionManager?");
        }
    }
}

pub(crate) async fn abort_tracing(
    context: &EnvironmentContext,
    trace_task: Option<TraceTask>,
    trace_proxy: SessionManagerProxyType,
) -> fho::Result<()> {
    if let Some(task) = trace_task {
        match task.abort().await {
            Ok(_) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("Error stopping trace: {e:?}").into()),
        }
    } else {
        if let SessionManagerProxyType::SessionManager(session_mgr_proxy) = trace_proxy {
            let r = session_mgr_proxy.abort_trace_session(0).await;
            let r = r.map_err(|e| anyhow::anyhow!("FIDL error mapping: {:?}", e))?;
            match r {
                Ok(_) => Ok(()),
                Err(e) => {
                    return_bug!(crate::handle_recording_error(context, e, "").await)
                }
            }
        } else {
            return_bug!("Unexpected state with no TraceTask, and no SessionManager?")
        }
    }
}

async fn download_trace(
    read_socket: fdomain_client::Socket,
    output_file: &str,
) -> Result<u64, anyhow::Error> {
    let mut output =
        File::create(&output_file).await.map_err(|e| bug!("Could not create output file: {e}"))?;
    log::info!("Starting local copy to {output_file}.");
    let start_time = Instant::now();
    let mut progress_reader = ProgressReader::new(read_socket);

    // Peek first 4 bytes to check for Zstd magic
    let mut magic = [0u8; 4];
    let mut peeked = 0;
    while peeked < 4 {
        let n = progress_reader
            .read(&mut magic[peeked..])
            .await
            .map_err(|e| bug!("Read error: {e}"))?;
        if n == 0 {
            break;
        }
        peeked += n;
    }

    let result = if peeked == 4 && u32::from_le_bytes(magic) == ZSTD_MAGIC_NUMBER {
        log::info!("Detected Zstd compression. Decompressing on the fly.");
        // Add the prefix in front of the stream, and decompress.
        decompress_zstd(io::Cursor::new(magic).chain(progress_reader), &mut output).await
    } else {
        // Not Zstd or too short, write what we read and copy the rest
        if peeked > 0 {
            use futures::AsyncWriteExt;
            AsyncWriteExt::write_all(&mut output, &magic[..peeked])
                .await
                .map_err(|e| bug!("Write error: {e}"))?;
        }
        io::copy(&mut progress_reader, &mut output).await
    };

    if let Ok(bytes) = &result {
        if let Err(e) = output.sync_all().await {
            log::error!("Failed to sync trace data to disk: {:?}", e);
            return Err(bug!("Failed to sync trace data to disk: {e}").into());
        }
        let duration = start_time.elapsed();
        let kbytes = *bytes / 1024;

        let rate =
            if duration.as_secs_f64() > 0. { kbytes as f64 / duration.as_secs_f64() } else { 0.0 };
        // progress_reader is moved in the Zstd case, so we can't reliably call status_update on it.
        // Instead, just log the final status.
        log::info!("Total size: {kbytes}kB, Duration: {duration:?}, Rate: {rate:.2} kB/s");
    }
    log::info!("Copy done");
    result.map_err(Into::into)
}

async fn decompress_zstd<R, W>(mut reader: R, mut writer: W) -> std_io::Result<u64>
where
    R: futures::AsyncRead + Unpin,
    W: futures::AsyncWrite + Unpin,
{
    use futures::{AsyncReadExt, AsyncWriteExt};
    let mut decoder = zstd::stream::raw::Decoder::new()
        .map_err(|e| std_io::Error::new(std_io::ErrorKind::Other, format!("zstd init: {e:?}")))?;
    // 128KB is the recommended size for Zstd (ZSTD_CStreamInSize/ZSTD_CStreamOutSize)
    let mut input_buf = vec![0u8; 128 * 1024];
    let mut output_buf = vec![0u8; 128 * 1024];
    let mut total_written = 0;

    // Loop reading from input, decompressing, and writing to output when
    // a full buffer has been decoded.
    while let n = reader.read(&mut input_buf).await?
        && n > 0
    {
        let mut read_offset = 0;
        while read_offset < n {
            let status =
                decoder.run_on_buffers(&input_buf[read_offset..n], &mut output_buf).map_err(
                    |e| std_io::Error::new(std_io::ErrorKind::Other, format!("zstd run: {e:?}")),
                )?;
            read_offset += status.bytes_read;
            if status.bytes_written > 0 {
                writer.write_all(&output_buf[..status.bytes_written]).await?;
                total_written += status.bytes_written as u64;
            }
            if status.bytes_read == 0 && status.bytes_written == 0 {
                // Should not happen if n > 0, but good to check progress
                if read_offset < n {
                    return Err(std_io::Error::new(std_io::ErrorKind::Other, "Zstd stuck"));
                }
            }
        }
    }

    // Flush any remaining uncompressed bytes buffered in ZSTD's internal decoder.
    loop {
        let status = decoder.run_on_buffers(&[], &mut output_buf).map_err(|e| {
            std_io::Error::new(std_io::ErrorKind::Other, format!("zstd flush run: {e:?}"))
        })?;
        if status.bytes_written > 0 {
            writer.write_all(&output_buf[..status.bytes_written]).await?;
            total_written += status.bytes_written as u64;
        } else {
            break;
        }
    }

    Ok(total_written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::AsyncReadExt;
    use futures::io::Cursor;

    #[fuchsia::test]
    async fn test_copy_zstd_with_prefix_decompresses_correctly() {
        // 1. Generate some data
        let original_data =
            b"Hello world! This is a test string to be compressed and decompressed.";
        let mut compressed_data = Vec::new();

        // 2. Compress it
        {
            let mut encoder = zstd::stream::write::Encoder::new(&mut compressed_data, 0).unwrap();
            use std::io::Write;
            encoder.write_all(original_data).unwrap();
            encoder.finish().unwrap();
        }

        let magic = &compressed_data[0..4];
        let rest = &compressed_data[4..];

        // 3. Verify magic
        assert_eq!(magic, ZSTD_MAGIC_NUMBER.to_le_bytes());

        // 5. Create the chained reader (mimicking download_trace)
        let reader = Cursor::new(magic).chain(Cursor::new(rest));
        let mut output = Vec::new();

        // 6. Run copy_zstd_with_prefix
        let bytes_written =
            decompress_zstd(reader, &mut output).await.expect("decompression failed");

        // 7. Verify
        assert_eq!(bytes_written, original_data.len() as u64);
        assert_eq!(output, original_data);
    }

    #[fuchsia::test]
    async fn test_copy_zstd_with_prefix_large_data() {
        // Compress larger data to ensure buffering works
        let original_data = vec![0xAB; 100_000]; // 100KB
        let mut compressed_data = Vec::new();
        {
            let mut encoder = zstd::stream::write::Encoder::new(&mut compressed_data, 0).unwrap();
            use std::io::Write;
            encoder.write_all(&original_data).unwrap();
            encoder.finish().unwrap();
        }

        let magic = &compressed_data[0..4];
        let rest = &compressed_data[4..];
        let reader = Cursor::new(magic).chain(Cursor::new(rest));
        let mut output = Vec::new();

        let bytes_written = decompress_zstd(reader, &mut output).await.unwrap();
        assert_eq!(bytes_written, original_data.len() as u64);
        assert_eq!(output, original_data);
    }

    #[fuchsia::test]
    async fn test_copy_zstd_with_prefix_exceeds_buffer_size() {
        // Compress 5 chunks of 30,000 bytes (150KB total) and flush after each chunk.
        // This forces unaligned ZSTD block boundaries relative to the 128KB decompression buffer,
        // ensuring that decompressed bytes from the final chunk remain buffered in decoder after EOF.
        let chunk_data: Vec<u8> = (0..30_000).map(|i| (i % 256) as u8).collect();
        let mut compressed_data = Vec::new();
        {
            let mut encoder = zstd::stream::write::Encoder::new(&mut compressed_data, 0).unwrap();
            use std::io::Write;
            for _ in 0..5 {
                encoder.write_all(&chunk_data).unwrap();
                encoder.flush().unwrap();
            }
            encoder.finish().unwrap();
        }

        let magic = &compressed_data[0..4];
        let rest = &compressed_data[4..];
        let reader = Cursor::new(magic).chain(Cursor::new(rest));
        let mut output = Vec::new();

        let bytes_written = decompress_zstd(reader, &mut output).await.unwrap();
        assert_eq!(bytes_written, 150_000);
        assert_eq!(output.len(), 150_000);
    }
}
