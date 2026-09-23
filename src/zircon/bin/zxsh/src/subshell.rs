// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Subshell execution from a VMO.
//!
//! The VMO contains serialized shell state to be executed in a child process.
//!
//! Data Format of the VMO:
//! ```text
//! +---------------------------------------------+
//! | SubshellPayloadHeader (24 bytes, zerocopy)  |
//! |  +---------------------------------------+  |
//! |  | Total Payload Size (U64, 8 bytes)     |  |
//! |  +---------------------------------------+  |
//! |  | Command AST Size (U64, 8 bytes)       |  |
//! |  +---------------------------------------+  |
//! |  | Env Size (U64, 8 bytes)               |  |
//! |  +---------------------------------------+  |
//! +---------------------------------------------+
//! | Serialized AST (Command AST Size bytes)     |
//! +---------------------------------------------+
//! | Serialized Env (Env Size bytes)             |
//! +---------------------------------------------+
//! ```

use crate::errors::zx_status_str;
use crate::eval::{EvalOutcome, ExecutionContext, ShellState, eval_command, run_exit_trap};
use crate::parser::ast::{ASTBuilder, Command};
use crate::process::spawn_command;
use crate::relative;
use crate::serialization::{Deserialize, Serialize};
use crate::string::path_buf_to_bstring;
use bstr::ByteSlice;
use zerocopy::{FromZeros, IntoBytes};

/// Fixed-size layout header at the start of a serialized subshell VMO payload.
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    zerocopy::FromBytes,
    zerocopy::IntoBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
)]
#[repr(C)]
pub struct SubshellPayloadHeader {
    pub total_length: u64,
    pub command_length: u64,
    pub environment_length: u64,
}

/// Container for deserialized subshell execution state and AST.
pub struct SubshellData {
    pub builder: ASTBuilder,
    pub root_command_pointer: relative::Ptr<Command>,
    pub state: ShellState,
}

/// Deserializes a subshell payload from raw byte slices containing command AST and environment data.
pub fn deserialize_subshell_payload(
    command_bytes: &[u8],
    environment_bytes: &[u8],
) -> Result<SubshellData, String> {
    let mut environment_offset = 0;
    let mut state = ShellState::deserialize(environment_bytes, &mut environment_offset)?;
    // Reset signal traps inherited from parent shell per POSIX subshell execution requirements.
    state.traps.clear();

    if let Ok(path) = state.cwd().to_path() {
        let _ = std::env::set_current_dir(path);
    }

    let mut builder = ASTBuilder::new();
    let root_command_pointer = builder.import_serialized_ast(command_bytes);

    Ok(SubshellData { builder, root_command_pointer, state })
}

/// Executes previously deserialized subshell AST and state data.
fn execute_subshell_data(subshell_data: &mut SubshellData) -> Result<i32, String> {
    let mut execution_context = ExecutionContext::initial()?;
    let result = match eval_command(
        &mut subshell_data.builder,
        subshell_data.root_command_pointer,
        &mut subshell_data.state,
        &mut execution_context,
    )? {
        EvalOutcome::Code(code) => code,
        EvalOutcome::Exit(code) => code,
        EvalOutcome::Return(code) => code,
        EvalOutcome::Break(_) | EvalOutcome::Continue(_) => 0,
    };
    run_exit_trap(&mut subshell_data.state, &mut execution_context);
    Ok(result)
}

/// Deserializes and executes a subshell payload from command and environment byte slices.
fn run_vmo(command_bytes: &[u8], environment_bytes: &[u8]) -> Result<i32, String> {
    let mut subshell_data = deserialize_subshell_payload(command_bytes, environment_bytes)?;
    execute_subshell_data(&mut subshell_data)
}

/// Run a subshell from a VMO.
pub fn run_subshell(vmo: zx::Vmo) -> Result<i32, String> {
    let mut header = SubshellPayloadHeader::new_zeroed();
    vmo.read(header.as_mut_bytes(), 0)
        .map_err(|error| format!("vmo read header failed: {}", zx_status_str(error)))?;

    let total_length = header.total_length as usize;
    let command_length = header.command_length as usize;
    let environment_length = header.environment_length as usize;

    if command_length + environment_length > total_length {
        return Err("Subshell header lengths exceed total payload length".to_string());
    }

    let payload = vmo
        .read_to_vec(std::mem::size_of::<SubshellPayloadHeader>() as u64, header.total_length)
        .map_err(|error| format!("vmo read payload failed: {}", zx_status_str(error)))?;

    let command_bytes = &payload[..command_length];
    let environment_bytes = &payload[command_length..command_length + environment_length];

    run_vmo(command_bytes, environment_bytes)
}

/// Serializes a command AST and shell state into a flat byte vector with a zero-copy layout header.
pub fn serialize_subshell_payload(
    command: &Command,
    state: &ShellState,
    source_buffer: &relative::Buffer,
) -> Vec<u8> {
    let header_size = std::mem::size_of::<SubshellPayloadHeader>();
    let mut out = vec![0u8; header_size];

    let command_start_index = out.len();
    command.serialize_into(&mut out, source_buffer);
    let command_length = (out.len() - command_start_index) as u64;

    let environment_start_index = out.len();
    state.serialize_into(&mut out);
    let environment_length = (out.len() - environment_start_index) as u64;

    let total_length = (out.len() - header_size) as u64;

    let header = SubshellPayloadHeader { total_length, command_length, environment_length };
    out[0..header_size].copy_from_slice(header.as_bytes());

    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubshellScriptArgs {
    Pass,
    DoNotPass,
}

/// Serializes a command and shell state into a VMO and forks a new child subshell process
/// (`--subshell-vmo`).
pub fn spawn_subshell_process(
    command: &Command,
    state: &ShellState,
    actions: &mut Vec<fdio::SpawnAction<'static>>,
    script_args: SubshellScriptArgs,
    source_buffer: &relative::Buffer,
) -> Result<zx::Process, String> {
    let bytes = serialize_subshell_payload(command, state, source_buffer);
    let vmo = zx::Vmo::create(bytes.len() as u64)
        .map_err(|error| format!("Vmo::create failed: {}", zx_status_str(error)))?;
    vmo.write(&bytes, 0).map_err(|error| format!("Vmo::write failed: {}", zx_status_str(error)))?;

    let self_path = std::env::current_exe()
        .map_err(|error| format!("std::env::current_exe failed: {error}"))?;
    let self_path_bstring = path_buf_to_bstring(self_path)
        .ok_or_else(|| "Failed to convert current_exe path to BString".to_string())?;
    let mut argv = vec![self_path_bstring];
    if script_args == SubshellScriptArgs::Pass {
        argv.push(state.script_name.clone());
        argv.extend(state.args.clone());
    }

    actions.push(fdio::SpawnAction::add_handle(
        fuchsia_runtime::HandleInfo::new(fuchsia_runtime::HandleType::User0, 0),
        vmo.into(),
    ));

    let variables = state.vars();
    spawn_command(&argv, &variables, actions)
        .map_err(|status| format!("Failed to spawn subshell zxsh: {}", zx_status_str(status)))
}
