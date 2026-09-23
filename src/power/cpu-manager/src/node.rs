// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::CpuManagerError;
use crate::message::{Message, MessageReturn};
use anyhow::Error;
use async_trait::async_trait;
use std::rc::Rc;

/// A trait that all nodes in the CpuManager must implement
#[async_trait(?Send)]
pub trait Node {
    /// Return a string to indicate the name of this node
    ///
    /// Each node should use this function to indicate a meaningful name. The name may be used for
    /// logging and/or debugging purposes.
    fn name(&self) -> String;

    /// Initialize any internal state or data that requires drivers or other async behavior.
    ///
    /// This function is called on every node after all nodes have been initially created. All
    /// nodes' `init()` functions are polled together asynchronously. Returning an error here will
    /// cause the Cpu Manager to fail to start.
    async fn init(&self) -> Result<(), Error> {
        Ok(())
    }

    /// Handle a new message
    ///
    /// All nodes must implement this message to support communication between nodes. This is the
    /// entry point for a Node to receive new messages.
    async fn handle_message(&self, _msg: &Message) -> Result<MessageReturn, CpuManagerError> {
        Err(CpuManagerError::Unsupported)
    }

    /// Send a message to another node
    ///
    /// This is implemented as a future to support scenarios where a node wishes to send messages to
    /// multiple other nodes. Errors are logged automatically.
    async fn send_message(
        &self,
        node: &Rc<dyn Node>,
        msg: &Message,
    ) -> Result<MessageReturn, CpuManagerError> {
        // TODO(fxbug.dev/42120903): Ideally we'd use a duration event here. But due to a limitation in
        // the Rust tracing library, that would require creating any formatted strings (such as the
        // "message" value) unconditionally, even when the tracing category is disabled. To avoid
        // that unnecessary computation, just use an instant event.
        fuchsia_trace::instant!(
            c"cpu_manager:messages",
            c"message_start",
            fuchsia_trace::Scope::Thread,
            "message" => format!("{:?}", msg).as_str(),
            "source_node" => self.name().as_str(),
            "dest_node" => node.name().as_str()
        );

        let result = node.handle_message(msg).await;
        fuchsia_trace::instant!(
            c"cpu_manager:messages",
            c"message_result",
            fuchsia_trace::Scope::Thread,
            "message" => format!("{:?}", msg).as_str(),
            "source_node" => self.name().as_str(),
            "dest_node" => node.name().as_str(),
            "result" => format!("{:?}", result).as_str()
        );
        result
    }
}
