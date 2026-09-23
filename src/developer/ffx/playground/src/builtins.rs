// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_io as fio;
use futures::FutureExt;
use futures::future::{Either, poll_fn};
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Poll, Waker};

use crate::error::{Result, RuntimeError};
use crate::interpreter::{
    Exception, FSError, IOError, Interpreter, InterpreterInner, LibraryProtocolNameExt as _,
    MessageError, SymlinkPolicy, canonicalize_path,
};
use crate::value::{
    InUseHandle, Invocable, PlaygroundValue, ReplayableIterator, ReplayableIteratorCursor, Value,
    ValueExt,
};

impl Interpreter {
    /// Add built-in commands to the playground's global scope.
    pub(crate) async fn add_builtins(&self, executor_fut: &mut (impl Future<Output = ()> + Unpin)) {
        let fut = async {
            let inner_weak = Arc::downgrade(&self.inner);
            let fs_root_getter =
                self.get_runnable("$fs_root").await.expect("Could not compile fs_root getter");
            let pwd_getter = self.get_runnable("$pwd").await.expect("Could not compile pwd getter");
            let pwd_getter_clone = pwd_getter.clone();
            let fs_root_getter_clone = fs_root_getter.clone();
            self.add_command("open", move |mut args, underscore| {
                let inner_weak = inner_weak.clone();
                let fs_root_getter = fs_root_getter.clone();
                let pwd_getter = pwd_getter.clone();
                async move {
                    let Some(inner) = inner_weak.upgrade() else {
                        return Err(anyhow!("Interpreter died"));
                    };
                    let Some(arg) = args.pop().or(underscore) else {
                        return Err(anyhow!("open requires exactly one argument or an input"));
                    };
                    if !args.is_empty() {
                        return Err(anyhow!("open requires at most one argument"));
                    }

                    let path = match arg {
                        Value::String(path) => path,
                        _ => return Err(anyhow!("open argument must be a path")),
                    };
                    let fs_root = fs_root_getter().await?;
                    let pwd = pwd_getter().await?;

                    Ok(inner.open(path, fs_root, pwd).await?)
                }
            });

            let inner_weak = Arc::downgrade(&self.inner);
            let fs_root_getter = fs_root_getter_clone.clone();
            self.add_command("req", move |mut args, under| {
                let inner_weak = inner_weak.clone();
                let fs_root_getter = fs_root_getter.clone();
                async move {
                    let fs_root = fs_root_getter().await?;
                    let Some(fs_root) = fs_root.to_in_use_handle() else {
                        return Err(anyhow!("$fs_root wasn't a handle"));
                    };
                    let fdomain = fs_root.fdomain()?;
                    let Some(inner) = inner_weak.upgrade() else {
                        return Err(anyhow!("Interpreter died"));
                    };

                    if args.len() != 1 {
                        return Err(anyhow!("req takes exactly one argument"));
                    }

                    let closure = args.pop().unwrap();

                    let (server, client) = InUseHandle::new_endpoints(fdomain);
                    let server = Value::OutOfLine(PlaygroundValue::InUseHandle(server));
                    let client = Value::OutOfLine(PlaygroundValue::InUseHandle(client));
                    let _ = inner.invoke_value(closure, vec![server], under).await?;
                    Ok(client)
                }
            });

            let inner_weak = Arc::downgrade(&self.inner);
            self.add_command("read", move |mut args, under| {
                let inner_weak = inner_weak.clone();
                async move {
                    let Some(inner) = inner_weak.upgrade() else {
                        return Err(anyhow!("Interpreter died"));
                    };

                    let Some(value) = args.pop().or(under) else {
                        return Err(anyhow!("read takes one argument or an input"));
                    };

                    if !args.is_empty() {
                        return Err(anyhow!("read takes at most one argument"));
                    }

                    if let Ok(client) = value
                        .try_client_channel(inner.lib_namespace(), &fio::FileMarker::library_name())
                    {
                        let proxy = fio::FileProxy::from_channel(client);
                        Ok(Value::OutOfLine(PlaygroundValue::Iterator(ReplayableIterator::from(
                            FileCursor(
                                Arc::new(FileCursorInner {
                                    proxy,
                                    cache: Mutex::new(FileCursorCache {
                                        cached: VecDeque::new(),
                                        cache_positions: [(0, 1)].into_iter().collect(),
                                    }),
                                }),
                                0,
                            ),
                        ))))
                    } else {
                        Err(anyhow!("value cannot be read"))
                    }
                }
            });

            let Value::OutOfLine(PlaygroundValue::Invocable(pwd_setter)) =
                self.run(r"\x {$pwd = $x}").await.expect("Could not build pwd setter")
            else {
                unreachable!("cd setter wasn't an invocable");
            };
            let inner_weak = Arc::downgrade(&self.inner);
            let pwd_getter = pwd_getter_clone.clone();
            let fs_root_getter = fs_root_getter_clone.clone();
            self.add_command("cd", move |args, _| {
                let pwd_setter = pwd_setter.clone();
                let pwd_getter = pwd_getter.clone();
                let inner_weak = inner_weak.clone();
                let fs_root_getter = fs_root_getter.clone();
                async move {
                    let Some(inner) = inner_weak.upgrade() else {
                        return Err(anyhow!("Interpreter died"));
                    };

                    let mut pwd = pwd_getter().await?;
                    let fs_root = fs_root_getter().await?;

                    let Result::<[Value; 1], _>::Ok([path]) = args.try_into() else {
                        return Err(anyhow!("cd takes exactly one argument"));
                    };

                    let Value::String(path) = path else {
                        return Err(anyhow!("path must be a string"));
                    };

                    let path = canonicalize_path(path, pwd.duplicate())?;
                    let _ = inner
                        .path_info(
                            path.clone(),
                            fs_root,
                            pwd,
                            fio::NodeAttributesQuery::empty(),
                            SymlinkPolicy::Follow,
                        )
                        .await
                        .map_err(|e| anyhow!("Cannot access {path}: {e}"))?;

                    let _ = pwd_setter
                        .invoke(vec![Value::String(path)], None)
                        .await
                        .expect("cd setter failed!");
                    Ok(Value::Null)
                }
            });

            let inner_weak = Arc::downgrade(&self.inner);
            let pwd_getter = pwd_getter_clone.clone();
            // Returns an iterator of objects of the form
            // { "name": Value::String, "kind": Value::String }
            self.add_command("ls", move |args, _| {
                let pwd_getter = pwd_getter.clone();
                let inner_weak = inner_weak.clone();
                let fs_root_getter = fs_root_getter_clone.clone();
                async move {
                    let Some(inner) = inner_weak.upgrade() else {
                        return Err(anyhow!("Interpreter died"));
                    };

                    let pwd = pwd_getter().await?;
                    let mut fs_root = fs_root_getter().await?;

                    let path = if args.is_empty() {
                        ".".to_owned()
                    } else if args.len() > 1 {
                        return Err(anyhow!("ls takes at most one argument"));
                    } else {
                        let [path] = args.try_into().unwrap();

                        let Value::String(path) = path else {
                            return Err(anyhow!("path must be a string"));
                        };

                        path
                    };

                    let info = inner
                        .path_info(
                            path.clone(),
                            fs_root.duplicate(),
                            pwd,
                            fio::NodeAttributesQuery::PROTOCOLS,
                            SymlinkPolicy::Follow,
                        )
                        .await?;
                    let protocols = info
                        .attributes
                        .immutable_attributes
                        .protocols
                        .unwrap_or(fio::NodeProtocolKinds::empty());

                    return if protocols.contains(fio::NodeProtocolKinds::DIRECTORY) {
                        let dir = inner.open_directory(info.hard_path, fs_root).await?;
                        Ok(Value::List(
                            fuchsia_fs_fdomain::directory::readdir(&dir)
                                .await?
                                .into_iter()
                                .map(|x| {
                                    Value::Object(vec![
                                        ("name".to_owned(), Value::String(x.name)),
                                        ("kind".to_owned(), Value::String(format!("{:?}", x.kind))),
                                    ])
                                })
                                .collect(),
                        ))
                    } else if protocols.contains(fio::NodeProtocolKinds::SYMLINK) {
                        unreachable!("path_info was supposed to traverse symlinks but didn't!");
                    } else {
                        let name = path.rsplit_once("/").unwrap().1.to_owned();
                        let kind = if protocols.contains(fio::NodeProtocolKinds::FILE) {
                            fio::DirentType::File
                        } else if protocols.contains(fio::NodeProtocolKinds::CONNECTOR) {
                            fio::DirentType::Service
                        } else {
                            fio::DirentType::Unknown
                        };

                        Ok(Value::Object(vec![
                            ("name".to_owned(), Value::String(name)),
                            ("kind".to_owned(), Value::String(format!("{:?}", kind))),
                        ]))
                    };
                }
            });

            let inner_weak = Arc::downgrade(&self.inner);
            self.add_command("srv", move |mut args, underscore| {
                let inner_weak = inner_weak.clone();
                async move {
                    let server = args
                        .pop()
                        .or(underscore)
                        .ok_or_else(|| anyhow!("Must supply an argument to serve"))?;
                    if !args.is_empty() {
                        return Err(anyhow!("serve takes at most one argument"));
                    }

                    let ch = server
                        .try_server_channel()
                        .map_err(|_| anyhow!("Value is not a FIDL server"))?;

                    Ok(Value::OutOfLine(PlaygroundValue::Iterator(
                        ServeCursor(Mutex::new(ServeCursorInner::Unpolled(
                            Arc::new(ch),
                            inner_weak.clone(),
                        )))
                        .into(),
                    )))
                }
            });

            self.run("def cat(path) {open $path | read}")
                .await
                .expect("Definition of `cat` builtin failed");
        };

        let Either::Left(_) = futures::future::select(pin!(fut), executor_fut).await else {
            unreachable!("Executor hung up early");
        };
    }
}

