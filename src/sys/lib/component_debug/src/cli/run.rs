// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cli::format::{
    format_create_error, format_destroy_error, format_resolve_error, format_start_error,
};
use crate::lifecycle::{
    ActionError, CreateError, DestroyError, StartError, create_instance_in_collection,
    destroy_instance_in_collection, resolve_instance, start_instance, start_instance_with_args,
};
use anyhow::{Result, bail, format_err};
#[allow(unused)]
use flex_client::ProxyHasDomain;
use flex_client::Socket;
use flex_fuchsia_component as fcomponent;
use flex_fuchsia_component_decl as fdecl;
use flex_fuchsia_process as fprocess;
use flex_fuchsia_sys2 as fsys;
use fuchsia_url::fuchsia_pkg::AbsoluteComponentUrl;
use futures::future::BoxFuture;
#[allow(unused)]
use futures::{AsyncReadExt, AsyncWriteExt};
use moniker::Moniker;
use std::io::Read;

// This value is fairly arbitrary. The value matches `MAX_BUF` from `fuchsia.io`, but that
// constant is for `fuchsia.io.File` transfers, which are unrelated to these `zx::socket`
// transfers.
const TRANSFER_CHUNK_SIZE: usize = 8192;

async fn copy<W: std::io::Write>(source: Socket, mut sink: W) -> Result<()> {
    #[cfg(not(feature = "fdomain"))]
    let mut source = fuchsia_async::Socket::from_socket(source);
    #[cfg(feature = "fdomain")]
    let mut source = source;
    let mut buf = [0u8; TRANSFER_CHUNK_SIZE];
    loop {
        let bytes_read = source.read(&mut buf).await?;
        if bytes_read == 0 {
            return Ok(());
        }
        sink.write_all(&buf[..bytes_read])?;
        sink.flush()?;
    }
}

// The normal Rust representation of this constant is in fuchsia-runtime, which
// cannot be used on host. Maybe there's a way to move fruntime::HandleType and
// fruntime::HandleInfo to a place that can be used on host?
fn handle_id_for_fd(fd: u32) -> u32 {
    const PA_FD: u32 = 0x30;
    PA_FD | fd << 16
}

struct Stdio {
    local_in: Socket,
    local_out: Socket,
    local_err: Socket,
}

impl Stdio {
    #[cfg(not(feature = "fdomain"))]
    fn new() -> (Self, Vec<fprocess::HandleInfo>) {
        let (local_in, remote_in) = fidl::Socket::create_stream();
        let (local_out, remote_out) = fidl::Socket::create_stream();
        let (local_err, remote_err) = fidl::Socket::create_stream();

        (
            Self { local_in, local_out, local_err },
            vec![
                fprocess::HandleInfo { handle: remote_in.into_handle(), id: handle_id_for_fd(0) },
                fprocess::HandleInfo { handle: remote_out.into_handle(), id: handle_id_for_fd(1) },
                fprocess::HandleInfo { handle: remote_err.into_handle(), id: handle_id_for_fd(2) },
            ],
        )
    }

    #[cfg(feature = "fdomain")]
    fn new(client: &std::sync::Arc<flex_client::Client>) -> (Self, Vec<fprocess::HandleInfo>) {
        use flex_client::HandleBased;
        let (local_in, remote_in) = client.create_stream_socket();
        let (local_out, remote_out) = client.create_stream_socket();
        let (local_err, remote_err) = client.create_stream_socket();

        (
            Self { local_in, local_out, local_err },
            vec![
                fprocess::HandleInfo { handle: remote_in.into_handle(), id: handle_id_for_fd(0) },
                fprocess::HandleInfo { handle: remote_out.into_handle(), id: handle_id_for_fd(1) },
                fprocess::HandleInfo { handle: remote_err.into_handle(), id: handle_id_for_fd(2) },
            ],
        )
    }

