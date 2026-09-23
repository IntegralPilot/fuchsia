// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use async_trait::async_trait;
use fdomain_fuchsia_exception::ProcessLimboProxy;
use ffx_debug_limbo_args::{LimboCommand, LimboSubCommand};
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use safe_string::TermSafe;
use target_holders::moniker;
use zx_status::Status;
use zx_types::{ZX_ERR_NOT_FOUND, ZX_ERR_UNAVAILABLE};

#[derive(FfxTool)]
pub struct LimboTool {
    #[command]
    cmd: LimboCommand,
    #[with(moniker("/core/exceptions"))]
    limbo_proxy: ProcessLimboProxy,
}

fho::embedded_plugin!(LimboTool);

#[async_trait(?Send)]
impl FfxMain for LimboTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        match self.cmd.command {
            LimboSubCommand::Status(_) => status(self.limbo_proxy, writer).await?,
            LimboSubCommand::Enable(_) => enable(self.limbo_proxy, writer).await?,
            LimboSubCommand::Disable(_) => disable(self.limbo_proxy, writer).await?,
            LimboSubCommand::List(_) => list(self.limbo_proxy, writer).await?,
            LimboSubCommand::Release(release_cmd) => {
                release(self.limbo_proxy, release_cmd.pid, writer).await?
            }
        }
        Ok(())
    }
}

async fn status<W: std::io::Write>(limbo_proxy: ProcessLimboProxy, mut writer: W) -> Result<()> {
    let active = limbo_proxy.get_active().await?;
    if active {
        writeln!(writer, "Limbo is active.")?;
    } else {
        writeln!(writer, "Limbo is not active.")?;
    }
    Ok(())
}

async fn enable<W: std::io::Write>(limbo_proxy: ProcessLimboProxy, mut writer: W) -> Result<()> {
    let active = limbo_proxy.get_active().await?;
    if active {
        writeln!(writer, "Limbo is already active.")?;
    } else {
        limbo_proxy.set_active(true).await?;
        writeln!(writer, "Activated the process limbo.")?;
    }
    Ok(())
}

async fn disable<W: std::io::Write>(limbo_proxy: ProcessLimboProxy, mut writer: W) -> Result<()> {
    let active = limbo_proxy.get_active().await?;
    if !active {
        writeln!(writer, "Limbo is already deactivated.")?;
    } else {
        limbo_proxy.set_active(false).await?;
        writeln!(
            writer,
            "Deactivated the process limbo. All contained processes have been freed."
        )?;
    }
    Ok(())
}

async fn list<W: std::io::Write>(limbo_proxy: ProcessLimboProxy, mut writer: W) -> Result<()> {
    match limbo_proxy
        .list_processes_waiting_on_exception()
        .await
        .context("FIDL error in list_processes_waiting_on_exception")?
    {
        Ok(exceptions) => {
            if exceptions.is_empty() {
                writeln!(writer, "No processes currently on limbo.")?;
            } else {
                writeln!(writer, "Processes currently on limbo:")?;
                for metadada in exceptions {
                    let info = metadada.info.expect("missing info");
                    let process_name = TermSafe::from_str_escaped(
                        metadada.process_name.expect("missing process_name"),
                    );
                    let thread_name = TermSafe::from_str_escaped(
                        metadada.thread_name.expect("missing thread_name"),
                    );
                    writeln!(
                        writer,
                        "- {} (pid: {}), thread {} (tid: {}) on exception: {:?}",
                        process_name, info.process_koid, thread_name, info.thread_koid, info.type_
                    )?;
                }
            }
        }
        Err(e) => {
            if e == ZX_ERR_UNAVAILABLE {
                writeln!(writer, "Process limbo is not active.")?;
            } else {
                writeln!(
                    writer,
                    "Could not list the process limbo: {:?}",
                    Status::err_from_raw(e)
                )?;
            }
        }
    }
    Ok(())
}