struct ServeCursor(Mutex<ServeCursorInner>);

enum ServeCursorInner {
    Unpolled(Arc<fdomain_client::Channel>, Weak<InterpreterInner>),
    Waiting(Vec<Waker>, Arc<ServeCursor>),
    Stored(Result<Option<Value>>, Arc<ServeCursor>),
}

impl ReplayableIteratorCursor for ServeCursor {
    fn next(
        self: Arc<Self>,
    ) -> (
        futures::prelude::future::BoxFuture<'static, Result<Option<Value>>>,
        Arc<dyn ReplayableIteratorCursor>,
    ) {
        enum NextAction<A, B, C, D> {
            StartPoll(A, B, C),
            WaitForPoll(D),
        }

        let next_action = match &mut *self.0.lock().unwrap() {
            inner @ ServeCursorInner::Unpolled(_, _) => {
                let ServeCursorInner::Unpolled(channel, weak_inner) = inner else { unreachable!() };
                let next = Arc::new(ServeCursor(Mutex::new(ServeCursorInner::Unpolled(
                    Arc::clone(channel),
                    weak_inner.clone(),
                ))));
                let channel = Arc::clone(channel);
                let weak_inner = weak_inner.clone();
                *inner = ServeCursorInner::Waiting(Vec::new(), Arc::clone(&next));

                NextAction::StartPoll(channel, weak_inner, next)
            }
            ServeCursorInner::Waiting(_, next) => NextAction::WaitForPoll(Arc::clone(next)),
            ServeCursorInner::Stored(value, next) => {
                let value =
                    value.as_mut().map(|x| x.as_mut().map(Value::duplicate)).map_err(|x| x.clone());
                return (
                    async move { value }.boxed(),
                    Arc::clone(next) as Arc<dyn ReplayableIteratorCursor>,
                );
            }
        };

        let (channel, weak_inner, next) = match next_action {
            NextAction::StartPoll(a, b, c) => (a, b, c),
            NextAction::WaitForPoll(next) => {
                let fut = poll_fn(move |ctx| match &mut *self.0.lock().unwrap() {
                    ServeCursorInner::Unpolled(_, _) => {
                        unreachable!("Serve cursor went from waiting to unpolled!")
                    }
                    ServeCursorInner::Waiting(wakers, _) => {
                        wakers.push(ctx.waker().clone());
                        Poll::Pending
                    }
                    ServeCursorInner::Stored(value, _) => Poll::Ready(
                        value
                            .as_mut()
                            .map(|x| x.as_mut().map(Value::duplicate))
                            .map_err(|x| x.clone()),
                    ),
                })
                .boxed();
                return (fut, next as Arc<dyn ReplayableIteratorCursor>);
            }
        };

        let fetch_value = async move {
            let buf = match channel.recv_msg().await {
                Ok(buf) => buf,
                Err(fdomain_client::Error::FDomain(fdomain_client::FDomainError::TargetError(
                    e,
                ))) if e == zx_status::Status::PEER_CLOSED.into_raw() => {
                    return Ok(None);
                }
                Err(e) => {
                    return Err(IOError::ChannelRead(e).into());
                }
            };
            let interpreter = weak_inner.upgrade().ok_or_else(|| RuntimeError::InterpreterDied)?;
            let (bytes, handles) = buf.split();

            let (header, value) = fidl_codec_fdomain::decode_request(
                interpreter.lib_namespace(),
                &bytes,
                handles.into_iter().map(Into::into).collect(),
            )
            .map_err(|e| MessageError::DecodeRequestFailed(Arc::new(e)))?;

            let mut value = value.upcast();

            let (protocol_name, method) =
                interpreter.lib_namespace().lookup_method_ordinal(header.ordinal).expect("FIDL Codec decoded a message then immediately claimed not to know the ordinal!");
            if let Some(ty) = method.response.clone() {
                if !matches!(value, Value::Object(_)) {
                    value = Value::Object(vec![("_".to_owned(), value)])
                }

                let Value::Object(fields) = &mut value else { unreachable!() };

                let txid = header.tx_id;
                let method_name = method.name.clone();

                let state = Mutex::new(Some((channel, weak_inner, ty, protocol_name, method_name)));
                fields.push((
                    "@".to_owned(),
                    Value::OutOfLine(PlaygroundValue::Invocable(Invocable::new(Arc::new(
                        move |mut args, underscore| {
                            let state = state.lock().unwrap().take();
                            async move {
                                let args_len = args.len();
                                let (channel, weak_inner, ty, protocol_name, method_name) =
                                    state.ok_or_else(|| MessageError::ResponseAlreadySent(txid))?;
                                let response = args.pop().or(underscore).ok_or_else(|| {
                                    Exception::WrongArgumentCount(
                                        format!("responder<{protocol_name}/{method_name}>"),
                                        1,
                                        args_len,
                                    )
                                })?;
                                if !args.is_empty() {
                                    return Err(Exception::WrongArgumentCount(
                                        format!("responder<{protocol_name}/{method_name}>"),
                                        1,
                                        args_len,
                                    )
                                    .into_err());
                                }
                                let interpreter = weak_inner
                                    .upgrade()
                                    .ok_or_else(|| RuntimeError::InterpreterDied)?;
                                let response =
                                    response.to_fidl_value(interpreter.lib_namespace(), &ty)?;

                                let (bytes, handles) = fidl_codec_fdomain::encode_response(
                                    interpreter.lib_namespace(),
                                    txid,
                                    &protocol_name,
                                    &method_name,
                                    response,
                                )
                                .map_err(|e| {
                                    MessageError::EncodeReplyFailed(
                                        protocol_name.to_owned(),
                                        method_name.to_owned(),
                                        Arc::new(e),
                                    )
                                })?;
                                channel
                                    .fdomain_write_etc(
                                        &bytes,
                                        handles.into_iter().map(Into::into).collect(),
                                    )
                                    .await
                                    .map_err(IOError::ChannelWrite)?;
                                Ok(Value::Null)
                            }
                            .boxed()
                        },
                    )))),
                ))
            }

            Ok(Some(value))
        };

        (
            async move {
                let mut value = fetch_value.await;
                let mut inner = self.0.lock().unwrap();

                let ServeCursorInner::Waiting(_, next) = &*inner else {
                    panic!("Race in Serve inner!");
                };
                let next = Arc::clone(next);
                let value_dup = value
                    .as_mut()
                    .map(|x| x.as_mut().map(Value::duplicate))
                    .map_err(|x: &mut crate::error::Error| x.clone());
                let ServeCursorInner::Waiting(waiters, _) =
                    std::mem::replace(&mut *inner, ServeCursorInner::Stored(value_dup, next))
                else {
                    unreachable!()
                };

                waiters.into_iter().for_each(Waker::wake);

                value
            }
            .boxed(),
            next,
        )
    }
}