    async fn forward(self) {
        let local_in = self.local_in;
        let local_out = self.local_out;
        let local_err = self.local_err;

        let mut local_in = flex_client::socket_to_async(local_in);

        std::thread::spawn(move || {
            let mut term_in = std::io::stdin().lock();
            let mut buf = [0u8; TRANSFER_CHUNK_SIZE];
            let mut executor = fuchsia_async::LocalExecutorBuilder::new().build();
            loop {
                let bytes_read = term_in.read(&mut buf)?;
                if bytes_read == 0 {
                    return Ok::<(), anyhow::Error>(());
                }

                executor.run_singlethreaded(local_in.write_all(&buf[..bytes_read]))?;
            }
        });

        std::thread::spawn(move || {
            let mut executor = fuchsia_async::LocalExecutorBuilder::new().build();
            let _result: Result<()> = executor
                .run_singlethreaded(async move { copy(local_err, std::io::stderr()).await });
        });

        std::thread::spawn(move || {
            let mut executor = fuchsia_async::LocalExecutorBuilder::new().build();
            let _result: Result<()> = executor
                .run_singlethreaded(async move { copy(local_out, std::io::stdout().lock()).await });
            std::process::exit(0);
        });

        // If we're following stdio, we just wait forever. When stdout is
        // closed, the whole process will exit.
        let () = futures::future::pending().await;
    }
}

