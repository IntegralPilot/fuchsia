// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_client::HandleBased as _;
use fdomain_client::fidl::{DiscoverableProtocolMarker, Proxy};
use fdomain_fuchsia_io as fio;
use fidl_codec_fdomain::{ObjectType, Value as FidlValue, library as lib};
use futures::channel::mpsc::{UnboundedSender, unbounded as unbounded_channel};
use futures::channel::oneshot::{Sender as OneshotSender, channel as oneshot_channel};
use futures::future::BoxFuture;
use futures::{Future, FutureExt, Stream, StreamExt};
use regex::Regex;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use thiserror::Error;

use crate::compiler::Visitor;
use crate::error::{Result, RuntimeError};
use crate::frame::GlobalVariables;
use crate::parser::{Mutability, Node, ParseResult, TabHint};
use crate::value::{InUseHandle, Invocable, PlaygroundValue, ReplayableIterator, Value, ValueExt};

/// Errors occurring due to the user misusing the language.
#[derive(Error, Debug, Clone)]
pub enum Exception {
    #[error("'{0}' is declared constant")]
    AssignToConst(String),
    #[error("Object key must be a string")]
    NonStringObjectKey,
    #[error("List index must be a positive integer")]
    NonPositiveIntegerListKey,
    #[error("Index into a FIDL union must be either the member name or `0`")]
    BadUnionKey,
    #[error("List index out of range")]
    ListIndexOutOfRange,
    #[error("Value does not have members or elements")]
    LookupNotSupported,
    #[error("Expression isn't assignable")]
    BadLValue,
    #[error("The '+' operator only applies to numbers and strings")]
    BadAdditionOperands,
    #[error("The '{0}' operator only applies to numbers")]
    BadNumericOperands(&'static str),
    #[error("The '{0}' operator only applies to booleans")]
    BadBooleanOperands(&'static str),
    #[error("Division by zero")]
    DivisionByZero,
    #[error("Condition must evaluate to a boolean")]
    NonBooleanConditional,
    #[error("Value isn't iterable")]
    NotIterable,
    #[error("Wrong number of arguments to '{0}' (need {1} have {2})")]
    WrongArgumentCount(String, usize, usize),
    #[error("Object does not contain '{0}'")]
    NoSuchObjectKey(String),
    #[error("Negation only applies to numbers")]
    BadNegationOperand,
    #[error("Range bound must be an integer")]
    NonNumericRangeBound,
    #[error("Left-unbounded ranges are not yet supported")]
    UnsupportedLeftUnboundedRange,
    #[error("{0} undeclared")]
    VariableUndeclared(String),
    #[error("Value cannot be invoked")]
    NotInvocable,
}

/// Errors occurring during operations on the interpreters internal filesystem.
#[derive(Error, Debug, Clone)]
pub enum FSError {
    #[error("'$fs_root' is not a handle")]
    FSRootNotHandle,
    #[error("'$fs_root' is not a directory")]
    FSRootNotDirectory,
    #[error("Opening path {0} yielded an OnOpen event, which is not a correct response to Open")]
    UnexpectedOnOpenEvent(String),
    #[error("Opening path {0} yielded an event with ordinal {1} before OnRepresentation")]
    UnexpectedEventInsteadOfRepresentation(String, u64),
    #[error("Proxy to path {0} shut down with no representation")]
    NoRepresentation(String),
    #[error("Proxy to path {0} failed to send a representation: {1}")]
    RepresentationFailed(String, fidl::Error),
    #[error("Symlink at {0} did not contain a target")]
    NoSymlinkTarget(String),
    #[error("Symlink at {0} had a non-utf8 target")]
    SymlinkNotUTF(String),
    #[error("Symlink recursion depth exceeded while opening {0}")]
    SymlinkRecursionExceeded(String),
    #[error("'$pwd' is not a string")]
    PwdNotString,
    #[error("'$pwd' is not an absolute path")]
    PwdNotAbsolute,
    #[error("Imported path at {0} is not a file")]
    ImportNotFile(String),
    #[error("Error reading file: {0}")]
    FileReadError(Arc<fuchsia_fs_fdomain::file::ReadError>),
    #[error("Could not get target for symlink at {0}: {1}")]
    SymlinkDescribeFailed(String, fidl::Error),
}

/// Errors occurring during raw IO on handles.
#[derive(Error, Debug, Clone)]
pub enum IOError {
    #[error("Socket read failed: {0}")]
    SocketRead(fdomain_client::Error),
    #[error("Socket write failed: {0}")]
    SocketWrite(fdomain_client::Error),
    #[error("Channel read failed: {0}")]
    ChannelRead(fdomain_client::Error),
    #[error("Channel write failed: {0}")]
    ChannelWrite(fdomain_client::Error),
}

/// Errors occurring when trying to send a FIDL message.
#[derive(Error, Debug, Clone)]
pub enum MessageError {
    #[error("Cannot find definition for protocol {0}")]
    ProtocolNotFound(String),
    #[error("Channel message argument must be a single, named object")]
    MessageArgumentsIncorrect,
    #[error("Channel message not provided")]
    NoMessage,
    #[error("No method {0} in protocol {1}")]
    NoMethodInProtocol(String, String),
    #[error("Response already sent for txid {0}")]
    ResponseAlreadySent(u32),
    #[error("Bad FIDL transaction header: {0}")]
    BadFidlTransactionHeader(fidl::Error),
    #[error("Could not encode FIDL request for method {0}/{1}: {2}")]
    EncodeRequestFailed(String, String, Arc<fidl_codec_fdomain::Error>),
    #[error("Could not decode FIDL reply for method {0}/{1}: {2}")]
    DecodeReplyFailed(String, String, Arc<fidl_codec_fdomain::Error>),
    #[error("Could not encode FIDL reply for method {0}/{1}: {2}")]
    EncodeReplyFailed(String, String, Arc<fidl_codec_fdomain::Error>),
    #[error("Could not decode incoming FIDL request: {0}")]
    DecodeRequestFailed(Arc<fidl_codec_fdomain::Error>),
}

/// Information about a path in the interpreter's namespace.
pub struct PathInfo {
    /// The actual path of the file after canonicalization and, if requested,
    /// following all symlinks.
    pub hard_path: String,

    /// Information about the file itself.
    pub attributes: fio::NodeAttributes2,
}

/// Suggests what to do when opening a path and encountering a symlink.
pub enum SymlinkPolicy {
    /// Follow symlinks.
    Follow,
    /// Do not follow symlinks.
    DontFollow,
}

impl SymlinkPolicy {
    /// Whether this policy says to follow symlinks.
    pub fn follow(&self) -> bool {
        matches!(self, SymlinkPolicy::Follow)
    }
}

/// Maximum number of symlinks we can follow on path lookup before exploding.
pub(crate) const SYMLINK_RECURSION_LIMIT: usize = 40;

/// Interior state of a channel server task. See [InterpreterInner::wait_tx_id].
#[derive(Default)]
struct ChannelServerState {
    /// Hashmap of FIDL transaction IDs => Senders to which replies with those
    /// transaction IDs should be forwarded.
    senders: HashMap<u32, OneshotSender<Result<fdomain_client::MessageBuf>>>,
    /// If we get a message out of a channel and don't recognize the transaction
    /// ID, the transaction ID and message are recorded here. Most likely it's
    /// just a timing issue, and the task which is expecting that message will
    /// be along shortly to pick it up.
    orphan_messages: HashMap<u32, Result<fdomain_client::MessageBuf>>,
    /// Whether we are currently running a task which reads the channel and
    /// responds to server messages.
    server_running: bool,
}

pub(crate) struct InterpreterInner {
    /// FIDL Codec namespace. Contains the imported FIDL type info from FIDL
    /// JSON for all the FIDL this interpreter knows about.
    lib_namespace: lib::Namespace,
    /// Sending a future on this sender causes it to be polled as part of the
    /// interpreter's run future, making this a sort of mini executor.
    task_sender: UnboundedSender<BoxFuture<'static, ()>>,
    /// State for tasks used to read from channels on behalf of other tasks,
    /// solving a synchronization issue that could come if any task could read
    /// from any channel by itself. See [InterpreterInner::wait_tx_id].
    channel_servers: Mutex<HashMap<u32, ChannelServerState>>,
    /// Pool of FIDL transaction IDs for whoever needs one of those.
    tx_id_pool: AtomicU32,
}

impl InterpreterInner {
    /// Fetch the library namespace.
    pub fn lib_namespace(&self) -> &lib::Namespace {
        &self.lib_namespace
    }

    /// Allocate a new transaction ID for use with FIDL messages. This is just a
    /// guaranteed unique `u32`. It is imbued with no special properties.
    pub fn alloc_tx_id(&self) -> u32 {
        self.tx_id_pool.fetch_add(1, Ordering::Relaxed)
    }

    /// Add a new task to this interpreter.
    pub fn push_task(&self, task: impl Future<Output = ()> + Send + 'static) {
        let _ = self.task_sender.unbounded_send(task.boxed());
    }

    /// Waits for the given handle (presumed to be a channel) to return a
    /// FIDL message with the given transaction ID, then sends the message
    /// through the given sender.
    ///
    /// Our model intrinsically allows multiple ownership of handles. If
    /// multiple tasks send requests on a channel, that's fine, they'll arrive
    /// in whatever order and the other end will sort them out. But if the other
    /// end sends a reply, the individual tasks might end up getting eachothers'
    /// replies if they all just naively read from the channel.
    ///
    /// So instead we have this method, which allows us to ask the interpreter
    /// to start pulling messages out from the channel, forward us the one that
    /// has the transaction ID we expect, and keep any others it may find for
    /// other tasks that may call. The interpreter has at most one of these
    /// "channel server" tasks for each handle being read in this way at any
    /// given time. If two tasks try to read from the same channel, one task
    /// will handle both.
    pub fn wait_tx_id(
        self: Arc<Self>,
        handle: InUseHandle,
        tx_id: u32,
        responder: OneshotSender<Result<fdomain_client::MessageBuf>>,
    ) {
        let handle_id = match handle.id() {
            Ok(x) => x,
            Err(e) => {
                let _ = responder.send(Err(e));
                return;
            }
        };

        let start_server = {
            let mut servers = self.channel_servers.lock().unwrap();

            let server = servers.entry(handle_id).or_default();

            if let Some(msg) = server.orphan_messages.remove(&tx_id) {
                let _ = responder.send(msg);
                return;
            }

            server.senders.insert(tx_id, responder);

            !std::mem::replace(&mut server.server_running, true)
        };

        if start_server {
            let weak_self = Arc::downgrade(&self);
            let _ = self.push_task(async move {
                enum FailureMode {
                    TransactionHeader(fidl::Error),
                    Handle(crate::error::Error),
                }

                let error = loop {
                    match handle.read_channel_etc().await {
                        Ok(mut buf) => {
                            let tx_id = match fidl::encoding::decode_transaction_header(buf.bytes())
                            {
                                Ok((fidl::encoding::TransactionHeader { tx_id, .. }, _)) => tx_id,
                                Err(e) => break FailureMode::TransactionHeader(e),
                            };

                            let buf =
                                std::mem::replace(&mut buf, fdomain_client::MessageBuf::new());

                            let Some(this) = weak_self.upgrade() else {
                                return;
                            };

                            let mut servers = this.channel_servers.lock().unwrap();
                            let Some(server) = servers.get_mut(&handle_id) else {
                                return;
                            };

                            if let Some(sender) = server.senders.remove(&tx_id) {
                                let _ = sender.send(Ok(buf));
                            } else {
                                let _ = server.orphan_messages.insert(tx_id, Ok(buf));
                            }

                            if server.senders.is_empty() {
                                server.server_running = false;
                                return;
                            }
                        }
                        Err(status) => break FailureMode::Handle(status),
                    }
                };

                let Some(this) = weak_self.upgrade() else {
                    return;
                };
                let mut servers = this.channel_servers.lock().unwrap();
                let Some(server) = servers.get_mut(&handle_id) else {
                    return;
                };

                for (_, sender) in server.senders.drain() {
                    let _ = sender.send(Err(match &error {
                        FailureMode::TransactionHeader(e) => {
                            MessageError::BadFidlTransactionHeader(e.clone()).into()
                        }
                        FailureMode::Handle(e) => e.clone(),
                    }));
                }

                server.server_running = false;
            });
        }
    }

    /// Assuming the given value contains an invocable, run it with the given
    /// arguments. You can set the value of `$_` with `underscore`.
    pub async fn invoke_value(
        self: Arc<Self>,
        invocable: Value,
        mut args: Vec<Value>,
        underscore: Option<Value>,
    ) -> Result<Value> {
        let in_use_handle = match invocable {
            Value::OutOfLine(PlaygroundValue::Invocable(invocable)) => {
                return invocable.invoke(args, underscore).await;
            }
            x => {
                let Some(in_use_handle) = x.to_in_use_handle() else {
                    return Err(Exception::NotInvocable.into());
                };
                in_use_handle
            }
        };

        let protocol_name = in_use_handle.get_client_protocol()?;

        let protocol = self
            .lib_namespace()
            .lookup(&protocol_name)
            .map_err(|_| MessageError::ProtocolNotFound(protocol_name.clone()))?;
        let lib::LookupResult::Protocol(protocol) = protocol else {
            return Err(Exception::NotInvocable.into());
        };

        if args.len() > 1 {
            return Err(MessageError::MessageArgumentsIncorrect.into());
        }

        let arg = if let Some(arg) = args.pop() {
            arg
        } else if let Some(underscore) = underscore {
            underscore
        } else {
            return Err(MessageError::NoMessage.into());
        };

        let Value::OutOfLine(PlaygroundValue::TypeHinted(method_name, arg)) = arg else {
            return Err(MessageError::MessageArgumentsIncorrect.into());
        };

        let Some(method) = protocol.methods.get(&method_name) else {
            return Err(MessageError::NoMethodInProtocol(method_name, protocol_name).into());
        };

        let request = if let Some(request_type) = &method.request {
            arg.to_fidl_value(self.lib_namespace(), request_type)?
        } else {
            FidlValue::Null
        };

        let tx_id =
            if method.has_response && method.response.is_some() { self.alloc_tx_id() } else { 0 };

        let (bytes, handles) = fidl_codec_fdomain::encode_request(
            self.lib_namespace(),
            tx_id,
            &protocol_name,
            &method.name,
            request,
        )
        .map_err(|e| {
            MessageError::EncodeRequestFailed(
                protocol_name.clone(),
                method_name.clone(),
                Arc::new(e),
            )
        })?;
        in_use_handle
            .write_channel_etc(&bytes, handles.into_iter().map(Into::into).collect())
            .await?;

        if tx_id != 0 {
            let (sender, receiver) = oneshot_channel();

            Arc::clone(&self).wait_tx_id(in_use_handle, tx_id, sender);

            let (bytes, handles) =
                receiver.await.map_err(|_| RuntimeError::InterpreterDied)??.split();

            let value = fidl_codec_fdomain::decode_response(
                self.lib_namespace(),
                &bytes,
                handles.into_iter().map(Into::into).collect(),
            )
            .map_err(|e| {
                MessageError::DecodeReplyFailed(
                    protocol_name.clone(),
                    method_name.clone(),
                    Arc::new(e),
                )
            })?;

            Ok(value.1.upcast())
        } else {
            Ok(Value::Null)
        }
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
    /// Helper for [`InterpreterInner::open`] that sends a request given a value
    /// of $fs_root and a path.
    async fn send_open_request(
        &self,
        fs_root: &InUseHandle,
        path: &String,
        flags: fio::Flags,
        attributes_requested: Option<fio::NodeAttributesQuery>,
    ) -> Result<fio::NodeProxy> {
        let (node, server) = fs_root.fdomain()?.create_proxy::<fio::NodeMarker>();

        let options = if let Some(attributes) = attributes_requested {
            vec![("attributes".to_owned(), FidlValue::U64(attributes.bits()))]
        } else {
            Vec::new()
        };

        let request = FidlValue::Object(vec![
            ("path".to_owned(), FidlValue::String(path.clone())),
            ("flags".to_owned(), FidlValue::U64(flags.bits())),
            ("options".to_owned(), FidlValue::Object(options)),
            (
                "object".to_owned(),
                FidlValue::Handle(server.into_handle(), ObjectType::Channel, None),
            ),
        ]);

        let library_name = fio::DirectoryMarker::library_name();
        let (bytes, handles) = fidl_codec_fdomain::encode_request(
            self.lib_namespace(),
            0,
            &library_name,
            "Open",
            request,
        )
        .map_err(|e| {
            MessageError::EncodeRequestFailed(library_name, "Open".to_owned(), Arc::new(e))
        })?;
        let handles: Vec<_> = handles.into_iter().map(Into::into).collect();
        fs_root.write_channel_etc(&bytes, handles).await?;
        Ok(node)
    }

    /// Get information about a file in this interpreter's namespace.
    pub async fn path_info(
        &self,
        path: String,
        fs_root: Value,
        pwd: Value,
        query: fio::NodeAttributesQuery,
        symlink_policy: SymlinkPolicy,
    ) -> Result<PathInfo> {
        let Some(fs_root) = fs_root.to_in_use_handle() else {
            return Err(FSError::FSRootNotHandle.into());
        };

        self.path_info_impl(path, &fs_root, pwd, query, symlink_policy).await
    }

    /// Like `path_info` but takes a `&InUseHandle` for `fs_root`.
    async fn path_info_impl(
        &self,
        path: String,
        fs_root: &InUseHandle,
        pwd: Value,
        query: fio::NodeAttributesQuery,
        symlink_policy: SymlinkPolicy,
    ) -> Result<PathInfo> {
        let query = if symlink_policy.follow() {
            query | fio::NodeAttributesQuery::PROTOCOLS
        } else {
            query
        };

        let mut path = canonicalize_path(path, pwd)?;
        let orig_path = path.clone();

        if !fs_root
            .get_client_protocol()
            .ok()
            .map(|x| self.lib_namespace().inherits(&x, &fio::DirectoryMarker::library_name()))
            .unwrap_or(false)
        {
            return Err(FSError::FSRootNotDirectory.into());
        }

        for _ in 0..SYMLINK_RECURSION_LIMIT {
            let node = self
                .send_open_request(
                    &fs_root,
                    &path,
                    fio::Flags::PROTOCOL_NODE | fio::Flags::FLAG_SEND_REPRESENTATION,
                    Some(query),
                )
                .await?;
            let event = node
                .take_event_stream()
                .next()
                .await
                .ok_or_else(|| FSError::NoRepresentation(path.to_owned()))?
                .map_err(|e| FSError::RepresentationFailed(path.to_owned(), e))?;

            let attributes = match event {
                fio::NodeEvent::OnOpen_ { .. } => {
                    return Err(FSError::UnexpectedOnOpenEvent(path.to_owned()).into());
                }
                fio::NodeEvent::OnRepresentation { payload } => match payload {
                    #[cfg(fuchsia_api_level_at_least = "27")]
                    fdomain_fuchsia_io::Representation::Node(info) => info.attributes,
                    #[cfg(not(fuchsia_api_level_at_least = "27"))]
                    fdomain_fuchsia_io::Representation::Connector(info) => info.attributes,
                    fdomain_fuchsia_io::Representation::Directory(info) => info.attributes,
                    fdomain_fuchsia_io::Representation::File(info) => info.attributes,
                    fdomain_fuchsia_io::Representation::Symlink(info) => info.attributes,
                    _ => None,
                },
                fio::NodeEvent::_UnknownEvent { ordinal, .. } => {
                    return Err(FSError::UnexpectedEventInsteadOfRepresentation(
                        path.to_owned(),
                        ordinal,
                    )
                    .into());
                }
            };

            let attributes = attributes.unwrap_or_else(|| fio::NodeAttributes2 {
                mutable_attributes: Default::default(),
                immutable_attributes: Default::default(),
            });

            let is_symlink = attributes
                .immutable_attributes
                .protocols
                .unwrap_or_default()
                .contains(fio::NodeProtocolKinds::SYMLINK);

            if symlink_policy.follow() && is_symlink {
                let symlink = self
                    .send_open_request(
                        &fs_root,
                        &path,
                        fio::Flags::PROTOCOL_SYMLINK | fio::PERM_READABLE,
                        None,
                    )
                    .await?;
                let symlink =
                    fio::SymlinkProxy::from_channel(symlink.into_channel().expect(
                        "Proxy couldn't be converted to channel immediately after creation!",
                    ));
                let symlink_info = symlink
                    .describe()
                    .await
                    .map_err(|e| FSError::SymlinkDescribeFailed(path.to_owned(), e))?;
                let Some(target) = symlink_info.target else {
                    return Err(FSError::NoSymlinkTarget(path).into());
                };
                let target =
                    String::from_utf8(target).map_err(|_| FSError::SymlinkNotUTF(path.clone()))?;
                let end = path.rfind('/').expect("Canonicalized path wasn't absolute!");
                let prefix = &path[..end];
                let prefix = if prefix.is_empty() { "/" } else { prefix };
                // Set the path to the new target, reset the flags, and
                // basically start the whole circus over.
                path = canonicalize_path_dont_check_pwd(target, prefix);
                continue;
            }

            return Ok(PathInfo { hard_path: path, attributes });
        }

        Err(FSError::SymlinkRecursionExceeded(orig_path).into())
    }

    /// Open a path which is known to be a directory.
    ///
    /// You should already have verified that the path is a directory with
    /// `path_info`. Opening things that aren't directories with this is an
    /// untested code path.
    ///
    /// The path must also be canonicalized already.
    pub(crate) async fn open_directory(
        &self,
        path: String,
        fs_root: Value,
    ) -> Result<fio::DirectoryProxy> {
        debug_assert!(path.starts_with("/"));

        let Some(fs_root) = fs_root.to_in_use_handle() else {
            return Err(FSError::FSRootNotHandle.into());
        };

        let node = self
            .send_open_request(
                &fs_root,
                &path,
                fio::Flags::PROTOCOL_DIRECTORY
                    | fio::PERM_READABLE
                    | fio::Flags::PERM_INHERIT_WRITE,
                None,
            )
            .await?;
        Ok(fio::DirectoryProxy::from_channel(
            node.into_channel().expect("Could not tear down proxy"),
        ))
    }

    /// Open a file in this interpreter's namespace.
    pub async fn open(&self, path: String, fs_root: Value, pwd: Value) -> Result<Value> {
        let Some(fs_root) = fs_root.to_in_use_handle() else {
            return Err(FSError::FSRootNotHandle.into());
        };

        let info = self
            .path_info_impl(
                path,
                &fs_root,
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

        let path = info.hard_path;

        let (proto, node) = if protocols.contains(fio::NodeProtocolKinds::DIRECTORY) {
            (
                fio::DirectoryMarker::library_name(),
                self.send_open_request(
                    &fs_root,
                    &path,
                    fio::Flags::PROTOCOL_DIRECTORY
                        | fio::PERM_READABLE
                        | fio::Flags::PERM_INHERIT_WRITE,
                    None,
                )
                .await?,
            )
        } else if protocols.contains(fio::NodeProtocolKinds::FILE) {
            (
                fio::FileMarker::library_name(),
                self.send_open_request(
                    &fs_root,
                    &path,
                    fio::Flags::PROTOCOL_FILE | fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE,
                    None,
                )
                .await?,
            )
        } else if protocols.contains(fio::NodeProtocolKinds::SYMLINK) {
            unreachable!("path_info was supposed to traverse symlinks but didn't!");
        } else if protocols.contains(fio::NodeProtocolKinds::CONNECTOR) {
            let last_slash = path.rfind('/').expect("Canonicalized path wasn't absolute!");
            let name = &path[last_slash + 1..];
            if name.starts_with("fuchsia.") {
                (
                    protocol_to_library_name(name),
                    self.send_open_request(&fs_root, &path, fio::Flags::PROTOCOL_SERVICE, None)
                        .await?,
                )
            } else {
                (
                    fio::NodeMarker::library_name(),
                    self.send_open_request(&fs_root, &path, fio::Flags::PROTOCOL_NODE, None)
                        .await?,
                )
            }
        } else {
            (
                fio::NodeMarker::library_name(),
                self.send_open_request(&fs_root, &path, fio::Flags::PROTOCOL_NODE, None).await?,
            )
        };

        Ok(Value::ClientEnd(node.into_channel().expect("Could not tear down proxy"), proto, None))
    }
}

pub struct Interpreter {
    pub(crate) inner: Arc<InterpreterInner>,
    global_variables: Mutex<GlobalVariables>,
}

impl Interpreter {
    /// Create a new interpreter.
    ///
    /// The interpreter itself may spawn multiple tasks, and polling the future
    /// returned alongside the interpreter is necessary to keep those tasks
    /// running, and thus the interpreter functioning correctly.
    pub async fn new(
        lib_namespace: lib::Namespace,
        fs_root: fdomain_client::fidl::ClientEnd<fdomain_fuchsia_io::DirectoryMarker>,
    ) -> (Self, impl Future<Output = ()>) {
        let (task_sender, task_receiver) = unbounded_channel();

        let fs_root =
            Value::ClientEnd(fs_root.into_channel(), fio::DirectoryMarker::library_name(), None);
        let mut global_variables = GlobalVariables::default();
        global_variables.define("fs_root".to_owned(), Ok(fs_root), Mutability::Mutable);
        global_variables.define(
            "pwd".to_owned(),
            Ok(Value::String("/".to_owned())),
            Mutability::Mutable,
        );
        let interpreter = Interpreter {
            inner: Arc::new(InterpreterInner {
                lib_namespace,
                task_sender,
                channel_servers: Mutex::new(HashMap::new()),
                tx_id_pool: AtomicU32::new(1),
            }),
            global_variables: Mutex::new(global_variables),
        };
        let mut executor = task_receiver.for_each_concurrent(None, |x| x);
        interpreter.add_builtins(&mut executor).await;

        (interpreter, executor)
    }

    /// Create a new interpreter with the given inner state.
    ///
    /// This is used to fork the interpreter for hygienic imports.
    pub(crate) async fn new_with_inner(
        inner: Arc<InterpreterInner>,
        fs_root: Value,
        pwd: Value,
    ) -> Self {
        let mut global_variables = GlobalVariables::default();
        global_variables.define("fs_root".to_owned(), Ok(fs_root), Mutability::Mutable);
        global_variables.define("pwd".to_owned(), Ok(pwd), Mutability::Mutable);
        let interpreter = Interpreter { inner, global_variables: Mutex::new(global_variables) };
        interpreter.add_builtins(&mut futures::future::pending()).await;

        interpreter
    }

    /// Looks up the given `path` in this namespace, then tries to load it as a
    /// text file, and, if successful, runs the contents as code in a new
    /// interpreter.
    pub(crate) fn run_isolated_import(
        self,
        path: String,
    ) -> impl Future<Output = Result<GlobalVariables>> + Send + 'static {
        async move {
            let file = self.open(path.clone()).await?;
            let file = file
                .try_client_channel(self.inner.lib_namespace(), &fio::FileMarker::library_name())
                .map_err(|_| FSError::ImportNotFile(path.clone()))?;
            let file = fdomain_fuchsia_io::FileProxy::from_channel(file);
            let file = fuchsia_fs_fdomain::file::read_to_string(&file)
                .await
                .map_err(|e| FSError::FileReadError(Arc::new(e)))?;
            self.run(file.as_str()).await?;
            Ok(self.global_variables.into_inner().unwrap())
        }
    }

    /// Take a [`ReplayableIterator`], which is how the playground [`Value`]
    /// type represents iterators, and convert it to a [`futures::Stream`],
    /// which is easier to work with directly in Rust.
    pub fn replayable_iterator_to_stream(
        &self,
        mut iter: ReplayableIterator,
    ) -> impl Stream<Item = Result<Value>> {
        let (sender, receiver) = unbounded_channel();

        self.inner.push_task(async move {
            loop {
                match iter.next().await {
                    Ok(Some(v)) => {
                        if sender.unbounded_send(Ok(v)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = sender.unbounded_send(Err(e));
                        break;
                    }
                }
            }
        });

        receiver
    }

    /// Compile the given code as an anonymous callable value in this
    /// interpreter's scope, then return it as a Rust closure so you can call
    /// the code repeatedly.
    pub async fn get_runnable(
        &self,
        code: &str,
    ) -> Result<impl (Fn() -> BoxFuture<'static, Result<Value>>) + Clone + use<>> {
        let Value::OutOfLine(PlaygroundValue::Invocable(program)) =
            self.run(format!("\\() {{ {code} }}").as_str()).await?
        else {
            unreachable!("Preamble didn't compile to invocable");
        };

        Ok(move || program.clone().invoke(vec![], None).boxed())
    }

    /// Add a new command to the global environment.
    ///
    /// Creates a global variable with the given `name` in this interpreter's
    /// global scope, and assigns it an invocable value which calls the closure
    /// given in `cmd`. The closure takes a list of arguments as [`Value`]s and
    /// an optional `Value` which is the current value of `$_`
    pub fn add_command<
        F: Fn(Vec<Value>, Option<Value>) -> R + Send + Sync + 'static,
        R: Future<Output = anyhow::Result<Value>> + Send + 'static,
    >(
        &self,
        name: &str,
        cmd: F,
    ) {
        let cmd = Arc::new(cmd);
        let cmd = move |args: Vec<Value>, underscore: Option<Value>| {
            let cmd = Arc::clone(&cmd);

            async move {
                cmd(args.into(), underscore).await.map_err(crate::error::Error::user_from_anyhow)
            }
            .boxed()
        };

        self.global_variables.lock().unwrap().define(
            name.to_owned(),
            Ok(Value::OutOfLine(PlaygroundValue::Invocable(Invocable::new(Arc::new(cmd))))),
            Mutability::Constant,
        );
    }

    /// Parse and run the given program in the context of this interpreter.
    pub async fn run<'a, T: std::convert::Into<ParseResult<'a>>>(
        &self,
        program: T,
    ) -> Result<Value> {
        let program: ParseResult<'a> = program.into();

        if !program.errors.is_empty() {
            let mut s = String::new();
            for error in program.errors.into_iter() {
                writeln!(
                    s,
                    "line {} column {}: {}",
                    error.0.location_line(),
                    error.0.get_utf8_column(),
                    error.1
                )
                .expect("Write to a string writer failed?!");
            }

            Err(RuntimeError::ParseError(s).into())
        } else {
            enum Section<'a> {
                Statements(Vec<Node<'a>>),
                Imports(Vec<String>),
            }

            let Node::Program(statements) = program.tree else {
                unreachable!("Parse tree was not rooted in a program node!");
            };

            let mut sections = Vec::new();

            for node in statements.into_iter() {
                if let Node::Import(path, None) = node {
                    let path = (*path.fragment()).to_owned();
                    if let Some(Section::Imports(paths)) = sections.last_mut() {
                        paths.push(path);
                    } else {
                        sections.push(Section::Imports(vec![path]));
                    }
                } else if let Some(Section::Statements(statements)) = sections.last_mut() {
                    statements.push(node);
                } else {
                    sections.push(Section::Statements(vec![node]));
                }
            }

            let mut ret = Value::Null;

            for section in sections {
                match section {
                    Section::Statements(s) => {
                        let mut visitor = Visitor::new(None, None);
                        let compiled = visitor.visit(Node::Program(s));
                        let (mut frame, invalid_ids) = {
                            let mut global_variables = self.global_variables.lock().unwrap();
                            // TODO: There's a complicated bug here where only the last
                            // declaration of a variable determines the mutability for the
                            // whole run of this compilation unit. We could fix it by not
                            // reusing slots for multiple declarations.
                            for (name, mutability) in visitor.get_top_level_variable_decls() {
                                global_variables.ensure_defined(
                                    name,
                                    || Err(Exception::VariableUndeclared(name.clone()).into_err()),
                                    mutability,
                                )
                            }
                            let slots_needed = visitor.slots_needed();
                            let (mut captured_ids, allocated_ids) = visitor.into_slot_data();
                            (
                                global_variables.as_frame(slots_needed, |ident| {
                                    if let Some(id) = captured_ids.remove(ident) {
                                        Some(id)
                                    } else {
                                        allocated_ids.get(ident).copied()
                                    }
                                }),
                                captured_ids,
                            )
                        };

                        for (name, slot) in invalid_ids {
                            frame.assign(slot, Err(Exception::VariableUndeclared(name).into()));
                        }

                        let frame = Mutex::new(frame);
                        ret = compiled(&self.inner, &frame).await?;
                    }
                    Section::Imports(imports) => {
                        fn boxed_run_import(
                            inner: Arc<InterpreterInner>,
                            fs_root: Value,
                            pwd: Value,
                            import: String,
                        ) -> futures::future::BoxFuture<'static, Result<GlobalVariables>>
                        {
                            async move {
                                let interpreter =
                                    Interpreter::new_with_inner(inner, fs_root, pwd).await;
                                interpreter.run_isolated_import(import).await
                            }
                            .boxed()
                        }
                        for import in imports {
                            let (fs_root, pwd) = {
                                let g = self.global_variables.lock().unwrap();
                                (g.get("fs_root"), g.get("pwd"))
                            };

                            let fs_root = fs_root.await.unwrap_or_else(|| {
                                Err(Exception::VariableUndeclared("fs_root".to_owned()).into())
                            })?;
                            let pwd = pwd.await.unwrap_or_else(|| {
                                Err(Exception::VariableUndeclared("pwd".to_owned()).into())
                            })?;
                            let inner = Arc::clone(&self.inner);

                            let got = boxed_run_import(inner, fs_root, pwd, import).await?;
                            self.global_variables.lock().unwrap().merge(got);
                        }
                        ret = Value::Null;
                    }
                }
            }

            Ok(ret)
        }
    }

    /// Performs tab completion. Takes in a command string and the cursor
    /// position and returns a list of tuples of strings to be inserted and
    /// where in the string they begin. The characters between the cursor
    /// position and the beginning position given with the completion should be
    /// deleted before the completion text is inserted in their place, and the
    /// cursor should end up at the end of the completion text.
    pub async fn complete(&self, cmd: String, cursor_pos: usize) -> Vec<(String, usize)> {
        let cmd: ParseResult<'_> = cmd.as_str().into();
        let whitespace = cmd.whitespace;
        let hints = cmd.tab_completions;
        let mut ret = Vec::new();

        let mut whitespace_range_start = cursor_pos;
        let mut whitespace_range_end = cursor_pos;

        for whitespace in whitespace.iter() {
            let whitespace_start = whitespace.location_offset();
            let whitespace_end = whitespace_start + whitespace.fragment().len();
            if whitespace_start <= cursor_pos && whitespace_end >= cursor_pos {
                whitespace_range_start = std::cmp::min(whitespace_range_start, whitespace_start);
                whitespace_range_end = std::cmp::max(whitespace_range_end, whitespace_end);
            }
        }

        for hint in hints.values().flat_map(|x| x.iter()) {
            let start = hint.span().location_offset();
            let end = start + hint.span().fragment().len();
            if start > whitespace_range_end {
                continue;
            }

            if end < whitespace_range_start {
                continue;
            }

            let fragment = if hint.span().fragment().is_empty() {
                ""
            } else {
                if start > cursor_pos || end < cursor_pos {
                    continue;
                }

                &hint.span().fragment()[..cursor_pos - start]
            };

            match hint {
                TabHint::Invocable(_) => {
                    for name in self.global_variables.lock().unwrap().names(|x| x.is_invocable()) {
                        if name.starts_with(fragment) {
                            ret.push((format!("{name} "), start))
                        }
                    }
                }
                TabHint::CommandArgument(_) => {
                    if fragment.is_empty() {
                        continue;
                    }

                    let file = self.open(fragment.to_owned()).await;

                    let (file, prefix, filter) = if let Ok(file) = file {
                        (file, fragment, "")
                    } else if fragment.ends_with("/") {
                        continue;
                    } else {
                        let (prefix, filter) = fragment.rsplit_once("/").unwrap_or((".", fragment));
                        let Ok(file) = self.open(prefix.to_owned()).await else {
                            continue;
                        };
                        (file, prefix, filter)
                    };

                    let dir = file.try_client_channel(
                        self.inner.lib_namespace(),
                        &fio::DirectoryMarker::library_name(),
                    );

                    if let Ok(dir) = dir {
                        let dir = fio::DirectoryProxy::from_channel(dir);
                        let Ok(entries) = fuchsia_fs_fdomain::directory::readdir(&dir).await else {
                            continue;
                        };

                        for entry in entries.into_iter().filter(|x| x.name.starts_with(filter)) {
                            let is_dir = if let Ok(node) = fuchsia_fs_fdomain::directory::open_node(
                                &dir,
                                &entry.name,
                                fio::Flags::PERM_GET_ATTRIBUTES,
                            )
                            .await
                            {
                                if let Ok(Ok((
                                    _,
                                    fio::ImmutableNodeAttributes {
                                        protocols: Some(protocols), ..
                                    },
                                ))) =
                                    node.get_attributes(fio::NodeAttributesQuery::PROTOCOLS).await
                                {
                                    protocols.contains(fio::NodeProtocolKinds::DIRECTORY)
                                } else {
                                    false
                                }
                            } else {
                                false
                            };

                            let postfix = if is_dir { "/" } else { " " };
                            let sep = if prefix.ends_with("/") { "" } else { "/" };
                            let name = entry.name;
                            ret.push((format!("{prefix}{sep}{name}{postfix}"), start));
                        }
                    } else if prefix == fragment && !fragment.ends_with("/") {
                        ret.push((format!("{fragment} "), start));
                    }
                }
            }
        }

        ret
    }

    /// Open a file in this interpreter's namespace.
    pub async fn open(&self, path: String) -> Result<Value> {
        let (fs_root, pwd) = {
            let globals = self.global_variables.lock().unwrap();
            (globals.get("fs_root"), globals.get("pwd"))
        };
        let fs_root =
            fs_root.await.ok_or_else(|| Exception::VariableUndeclared("fs_root".to_owned()))??;
        let pwd = pwd.await.ok_or_else(|| Exception::VariableUndeclared("pwd".to_owned()))??;

        self.inner.open(path, fs_root, pwd).await
    }
}

#[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255249)
/// Turn a path into a dotless, absolute form.
pub fn canonicalize_path(path: String, pwd: Value) -> Result<String> {
    let Value::String(pwd) = pwd else {
        return Err(FSError::PwdNotString.into());
    };

    if !pwd.starts_with("/") {
        return Err(FSError::PwdNotAbsolute.into());
    }

    Ok(canonicalize_path_dont_check_pwd(path, &pwd))
}

/// Turn a path into a dotless, absolute form. Don't validate the pwd string first.
pub fn canonicalize_path_dont_check_pwd(path: String, pwd: &str) -> String {
    static MULTIPLE_SLASHES: OnceLock<Regex> = OnceLock::new();
    static SINGLE_DOT: OnceLock<Regex> = OnceLock::new();
    static DOUBLE_DOT_ELIMINATION: OnceLock<Regex> = OnceLock::new();

    let multiple_slashes = MULTIPLE_SLASHES.get_or_init(|| Regex::new(r"/{2,}").unwrap());
    let single_dot = SINGLE_DOT.get_or_init(|| Regex::new(r"(?:/\.)+(/|$)").unwrap());
    let double_dot_elimination =
        DOUBLE_DOT_ELIMINATION.get_or_init(|| Regex::new(r"(^|[^/]+)/\.\.(/|$)").unwrap());

    let sep = if pwd.ends_with("/") { "" } else { "/" };
    let path = if path.starts_with("/") { path } else { format!("{pwd}{sep}{path}") };
    let path = multiple_slashes.replace_all(&path, "/");
    let mut path = single_dot.replace_all(&path, "${1}").to_string();
    let mut path_len = path.len();
    loop {
        let new_path = double_dot_elimination.replace(&path, "");
        if new_path.len() == path_len {
            break;
        }
        path = new_path.to_string();
        path_len = path.len();
    }

    if path.ends_with("/") {
        path.pop();
    }

    if path.is_empty() {
        path.push('/');
    }

    path.to_string()
}

/// Convert discoverable protocol name (e.g. fuchsia.fshost.Admin) to the library name used by
/// fidlcodec (e.g. fuchsia.fshost/Admin).
pub fn protocol_to_library_name(protocol: &str) -> String {
    let mut library_name = protocol.to_owned();
    let final_dot = library_name.rfind('.').unwrap();
    library_name.replace_range(final_dot..final_dot + 1, "/");
    library_name
}

/// Extension trait that adds equivalent library name to a discoverable protocol marker.
pub trait LibraryProtocolNameExt {
    /// The equivalent library name (e.g. fuchsia.fshost/Admin) for use with fidlcodec.
    fn library_name() -> String;
}

impl<P: DiscoverableProtocolMarker> LibraryProtocolNameExt for P {
    fn library_name() -> String {
        protocol_to_library_name(P::PROTOCOL_NAME)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn canonicalize() {
        assert_eq!(
            "/foo",
            &canonicalize_path(
                "././/baz/bang/../../..".to_owned(),
                Value::String("/foo/bar".to_owned())
            )
            .unwrap()
        );
    }
}