/// Cached data read from a file that's being read through a ReplayableIterator
struct FileCursorCache {
    /// Contains bytes read from the file. The section of the file represented
    /// starts at the offset of the lowest key in [`cached_positions`].
    cached: VecDeque<u8>,
    /// Positions of the file that we'd like to cache. Each key in the map is an
    /// offset within the file itself where a [`FileCursor`] is currently
    /// pointed. Each value is how many such cursors are pointed there. The
    /// lowest-value key is the offset from which the data in [`cached`] was read.
    cache_positions: BTreeMap<usize, usize>,
}

/// Shared portion of [`FileCursor`]
struct FileCursorInner {
    proxy: fio::FileProxy,
    cache: Mutex<FileCursorCache>,
}

impl FileCursorInner {
    /// Amount of data to attempt to read every time we read from the file.
    const READ_BLOCK_SIZE: u64 = 64;

    /// Read data from the underlying file at the given offset. Makes use of our
    /// contained cache to avoid duplicate reads.
    async fn read(&self, pos: usize) -> Result<Option<Value>> {
        let mut bytes = Vec::new();
        loop {
            {
                let mut cache = self.cache.lock().unwrap();
                let cache_pos = *cache
                    .cache_positions
                    .first_key_value()
                    .expect("File cursor unregistered position from cache!")
                    .0;
                if !bytes.is_empty() {
                    cache.cached.extend(bytes.drain(..));
                }
                assert!(cache_pos <= pos, "Iterator cursor precedes retained state!");
                let pos = pos - cache_pos;
                if let Some(byte) = cache.cached.get(pos).copied() {
                    return Ok(Some(Value::U8(byte)));
                }
            }
            bytes = fuchsia_fs_fdomain::file::read_num_bytes(&self.proxy, Self::READ_BLOCK_SIZE)
                .await
                .map_err(|e| FSError::FileReadError(Arc::new(e)))?;
            if bytes.is_empty() {
                return Ok(None);
            }
        }
    }
}