pub async fn run_cmd<W: std::io::Write>(
    moniker: Moniker,
    url: AbsoluteComponentUrl,
    recreate: bool,
    connect_stdio: bool,
    config_overrides: Vec<fdecl::ConfigOverride>,
    lifecycle_controller_factory: impl Fn()
        -> BoxFuture<'static, Result<fsys::LifecycleControllerProxy>>,
    mut writer: W,
) -> Result<()> {
    let lifecycle_controller = lifecycle_controller_factory().await?;
    let parent = moniker
        .parent()
        .ok_or_else(|| format_err!("Error: {} does not reference a dynamic instance", moniker))?;
    let leaf = moniker
        .leaf()
        .ok_or_else(|| format_err!("Error: {} does not reference a dynamic instance", moniker))?;
    let child_name = leaf.name();
    let collection = leaf
        .collection()
        .ok_or_else(|| format_err!("Error: {} does not reference a dynamic instance", moniker))?;

    if recreate {
        // First try to destroy any existing instance at this monker.
        match destroy_instance_in_collection(&lifecycle_controller, &parent, collection, child_name)
            .await
        {
            Ok(()) => {
                writeln!(writer, "Destroyed existing component instance at {}...", moniker)?;
            }
            Err(DestroyError::ActionError(ActionError::InstanceNotFound))
            | Err(DestroyError::ActionError(ActionError::InstanceNotResolved)) => {
                // No resolved component exists at this moniker. Nothing to do.
            }
            Err(e) => return Err(format_destroy_error(&moniker, e)),
        }
    }

    writeln!(writer, "URL: {}", url)?;
    writeln!(writer, "Moniker: {}", moniker)?;
    writeln!(writer, "Creating component instance...")?;

    // First try to use StartWithArgs

    let (mut maybe_stdio, numbered_handles) = if connect_stdio {
        #[cfg(not(feature = "fdomain"))]
        let (stdio, numbered_handles) = Stdio::new();
        #[cfg(feature = "fdomain")]
        let (stdio, numbered_handles) = Stdio::new(&lifecycle_controller.domain());
        (Some(stdio), Some(numbered_handles))
    } else {
        (None, Some(vec![]))
    };

    let create_result = create_instance_in_collection(
        &lifecycle_controller,
        &parent,
        collection,
        child_name,
        &url,
        config_overrides.clone(),
        None,
    )
    .await;

    match create_result {
        Err(CreateError::InstanceAlreadyExists) => {
            bail!(
                "\nError: {} already exists.\nUse --recreate to destroy and create a new instance, or provide a different moniker.\n",
                moniker
            )
        }
        Err(e) => {
            return Err(format_create_error(&moniker, &parent, collection, e));
        }
        Ok(()) => {}
    }

    writeln!(writer, "Resolving component instance...")?;
    if let Err(e) = resolve_instance(&lifecycle_controller, &moniker).await {
        writeln!(writer, "Cleaning up component instance...")?;
        if let Err(de) =
            destroy_instance_in_collection(&lifecycle_controller, &parent, collection, child_name)
                .await
        {
            writeln!(writer, "Warning: Failed to clean up component instance: {:?}", de)?;
        }
        return Err(format_resolve_error(&moniker, e));
    }

    writeln!(writer, "Starting component instance...")?;
    let start_args = fcomponent::StartChildArgs { numbered_handles, ..Default::default() };
    let res = start_instance_with_args(&lifecycle_controller, &moniker, start_args).await;
    if let Err(StartError::ActionError(ActionError::Fidl(_e))) = &res {
        // A FIDL error here could indicate that we're talking to a version of component manager
        // that does not support `fuchsia.sys2/LifecycleController.StartInstanceWithArgs`. Let's
        // try again with `fuchsia.sys2/LifecycleController.StartInstance`.

        // Component manager will close the lifecycle controller when it encounters a FIDL error,
        // so we need to create a new one.
        let lifecycle_controller = lifecycle_controller_factory().await?;

        if connect_stdio {
            // We want to provide stdio handles to the component, but this is only possible when
            // creating an instance when we have to use the legacy `StartInstance`. Delete and
            // recreate the component, providing the handles to the create call.

            #[cfg(not(feature = "fdomain"))]
            let (stdio, numbered_handles) = Stdio::new();
            #[cfg(feature = "fdomain")]
            let (stdio, numbered_handles) = Stdio::new(&lifecycle_controller.domain());
            maybe_stdio = Some(stdio);
            let create_args = fcomponent::CreateChildArgs {
                numbered_handles: Some(numbered_handles),
                ..Default::default()
            };

            destroy_instance_in_collection(&lifecycle_controller, &parent, collection, child_name)
                .await?;
            create_instance_in_collection(
                &lifecycle_controller,
                &parent,
                collection,
                child_name,
                &url,
                config_overrides,
                Some(create_args),
            )
            .await?;
            resolve_instance(&lifecycle_controller, &moniker)
                .await
                .map_err(|e| format_resolve_error(&moniker, e))?;
        }

        let _stop_future = start_instance(&lifecycle_controller, &moniker)
            .await
            .map_err(|e| format_start_error(&moniker, e))?;
    } else {
        let _stop_future = res.map_err(|e| format_start_error(&moniker, e))?;
    }

    if let Some(stdio) = maybe_stdio {
        stdio.forward().await;
    }

    writeln!(writer, "Component instance is running!")?;

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use flex_fuchsia_sys2 as fsys;
    use futures::{FutureExt, TryStreamExt};

    fn setup_fake_lifecycle_controller_ok(
        expected_parent_moniker: &'static str,
        expected_collection: &'static str,
        expected_name: &'static str,
        expected_url: &'static str,
        expected_moniker: &'static str,
        expect_destroy: bool,
    ) -> fsys::LifecycleControllerProxy {
        let (lifecycle_controller, mut stream) =
            create_proxy_and_stream::<fsys::LifecycleControllerMarker>();
        fuchsia_async::Task::local(async move {
            if expect_destroy {
                let req = stream.try_next().await.unwrap().unwrap();
                match req {
                    fsys::LifecycleControllerRequest::DestroyInstance {
                        parent_moniker,
                        child,
                        responder,
                    } => {
                        assert_eq!(
                            Moniker::parse_str(expected_parent_moniker),
                            Moniker::parse_str(&parent_moniker)
                        );
                        assert_eq!(expected_name, child.name);
                        assert_eq!(expected_collection, child.collection.unwrap());
                        responder.send(Ok(())).unwrap();
                    }
                    _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
                }
            }

            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::CreateInstance {
                    parent_moniker,
                    collection,
                    decl,
                    responder,
                    args: _,
                } => {
                    assert_eq!(
                        Moniker::parse_str(expected_parent_moniker),
                        Moniker::parse_str(&parent_moniker)
                    );
                    assert_eq!(expected_collection, collection.name);
                    assert_eq!(expected_name, decl.name.unwrap());
                    assert_eq!(expected_url, decl.url.unwrap());
                    responder.send(Ok(())).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }

            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::ResolveInstance { moniker, responder } => {
                    assert_eq!(Moniker::parse_str(expected_moniker), Moniker::parse_str(&moniker));
                    responder.send(Ok(())).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }

            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::StartInstanceWithArgs {
                    moniker,
                    binder: _,
                    args: _,
                    responder,
                } => {
                    assert_eq!(Moniker::parse_str(expected_moniker), Moniker::parse_str(&moniker));
                    responder.send(Ok(())).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }
        })
        .detach();
        lifecycle_controller
    }

    fn setup_fake_lifecycle_controller_fail(
        expected_parent_moniker: &'static str,
        expected_collection: &'static str,
        expected_name: &'static str,
        expected_url: &'static str,
    ) -> fsys::LifecycleControllerProxy {
        let (lifecycle_controller, mut stream) =
            create_proxy_and_stream::<fsys::LifecycleControllerMarker>();
        fuchsia_async::Task::local(async move {
            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::DestroyInstance {
                    parent_moniker,
                    child,
                    responder,
                } => {
                    assert_eq!(
                        Moniker::parse_str(expected_parent_moniker),
                        Moniker::parse_str(&parent_moniker)
                    );
                    assert_eq!(expected_name, child.name);
                    assert_eq!(expected_collection, child.collection.unwrap());
                    responder.send(Ok(())).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }

            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::CreateInstance {
                    parent_moniker,
                    collection,
                    decl,
                    responder,
                    args: _,
                } => {
                    assert_eq!(
                        Moniker::parse_str(expected_parent_moniker),
                        Moniker::parse_str(&parent_moniker)
                    );
                    assert_eq!(expected_collection, collection.name);
                    assert_eq!(expected_name, decl.name.unwrap());
                    assert_eq!(expected_url, decl.url.unwrap());
                    responder.send(Err(fsys::CreateError::InstanceAlreadyExists)).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }
        })
        .detach();
        lifecycle_controller
    }

    fn setup_fake_lifecycle_controller_recreate(
        expected_parent_moniker: &'static str,
        expected_collection: &'static str,
        expected_name: &'static str,
        expected_url: &'static str,
        expected_moniker: &'static str,
    ) -> fsys::LifecycleControllerProxy {
        let (lifecycle_controller, mut stream) =
            create_proxy_and_stream::<fsys::LifecycleControllerMarker>();
        fuchsia_async::Task::local(async move {
            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::DestroyInstance {
                    parent_moniker,
                    child,
                    responder,
                } => {
                    assert_eq!(
                        Moniker::parse_str(expected_parent_moniker),
                        Moniker::parse_str(&parent_moniker)
                    );
                    assert_eq!(expected_name, child.name);
                    assert_eq!(expected_collection, child.collection.unwrap());
                    responder.send(Ok(())).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }

            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::CreateInstance {
                    parent_moniker,
                    collection,
                    decl,
                    responder,
                    args: _,
                } => {
                    assert_eq!(
                        Moniker::parse_str(expected_parent_moniker),
                        Moniker::parse_str(&parent_moniker)
                    );
                    assert_eq!(expected_collection, collection.name);
                    assert_eq!(expected_name, decl.name.unwrap());
                    assert_eq!(expected_url, decl.url.unwrap());
                    responder.send(Ok(())).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }

            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::ResolveInstance { moniker, responder } => {
                    assert_eq!(Moniker::parse_str(expected_moniker), Moniker::parse_str(&moniker));
                    responder.send(Ok(())).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }

            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::StartInstanceWithArgs {
                    moniker,
                    binder: _,
                    args: _,
                    responder,
                } => {
                    assert_eq!(Moniker::parse_str(expected_moniker), Moniker::parse_str(&moniker));
                    responder.send(Ok(())).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }
        })
        .detach();
        lifecycle_controller
    }

    #[fuchsia::test]
    async fn test_ok() -> Result<()> {
        let mut output = Vec::new();
        let lifecycle_controller = setup_fake_lifecycle_controller_ok(
            "/some",
            "collection",
            "name",
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm",
            "/some/collection:name",
            true,
        );
        let response = run_cmd(
            "/some/collection:name".try_into().unwrap(),
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm".try_into().unwrap(),
            true,
            false,
            vec![],
            move || {
                let lifecycle_controller = lifecycle_controller.clone();
                async move { Ok(lifecycle_controller) }.boxed()
            },
            &mut output,
        )
        .await;
        response.unwrap();
        Ok(())
    }

    #[fuchsia::test]
    async fn test_name() -> Result<()> {
        let mut output = Vec::new();
        let lifecycle_controller = setup_fake_lifecycle_controller_ok(
            "/core",
            "ffx-laboratory",
            "foobar",
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm",
            "/core/ffx-laboratory:foobar",
            false,
        );
        let response = run_cmd(
            "/core/ffx-laboratory:foobar".try_into().unwrap(),
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm".try_into().unwrap(),
            false,
            false,
            vec![],
            move || {
                let lifecycle_controller = lifecycle_controller.clone();
                async move { Ok(lifecycle_controller) }.boxed()
            },
            &mut output,
        )
        .await;
        response.unwrap();
        Ok(())
    }

    #[fuchsia::test]
    async fn test_fail() -> Result<()> {
        let mut output = Vec::new();
        let lifecycle_controller = setup_fake_lifecycle_controller_fail(
            "/core",
            "ffx-laboratory",
            "test",
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm",
        );
        let response = run_cmd(
            "/core/ffx-laboratory:test".try_into().unwrap(),
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm".try_into().unwrap(),
            true,
            false,
            vec![],
            move || {
                let lifecycle_controller = lifecycle_controller.clone();
                async move { Ok(lifecycle_controller) }.boxed()
            },
            &mut output,
        )
        .await;
        response.unwrap_err();
        Ok(())
    }

    #[fuchsia::test]
    async fn test_recreate() -> Result<()> {
        let mut output = Vec::new();
        let lifecycle_controller = setup_fake_lifecycle_controller_recreate(
            "/core",
            "ffx-laboratory",
            "test",
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm",
            "/core/ffx-laboratory:test",
        );
        let response = run_cmd(
            "/core/ffx-laboratory:test".try_into().unwrap(),
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm".try_into().unwrap(),
            true,
            false,
            vec![],
            move || {
                let lifecycle_controller = lifecycle_controller.clone();
                async move { Ok(lifecycle_controller) }.boxed()
            },
            &mut output,
        )
        .await;
        response.unwrap();
        Ok(())
    }

    fn setup_fake_lifecycle_controller_resolve_fail_cleanup(
        expected_parent_moniker: &'static str,
        expected_collection: &'static str,
        expected_name: &'static str,
        expected_url: &'static str,
        expected_moniker: &'static str,
        destroy_sender: futures::channel::oneshot::Sender<()>,
    ) -> fsys::LifecycleControllerProxy {
        let (lifecycle_controller, mut stream) =
            create_proxy_and_stream::<fsys::LifecycleControllerMarker>();
        fuchsia_async::Task::local(async move {
            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::CreateInstance {
                    parent_moniker,
                    collection,
                    decl,
                    responder,
                    ..
                } => {
                    assert_eq!(
                        Moniker::parse_str(expected_parent_moniker),
                        Moniker::parse_str(&parent_moniker)
                    );
                    assert_eq!(expected_collection, collection.name);
                    assert_eq!(expected_name, decl.name.unwrap());
                    assert_eq!(expected_url, decl.url.unwrap());
                    responder.send(Ok(())).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }

            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::ResolveInstance { moniker, responder } => {
                    assert_eq!(Moniker::parse_str(expected_moniker), Moniker::parse_str(&moniker));
                    responder.send(Err(fsys::ResolveError::PackageNotFound)).unwrap();
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }

            let req = stream.try_next().await.unwrap().unwrap();
            match req {
                fsys::LifecycleControllerRequest::DestroyInstance {
                    parent_moniker,
                    child,
                    responder,
                } => {
                    assert_eq!(
                        Moniker::parse_str(expected_parent_moniker),
                        Moniker::parse_str(&parent_moniker)
                    );
                    assert_eq!(expected_name, child.name);
                    assert_eq!(expected_collection, child.collection.unwrap());
                    responder.send(Ok(())).unwrap();
                    let _ = destroy_sender.send(());
                }
                _ => panic!("Unexpected Lifecycle Controller request: {:?}", req),
            }
        })
        .detach();
        lifecycle_controller
    }

    #[fuchsia::test]
    async fn test_resolve_fail_cleanup() -> Result<()> {
        let mut output = Vec::new();
        let (destroy_sender, mut destroy_receiver) = futures::channel::oneshot::channel::<()>();

        let lifecycle_controller = setup_fake_lifecycle_controller_resolve_fail_cleanup(
            "/core",
            "ffx-laboratory",
            "test",
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm",
            "/core/ffx-laboratory:test",
            destroy_sender,
        );
        let response = run_cmd(
            "/core/ffx-laboratory:test".try_into().unwrap(),
            "fuchsia-pkg://fuchsia.com/test#meta/test.cm".try_into().unwrap(),
            false,
            false,
            vec![],
            move || {
                let lifecycle_controller = lifecycle_controller.clone();
                async move { Ok(lifecycle_controller) }.boxed()
            },
            &mut output,
        )
        .await;

        assert!(response.is_err());
        // Verify that destroy was called by checking the channel
        assert!(destroy_receiver.try_recv().unwrap().is_some());
        Ok(())
    }
}