async fn release<W: std::io::Write>(
    limbo_proxy: ProcessLimboProxy,
    pid: u64,
    mut writer: W,
) -> Result<()> {
    match limbo_proxy.release_process(pid).await? {
        Ok(_) => writeln!(writer, "Successfully release process {} from limbo.", pid)?,
        Err(e) => match e {
            ZX_ERR_UNAVAILABLE => writeln!(writer, "Process limbo is not active.")?,
            ZX_ERR_NOT_FOUND => writeln!(writer, "Could not find pid {} in limbo.", pid)?,
            e => writeln!(
                writer,
                "Could not release process {} from limbo: {:?}",
                pid,
                Status::err_from_raw(e)
            )?,
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdomain_fuchsia_exception::{
        ExceptionInfo, ExceptionType, ProcessExceptionInfo, ProcessLimboRequest,
    };
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_list_clean_names() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            ProcessLimboRequest::ListProcessesWaitingOnException { responder } => {
                let _ = responder.send(Ok(&[ProcessExceptionInfo {
                    info: Some(ExceptionInfo {
                        process_koid: 1234,
                        thread_koid: 5678,
                        type_: ExceptionType::SwBreakpoint,
                    }),
                    process_name: Some("clean_proc".to_string()),
                    thread_name: Some("clean_thread".to_string()),
                    ..Default::default()
                }]));
            }
            other => panic!("Unexpected request: {:?}", other),
        });

        let mut output = Vec::new();
        list(proxy, &mut output).await.unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(
            output_str,
            "Processes currently on limbo:\n- clean_proc (pid: 1234), thread clean_thread (tid: 5678) on exception: SwBreakpoint\n"
        );
    }

    #[fuchsia::test]
    async fn test_list_sanitizes_control_characters() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            ProcessLimboRequest::ListProcessesWaitingOnException { responder } => {
                let _ = responder.send(Ok(&[ProcessExceptionInfo {
                    info: Some(ExceptionInfo {
                        process_koid: 1001,
                        thread_koid: 2002,
                        type_: ExceptionType::FatalPageFault,
                    }),
                    process_name: Some("proc\x1b[31m_evil\n\r\t\0".to_string()),
                    thread_name: Some("thread\x1b]0;hacked\x07_name".to_string()),
                    ..Default::default()
                }]));
            }
            other => panic!("Unexpected request: {:?}", other),
        });

        let mut output = Vec::new();
        list(proxy, &mut output).await.unwrap();
        let output_str = String::from_utf8(output).unwrap();

        // Ensure raw control characters are NOT present
        assert!(!output_str.contains('\x1b'));
        assert!(!output_str.contains('\0'));
        assert!(!output_str.contains('\x07'));
        assert!(!output_str.contains('\r'));

        // Ensure escaped representation is printed
        assert_eq!(
            output_str,
            "Processes currently on limbo:\n- proc\\u{1b}[31m_evil\\n\\r\\t\\u{0} (pid: 1001), thread thread\\u{1b}]0;hacked\\u{7}_name (tid: 2002) on exception: FatalPageFault\n"
        );
    }

    #[fuchsia::test]
    async fn test_list_empty() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            ProcessLimboRequest::ListProcessesWaitingOnException { responder } => {
                let _ = responder.send(Ok(&[]));
            }
            other => panic!("Unexpected request: {:?}", other),
        });

        let mut output = Vec::new();
        list(proxy, &mut output).await.unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "No processes currently on limbo.\n");
    }

    #[fuchsia::test]
    async fn test_list_unavailable() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            ProcessLimboRequest::ListProcessesWaitingOnException { responder } => {
                let _ = responder.send(Err(zx_types::ZX_ERR_UNAVAILABLE));
            }
            other => panic!("Unexpected request: {:?}", other),
        });

        let mut output = Vec::new();
        list(proxy, &mut output).await.unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "Process limbo is not active.\n");
    }

    #[fuchsia::test]
    async fn test_status() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            ProcessLimboRequest::GetActive { responder } => {
                let _ = responder.send(true);
            }
            other => panic!("Unexpected request: {:?}", other),
        });

        let mut output = Vec::new();
        status(proxy, &mut output).await.unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "Limbo is active.\n");
    }

    #[fuchsia::test]
    async fn test_enable() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            ProcessLimboRequest::GetActive { responder } => {
                let _ = responder.send(false);
            }
            ProcessLimboRequest::SetActive { active, responder } => {
                assert!(active);
                let _ = responder.send();
            }
            other => panic!("Unexpected request: {:?}", other),
        });

        let mut output = Vec::new();
        enable(proxy, &mut output).await.unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "Activated the process limbo.\n");
    }

    #[fuchsia::test]
    async fn test_disable() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            ProcessLimboRequest::GetActive { responder } => {
                let _ = responder.send(true);
            }
            ProcessLimboRequest::SetActive { active, responder } => {
                assert!(!active);
                let _ = responder.send();
            }
            other => panic!("Unexpected request: {:?}", other),
        });

        let mut output = Vec::new();
        disable(proxy, &mut output).await.unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Deactivated the process limbo. All contained processes have been freed.\n"
        );
    }

    #[fuchsia::test]
    async fn test_release_success() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            ProcessLimboRequest::ReleaseProcess { process_koid, responder } => {
                assert_eq!(process_koid, 42);
                let _ = responder.send(Ok(()));
            }
            other => panic!("Unexpected request: {:?}", other),
        });

        let mut output = Vec::new();
        release(proxy, 42, &mut output).await.unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Successfully release process 42 from limbo.\n"
        );
    }
}