/// [`ReplayableIteratorCursor`] that yields the bytes of a file.
struct FileCursor(Arc<FileCursorInner>, usize);

impl Drop for FileCursor {
    fn drop(&mut self) {
        // Tell the cache that there is one less cursor looking at the given offset.
        let mut cache = self.0.cache.lock().unwrap();
        *cache.cache_positions.get_mut(&self.1).expect("File cursor has no cache position!") -= 1;

        // Get the offset of the leftmost byte of the file which is currently in the cache.
        let start = *cache.cache_positions.first_key_value().unwrap().0;

        // If no cursors are currently pointed to the leftmost cached position,
        // discard the entry for that position. Repeat this until we have a
        // leftmost entry that is actually used by an existing cursor.
        while let Some(entry) = cache.cache_positions.first_entry().filter(|x| *x.get() == 0) {
            entry.remove();
        }

        // If we removed the leftmost entry, discard cached data until our cache
        // starts at the new leftmost entry.
        if let Some((&end, _)) = cache.cache_positions.first_key_value().filter(|x| *x.0 != start) {
            let len = cache.cached.len();
            cache.cached.drain(..(std::cmp::min(end - start, len)));
        }
    }
}

impl ReplayableIteratorCursor for FileCursor {
    fn next(
        self: Arc<Self>,
    ) -> (
        futures::prelude::future::BoxFuture<'static, Result<Option<Value>>>,
        Arc<dyn ReplayableIteratorCursor>,
    ) {
        let next = Arc::new(FileCursor(Arc::clone(&self.0), self.1 + 1));
        self.0
            .cache
            .lock()
            .unwrap()
            .cache_positions
            .entry(self.1 + 1)
            .and_modify(|e| *e += 1)
            .or_insert(1);
        let yielder = async move { self.0.read(self.1).await }.boxed();
        (yielder, next)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test::*;
    use std::collections::HashMap;
    use {fdomain_fuchsia_io as fio, fdomain_test_fidlcodec_examples as fctest};

    #[fuchsia::test]
    async fn open() {
        Test::test("open /test")
            .with_fidl()
            .with_standard_test_dirs()
            .await
            .check_async(|value| async move {
                assert!(value.is_client(&fio::DirectoryMarker::library_name()));
                let Value::ClientEnd(endpoint, _, _) = value else {
                    panic!();
                };
                let proxy = fdomain_client::fidl::ClientEnd::<fio::DirectoryMarker>::from(endpoint)
                    .into_proxy();
                let mut dirs = fuchsia_fs_fdomain::directory::readdir(&proxy).await.unwrap();
                dirs.sort_by(|x, y| x.name.cmp(&y.name));
                let [foo, neils_philosophy] = dirs.try_into().unwrap();
                assert_eq!("foo", foo.name);
                assert_eq!("neils_philosophy", neils_philosophy.name);
            })
            .await
    }

    #[fuchsia::test]
    async fn req() {
        Test::test(format!("open /test | req \\i _ @Clone {{ request: $i }}",))
            .with_fidl()
            .with_standard_test_dirs()
            .await
            .check_async(|value| async move {
                let Value::OutOfLine(PlaygroundValue::InUseHandle(i)) = value else {
                    panic!();
                };
                let endpoint = i.take_client(Some("fuchsia.unknown/Cloneable")).unwrap();
                let proxy = fdomain_client::fidl::ClientEnd::<fio::DirectoryMarker>::from(endpoint)
                    .into_proxy();
                let (_, attrs) = proxy
                    .get_attributes(fio::NodeAttributesQuery::PROTOCOLS)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(attrs.protocols.unwrap(), fio::NodeProtocolKinds::DIRECTORY);
            })
            .await
    }

    #[fuchsia::test]
    async fn read() {
        async fn test(path: &str) {
            Test::test(path)
                .with_fidl()
                .with_standard_test_dirs()
                .await
                .check_async(|value| async move {
                    let Value::OutOfLine(PlaygroundValue::Iterator(mut i)) = value else {
                        panic!();
                    };
                    let mut bytes = Vec::new();
                    while let Some(byte) = i.next().await.unwrap() {
                        let Value::U8(byte) = byte else { panic!() };
                        bytes.push(byte);
                    }

                    assert_eq!(crate::test::NEILS_PHILOSOPHY, bytes.as_slice());
                })
                .await
        }
        test("open /test/neils_philosophy | read").await;
        test("open /test/foo/relative_symlink | read").await;
    }

    #[fuchsia::test]
    async fn read_no_pipe() {
        Test::test("read {open /test/neils_philosophy}")
            .with_fidl()
            .with_standard_test_dirs()
            .await
            .check_async(|value| async move {
                let Value::OutOfLine(PlaygroundValue::Iterator(mut i)) = value else {
                    panic!();
                };
                let mut bytes = Vec::new();
                while let Some(byte) = i.next().await.unwrap() {
                    let Value::U8(byte) = byte else { panic!() };
                    bytes.push(byte);
                }

                assert_eq!(crate::test::NEILS_PHILOSOPHY, bytes.as_slice());
            })
            .await
    }

    #[fuchsia::test]
    async fn cd() {
        Test::test("cd /test; let a = $pwd; cd foo; let b = $pwd; cd \"../..\"; let c = $pwd; [$a, $b, $c]")
            .with_fidl()
            .with_standard_test_dirs()
                .await
            .check(|value| {
                let Value::List(value) = value else {
                    panic!();
                };
                let [Value::String(a), Value::String(b), Value::String(c)]: [Value; 3] = value.try_into().unwrap() else {
                    panic!();
                };

                assert_eq!("/test", &a);
                assert_eq!("/test/foo", &b);
                assert_eq!("/", &c);
            }).await
    }

    #[fuchsia::test]
    async fn cd_bad_dir() {
        Test::test("cd /this_folder_doesnt_exist")
            .with_fidl()
            .with_standard_test_dirs()
            .await
            .check_fails(|e| {
                let e = format!("{e}");
                assert!(e.contains("Cannot access /this_folder_doesnt_exist"));
                assert!(e.contains("NOT_FOUND"));
            })
            .await
    }

    #[fuchsia::test]
    async fn ls() {
        async fn test(path: &str) {
            Test::test(path)
                .with_fidl()
                .with_standard_test_dirs()
                .await
                .check(|value| {
                    let Value::List(value) = value else {
                        panic!();
                    };
                    let [Value::Object(a), Value::Object(b)]: [Value; 2] =
                        value.try_into().unwrap()
                    else {
                        panic!();
                    };
                    let mut a: HashMap<_, _> = a.into_iter().collect();
                    let mut b: HashMap<_, _> = b.into_iter().collect();

                    let a_name = a.get("name").unwrap();
                    let Value::String(a_name) = a_name else {
                        panic!();
                    };

                    if a_name != "foo" {
                        std::mem::swap(&mut a, &mut b);
                    }

                    let a_name = a.remove("name").unwrap();
                    let a_kind = a.remove("kind").unwrap();
                    let b_name = b.remove("name").unwrap();
                    let b_kind = b.remove("kind").unwrap();

                    assert!(a.is_empty());
                    assert!(b.is_empty());

                    let Value::String(a_name) = a_name else {
                        panic!();
                    };

                    let Value::String(b_name) = b_name else {
                        panic!();
                    };

                    let Value::String(a_kind) = a_kind else {
                        panic!();
                    };

                    let Value::String(b_kind) = b_kind else {
                        panic!();
                    };

                    assert_eq!(&a_name, "foo");
                    assert_eq!(&b_name, "neils_philosophy");
                    assert_eq!(&a_kind, "Directory");
                    assert_eq!(&b_kind, "File");
                })
                .await
        }
        test("ls /test").await;
        test("ls /test/foo/absolute_symlink").await;
    }

    #[fuchsia::test]
    async fn ls_file() {
        Test::test("ls /test/neils_philosophy")
            .with_fidl()
            .with_standard_test_dirs()
            .await
            .check(|value| {
                let Value::Object(value) = value else {
                    panic!();
                };
                let mut value: HashMap<_, _> = value.into_iter().collect();

                let name = value.remove("name").unwrap();
                let kind = value.remove("kind").unwrap();

                assert!(value.is_empty());

                let Value::String(name) = name else {
                    panic!();
                };

                let Value::String(kind) = kind else {
                    panic!();
                };

                assert_eq!(&name, "neils_philosophy");
                assert_eq!(&kind, "File");
            })
            .await
    }

    #[fuchsia::test]
    async fn serve() {
        Test::test("\\x srv $x")
            .with_fidl()
            .check_async_with_fdomain(|value, fdomain_client| async move {
                let Value::OutOfLine(PlaygroundValue::Invocable(value)) = value else {
                    panic!();
                };
                let (echo, server) = fdomain_client.create_proxy::<fctest::EchoMarker>();
                let server = Value::ServerEnd(
                    Into::<fidl_codec_fdomain::NullableHandle>::into(server).into(),
                    "test.fidlcodec.examples/Echo".to_owned(),
                    None,
                );
                let mut requests = value.invoke(vec![server], None).await.unwrap();
                let requests_dup = requests.duplicate();
                let ((), ()) = futures::future::join(
                    async move {
                        let got = echo.echo_string(Some("Hello")).await.unwrap().unwrap();
                        assert_eq!("Hello", got);
                    },
                    async move {
                        let Value::OutOfLine(PlaygroundValue::Iterator(mut requests)) = requests
                        else {
                            panic!();
                        };

                        let next = requests.next().await.unwrap().unwrap();
                        let Value::Object(next) = next else {
                            panic!();
                        };

                        let [a, b] = next.try_into().unwrap();

                        let (request, responder) = if &a.0 == "@" { (b, a) } else { (a, b) };

                        assert_eq!("value", &request.0);
                        let value = request.1;
                        assert_eq!("@", &responder.0);
                        let responder = responder.1;

                        let Value::String(value) = value else { panic!() };
                        assert_eq!("Hello", &value);

                        let Value::OutOfLine(PlaygroundValue::Invocable(responder)) = responder
                        else {
                            panic!()
                        };

                        let responder_clone = responder.clone();
                        let res = responder
                            .invoke(
                                vec![Value::Object(vec![(
                                    "response".to_owned(),
                                    Value::String(value.clone()),
                                )])],
                                None,
                            )
                            .await
                            .unwrap();
                        assert!(matches!(res, Value::Null));
                        assert!(
                            responder_clone
                                .invoke(
                                    vec![Value::Object(vec![(
                                        "response".to_owned(),
                                        Value::String(value),
                                    )])],
                                    None,
                                )
                                .await
                                .is_err()
                        );

                        assert!(requests.next().await.unwrap().is_none());
                    },
                )
                .await;

                let Value::OutOfLine(PlaygroundValue::Iterator(mut requests)) = requests_dup else {
                    panic!();
                };

                let next = requests.next().await.unwrap().unwrap();
                let Value::Object(next) = next else {
                    panic!();
                };

                let [a, b] = next.try_into().unwrap();

                let (request, responder) = if &a.0 == "@" { (b, a) } else { (a, b) };

                assert_eq!("value", &request.0);
                let value = request.1;
                assert_eq!("@", &responder.0);
                let responder = responder.1;

                let Value::String(value) = value else { panic!() };
                assert_eq!("Hello", &value);

                let Value::OutOfLine(PlaygroundValue::Invocable(responder)) = responder else {
                    panic!()
                };

                let responder_clone = responder.clone();
                assert!(
                    responder_clone
                        .invoke(
                            vec![Value::Object(vec![(
                                "response".to_owned(),
                                Value::String(value),
                            )])],
                            None,
                        )
                        .await
                        .is_err()
                );
            })
            .await
    }

    #[fuchsia::test]
    async fn cat() {
        async fn test(path: &str) {
            Test::test(path)
                .with_fidl()
                .with_standard_test_dirs()
                .await
                .check_async(|value| async move {
                    let Value::OutOfLine(PlaygroundValue::Iterator(mut i)) = value else {
                        panic!();
                    };
                    let mut bytes = Vec::new();
                    while let Some(byte) = i.next().await.unwrap() {
                        let Value::U8(byte) = byte else { panic!() };
                        bytes.push(byte);
                    }

                    assert_eq!(crate::test::NEILS_PHILOSOPHY, bytes.as_slice());
                })
                .await
        }
        test("cat /test/neils_philosophy").await;
        test("cat /test/foo/relative_symlink").await;
    }
}
