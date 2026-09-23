// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use derivative::Derivative;
use starnix_uapi::errors::Errno;
use starnix_uapi::signals::{SIGINT, SIGQUIT, SIGSTOP, Signal};
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    ECHO, ECHOCTL, ECHOE, ECHOK, ECHOKE, ECHONL, ECHOPRT, ICANON, ICRNL, IEXTEN, IGNCR, INLCR,
    ISIG, IUCLC, IUTF8, IXANY, IXON, NOFLSH, OCRNL, OLCUC, ONLCR, ONLRET, ONOCR, OPOST, TABDLY,
    VEOF, VEOL, VEOL2, VERASE, VINTR, VKILL, VLNEXT, VQUIT, VREPRINT, VSTART, VSTOP, VSUSP,
    VWERASE, XTABS, cc_t, errno, error, tcflag_t, uapi,
};
use std::collections::VecDeque;

// CANON_MAX_BYTES is the number of bytes that fit into a single line of
// terminal input in canonical mode. See https://github.com/google/gvisor/blob/master/pkg/sentry/fs/tty/line_discipline.go
const CANON_MAX_BYTES: usize = 4096;

// NON_CANON_MAX_BYTES is the maximum number of bytes that can be read at
// a time in non canonical mode.
const NON_CANON_MAX_BYTES: usize = CANON_MAX_BYTES - 1;

// WAIT_BUFFER_MAX_BYTES is the maximum size of a wait buffer. It is based on
// https://github.com/google/gvisor/blob/master/pkg/sentry/fsimpl/devpts/queue.go
const WAIT_BUFFER_MAX_BYTES: usize = 131072;

const SPACES_PER_TAB: usize = 8;

// DISABLED_CHAR is used to indicate that a control character is disabled.
const DISABLED_CHAR: u8 = 0;

const BACKSPACE_CHAR: u8 = 8; // \b

/// The offset in ASCII between a control character and it's character name.
/// For example, typing CTRL-C on a keyboard generates the value
/// b'C' - CONTROL_OFFSET
const CONTROL_OFFSET: u8 = 0x40;

#[derive(Derivative)]
#[derivative(Default)]
#[derivative(Debug)]
pub struct LineDiscipline {
    /// |true| is the terminal is locked.
    #[derivative(Default(value = "true"))]
    pub locked: bool,

    /// |true| if the output is stopped (due to IXON).
    #[derivative(Default(value = "false"))]
    pub stopped: bool,

    /// Terminal size.
    pub window_size: uapi::winsize,

    /// Terminal configuration.
    #[derivative(Default(value = "get_default_termios()"))]
    termios: uapi::termios2,

    /// True if the terminal is currently in the middle of an erase sequence (ECHOPRT).
    #[derivative(Default(value = "false"))]
    erasing: bool,

    /// True if the next character should be treated literally.
    #[derivative(Default(value = "false"))]
    lnext: bool,

    /// Location in a row of the cursor. Needed to handle certain special characters like
    /// backspace.
    column: usize,

    /// Packet mode state (TIOCPKT).
    #[derivative(Default(value = "false"))]
    packet_mode_enabled: bool,

    /// Packet mode pending events.
    #[derivative(Default(value = "0"))]
    packet_mode_pending_events: u8,

    /// The number of active references to the main part of the terminal. Starts as `None`. The
    /// main part of the terminal is considered closed when this is `Some(0)`.
    main_references: Option<u32>,

    /// The number of active references to the replica part of the terminal. Starts as `None`. The
    /// replica part of the terminal is considered closed when this is `Some(0)`.
    replica_references: Option<u32>,

    /// Input queue of the terminal. Data flow from the main side to the replica side.
    #[derivative(Default(value = "Queue::input_queue()"))]
    input_queue: Option<Queue>,

    /// Output queue of the terminal. Data flow from the replica side to the main side.
    #[derivative(Default(value = "Queue::output_queue()"))]
    output_queue: Option<Queue>,
}

/// Helper trait for input/output buffers.
pub trait InputBuffer {
    fn available(&self) -> usize;
    fn read_to_vec_exact(&mut self, size: usize) -> Result<Vec<u8>, Errno>;
}

pub trait OutputBuffer {
    fn write(&mut self, data: &[u8]) -> Result<usize, Errno>;
}

/// Macro to help working with the terminal queues.
macro_rules! with_queue {
    ($self_:tt . $name:ident . $fn:ident ( $($param:expr),*$(,)?)) => {
        {
        let mut queue = $self_.$name . take().unwrap();
        let result = queue.$fn( $($param),* );
        $self_.$name = Some(queue);
        result
        }
    };
}

/// Keep track of the signals to send when handling terminal content.
#[must_use]
pub struct PendingSignals {
    signals: Vec<Signal>,
}

impl PendingSignals {
    pub fn new() -> Self {
        Self { signals: vec![] }
    }

    /// Add the given signal to the list of signal to send to the associate process group.
    fn add(&mut self, signal: Signal) {
        self.signals.push(signal);
    }

    /// Append all pending signals in `other` to `self`.
    fn append(&mut self, mut other: Self) {
        self.signals.append(&mut other.signals);
    }

    /// Returns a slice of the pending signals.
    pub fn signals(&self) -> &[Signal] {
        &self.signals[..]
    }
}

/// Represents the type of erase operation that can be performed on terminal input.
#[derive(Debug, PartialEq)]
enum EraseType {
    /// Erase a single character (typically triggered by backspace)
    Character,
    /// Erase a word (typically triggered by Ctrl+W)
    Word,
    /// Erase the entire line (typically triggered by Ctrl+U)
    Line,
}

impl LineDiscipline {
    /// Returns the terminal configuration.
    pub fn termios(&self) -> &uapi::termios2 {
        &self.termios
    }

    pub fn is_canon_enabled(&self) -> bool {
        self.termios.has_local_flags(ICANON)
    }

    pub fn is_packet_mode_enabled(&self) -> bool {
        self.packet_mode_enabled
    }

    pub fn set_packet_mode(&mut self, enabled: bool) {
        self.packet_mode_enabled = enabled;
        if !enabled {
            self.packet_mode_pending_events = 0;
        }
    }

    pub fn has_packet_mode_pending_events(&self) -> bool {
        self.packet_mode_enabled && self.packet_mode_pending_events != 0
    }

    /// Returns the number of available bytes to read from the side of the terminal described by
    /// `is_main`.
    pub fn get_available_read_size(&self, is_main: bool) -> usize {
        let queue = if is_main { self.output_queue() } else { self.input_queue() };
        queue.readable_size()
    }

    /// Sets the terminal configuration.
    pub fn set_termios(&mut self, termios: uapi::termios2) -> PendingSignals {
        let old_canon_enabled = self.is_canon_enabled();
        let old_ixon = self.termios.c_iflag & uapi::IXON != 0;
        self.termios = termios;

        if self.packet_mode_enabled {
            let new_ixon = self.termios.c_iflag & uapi::IXON != 0;
            if old_ixon != new_ixon {
                let event = if new_ixon { uapi::TIOCPKT_DOSTOP } else { uapi::TIOCPKT_NOSTOP };
                self.packet_mode_pending_events |= event as u8;
            }
        }

        if old_canon_enabled && !self.is_canon_enabled() {
            with_queue!(self.input_queue.on_canon_disabled(self))
        } else {
            PendingSignals::new()
        }
    }

    /// Flushes queues according to `queue_selector` (TCIFLUSH, TCOFLUSH, TCIOFLUSH).
    pub fn flush(&mut self, is_main: bool, queue_selector: u32) -> Result<(), Errno> {
        // We can receive a flush request from either the main or the replica which switch what the
        // input and output queues are referring to.
        let (input_queue, output_queue) = if is_main {
            (self.output_queue.as_mut().unwrap(), self.input_queue.as_mut().unwrap())
        } else {
            (self.input_queue.as_mut().unwrap(), self.output_queue.as_mut().unwrap())
        };

        let event;
        // For input flushes, we discard all data in the pipeline. Data that's already been
        // delivered to us, and data that's waiting to be delivered.
        //
        // For output flushes, we want to only discard data that has been sent, but not yet
        // delivered to the other side's read_queue. Since this we're a pty and sending is just a
        // memcpy rather than actually going across a wire, the only time this will happen when the
        // read_queue fills up and there is backpressure due to full buffers.
        match queue_selector {
            uapi::TCIFLUSH => {
                input_queue.flush();
                event = uapi::TIOCPKT_FLUSHREAD;
            }
            uapi::TCOFLUSH => {
                output_queue.flush_unprocessed();
                event = uapi::TIOCPKT_FLUSHWRITE;
            }
            uapi::TCIOFLUSH => {
                input_queue.flush();
                output_queue.flush_unprocessed();
                event = uapi::TIOCPKT_FLUSHREAD | uapi::TIOCPKT_FLUSHWRITE;
            }
            _ => return error!(EINVAL),
        };

        if !is_main && self.packet_mode_enabled {
            self.packet_mode_pending_events |= event as u8;
        }

        Ok(())
    }

    /// `close` implementation of the main side of the terminal.
    pub fn main_close(&mut self) {
        self.main_references = self.main_references.map(|v| v - 1);
    }

    /// Called when a new reference to the main side of this terminal is made.
    pub fn main_open(&mut self) {
        self.main_references = Some(self.main_references.unwrap_or(0) + 1);
    }

    pub fn is_main_closed(&self) -> bool {
        matches!(self.main_references, Some(0))
    }

    /// `query_events` implementation of the main side of the terminal.
    pub fn main_query_events(&self) -> FdEvents {
        if self.is_replica_closed() && self.output_queue().readable_size() == 0 {
            return FdEvents::POLLOUT | FdEvents::POLLHUP;
        }
        let mut events =
            self.output_queue().read_readiness() | self.input_queue().write_readiness();
        if self.packet_mode_enabled && self.packet_mode_pending_events != 0 {
            events |= FdEvents::POLLIN | FdEvents::POLLPRI;
        }
        events
    }

    /// `read` implementation of the main side of the terminal.
    pub fn main_read(&mut self, data: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        if self.is_replica_closed() && self.output_queue().readable_size() == 0 {
            return error!(EIO);
        }
        if self.packet_mode_enabled {
            if self.packet_mode_pending_events != 0 {
                let event = self.packet_mode_pending_events;
                self.packet_mode_pending_events = 0;
                return data.write(&[event]);
            }
            if self.output_queue().readable_size() == 0 {
                return error!(EAGAIN);
            }
            let written = data.write(&[0])?;
            if written == 0 {
                return Ok(0);
            }
            let res = with_queue!(self.output_queue.read(self, data));
            match res {
                Ok(n) => return Ok(n + 1),
                Err(e) if e == errno!(EAGAIN) => return Ok(1),
                Err(e) => return Err(e),
            }
        }
        with_queue!(self.output_queue.read(self, data))
    }

    /// `write` implementation of the main side of the terminal.
    pub fn main_write(
        &mut self,
        data: &mut dyn InputBuffer,
    ) -> Result<(usize, PendingSignals), Errno> {
        with_queue!(self.input_queue.write(self, data))
    }

    /// `close` implementation of the replica side of the terminal.
    pub fn replica_close(&mut self) {
        self.replica_references = self.replica_references.map(|v| v - 1);
    }

    /// Called when a new reference to the replica side of this terminal is made.
    pub fn replica_open(&mut self) {
        self.replica_references = Some(self.replica_references.unwrap_or(0) + 1);
    }

    pub fn is_replica_closed(&self) -> bool {
        matches!(self.replica_references, Some(0))
    }

    /// `query_events` implementation of the replica side of the terminal.
    pub fn replica_query_events(&self) -> FdEvents {
        if self.is_main_closed() {
            return FdEvents::POLLIN | FdEvents::POLLOUT | FdEvents::POLLERR | FdEvents::POLLHUP;
        }
        self.input_queue().read_readiness() | self.output_queue().write_readiness()
    }

    /// `read` implementation of the replica side of the terminal.
    pub fn replica_read(&mut self, data: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        if self.is_main_closed() {
            return Ok(0);
        }
        with_queue!(self.input_queue.read(self, data))
    }

    /// `write` implementation of the replica side of the terminal.
    pub fn replica_write(&mut self, data: &mut dyn InputBuffer) -> Result<usize, Errno> {
        if self.is_main_closed() {
            return error!(EIO);
        }
        if self.stopped && self.termios.has_input_flags(IXON) {
            return error!(EAGAIN);
        }
        let (read_from_userspace, signals) = with_queue!(self.output_queue.write(self, data))?;
        // Writing to the replica side never generates signals.
        assert!(signals.signals().is_empty());
        Ok(read_from_userspace)
    }

    /// Returns the input queue.
    fn input_queue(&self) -> &Queue {
        self.input_queue.as_ref().unwrap()
    }

    /// Returns the output_queue. The Option is always filled.
    fn output_queue(&self) -> &Queue {
        self.output_queue.as_ref().unwrap()
    }

    /// Return whether a signal must be send when receiving `byte`, and if yes, which.
    fn handle_signals(&mut self, byte: RawByte) -> Option<Signal> {
        if !self.termios.has_local_flags(ISIG) {
            return None;
        }
        self.termios.signal(byte)
    }

    fn extend_echo_bytes(&self, target: &mut Vec<RawByte>, byte: RawByte) {
        if self.termios.has_local_flags(ECHOCTL) {
            if let Some(control_character_echo) = generate_control_character_echo(byte) {
                target.extend(control_character_echo);
                return;
            }
        }
        target.push(byte);
    }

    fn transform(
        &mut self,
        is_input: bool,
        queue: &mut Queue,
        buffer: &[RawByte],
    ) -> (usize, PendingSignals) {
        if is_input {
            self.transform_input(queue, buffer)
        } else {
            (self.transform_output(queue, buffer), PendingSignals::new())
        }
    }

    fn transform_output(&mut self, queue: &mut Queue, original_buffer: &[RawByte]) -> usize {
        let mut buffer = original_buffer;

        // transform_output is effectively always in noncanonical mode, as the
        // main termios never has ICANON set.

        if !self.termios.has_output_flags(OPOST) {
            let limit = CANON_MAX_BYTES.saturating_sub(queue.readable_size());
            if limit == 0 {
                return 0;
            }
            let to_write = std::cmp::min(limit, buffer.len());
            queue.read_queue.push_back(buffer[..to_write].to_vec());
            return to_write;
        }

        let mut return_value = 0;
        while !buffer.is_empty()
            && queue.readable_size() + queue.line_buffer.len() < CANON_MAX_BYTES
        {
            let size = compute_next_character_size(buffer, &self.termios);
            let mut character_bytes = buffer[..size].to_vec();
            return_value += size;
            buffer = &buffer[size..];

            if self.termios.has_output_flags(OLCUC) {
                character_bytes[0].make_ascii_uppercase();
            }
            match character_bytes[0] {
                b'\n' => {
                    if self.termios.has_output_flags(ONLRET) {
                        self.column = 0;
                    }
                    if self.termios.has_output_flags(ONLCR) {
                        queue.line_buffer.extend_from_slice(&[b'\r', b'\n']);
                        continue;
                    }
                }
                b'\r' => {
                    if self.termios.has_output_flags(ONOCR) && self.column == 0 {
                        continue;
                    }
                    if self.termios.has_output_flags(OCRNL) {
                        character_bytes[0] = b'\n';
                        if self.termios.has_output_flags(ONLRET) {
                            self.column = 0;
                        }
                    } else {
                        self.column = 0;
                    }
                }
                b'\t' => {
                    let spaces = SPACES_PER_TAB - self.column % SPACES_PER_TAB;
                    if self.termios.c_oflag & TABDLY == XTABS {
                        self.column += spaces;
                        queue.line_buffer.extend(std::iter::repeat(b' ').take(spaces));
                        continue;
                    }
                    self.column += spaces;
                }
                BACKSPACE_CHAR => {
                    if self.column > 0 {
                        self.column -= 1;
                    }
                }
                _ => {
                    self.column += 1;
                }
            }
            queue.line_buffer.append(&mut character_bytes);
        }
        if !queue.line_buffer.is_empty() {
            queue.flush_line_buffer();
        }
        return_value
    }

    fn transform_input(
        &mut self,
        queue: &mut Queue,
        original_buffer: &[RawByte],
    ) -> (usize, PendingSignals) {
        let mut buffer = original_buffer;

        let max_bytes = if self.termios.has_local_flags(ICANON) {
            CANON_MAX_BYTES
        } else {
            NON_CANON_MAX_BYTES
        };

        let mut return_value = 0;
        let mut signals = PendingSignals::new();
        while !buffer.is_empty()
            && queue.readable_size() + queue.line_buffer.len() < CANON_MAX_BYTES
        {
            let size = compute_next_character_size(buffer, &self.termios);
            let mut character_bytes = buffer[..size].to_vec();
            // It is guaranteed that character_bytes has at least one element.

            if self.lnext {
                self.lnext = false;
                if self.termios.has_local_flags(ECHO) {
                    let mut echo_bytes = vec![];
                    self.extend_echo_bytes(&mut echo_bytes, character_bytes[0]);
                    signals.append(with_queue!(self.output_queue.write_bytes(self, &echo_bytes)));
                }

                queue.line_buffer.extend_from_slice(&character_bytes);
                buffer = &buffer[size..];
                return_value += size;
                continue;
            }

            if self.termios.has_local_flags(IEXTEN) {
                // VLNEXT
                if character_bytes[0] == self.termios.c_cc[VLNEXT as usize]
                    && self.termios.c_cc[VLNEXT as usize] != DISABLED_CHAR
                {
                    self.lnext = true;
                    if self.termios.has_local_flags(ECHO) && self.termios.has_local_flags(ECHOCTL) {
                        let echo_bytes = vec![b'^', BACKSPACE_CHAR];
                        signals
                            .append(with_queue!(self.output_queue.write_bytes(self, &echo_bytes)));
                    }
                    buffer = &buffer[size..];
                    return_value += size;
                    continue;
                }
                // VREPRINT
                if character_bytes[0] == self.termios.c_cc[VREPRINT as usize]
                    && self.termios.c_cc[VREPRINT as usize] != DISABLED_CHAR
                {
                    if self.termios.has_local_flags(ECHO) {
                        let mut echo_bytes = vec![];
                        self.extend_echo_bytes(&mut echo_bytes, character_bytes[0]);
                        echo_bytes.push(b'\n');
                        for byte in &queue.line_buffer {
                            self.extend_echo_bytes(&mut echo_bytes, *byte);
                        }
                        signals
                            .append(with_queue!(self.output_queue.write_bytes(self, &echo_bytes)));
                    }
                    buffer = &buffer[size..];
                    return_value += size;
                    continue;
                }
            }

            if self.termios.has_input_flags(IUCLC) && self.termios.has_local_flags(IEXTEN) {
                character_bytes[0].make_ascii_lowercase();
            }

            let mut signal_generated = false;
            if let Some(signal) = self.handle_signals(character_bytes[0]) {
                signals.add(signal);
                signal_generated = true;
                if !self.termios.has_local_flags(NOFLSH) {
                    queue.flush();
                    if let Some(ref mut output_queue) = self.output_queue {
                        output_queue.flush();
                    }
                }
            }

            // Handle IXON/IXOFF (software flow control)
            if self.termios.has_input_flags(IXON) {
                if character_bytes[0] == self.termios.c_cc[VSTOP as usize] {
                    self.stopped = true;
                    buffer = &buffer[size..];
                    return_value += size;
                    continue;
                }
                // POSIX says:
                // "If IXON is set, start/stop output control is enabled. A received STOP character
                // suspends output and a received START character restarts output. The STOP and
                // START characters are not read, but performing the flow control functions."
                //
                // "If IXANY is set, any input character restarts output that has been suspended."
                if self.stopped
                    && (character_bytes[0] == self.termios.c_cc[VSTART as usize]
                        || self.termios.has_input_flags(IXANY))
                {
                    self.stopped = false;
                    // If it was START, we consume it. If it was IXANY (and not START), we usually
                    // process it?
                    // "The START character is not read".
                    // If IXANY is set and char != START, we should restart AND process the char.
                    if character_bytes[0] == self.termios.c_cc[VSTART as usize] {
                        buffer = &buffer[size..];
                        return_value += size;
                        continue;
                    }
                }
            }

            match character_bytes[0] {
                b'\r' => {
                    if self.termios.has_input_flags(IGNCR) {
                        buffer = &buffer[size..];
                        return_value += size;
                        continue;
                    }
                    if self.termios.has_input_flags(ICRNL) {
                        character_bytes[0] = b'\n';
                    }
                }
                b'\n' => {
                    if self.termios.has_input_flags(INLCR) {
                        character_bytes[0] = b'\r'
                    }
                }
                _ => {}
            }
            // In canonical mode, we discard non-terminating characters
            // after the first 4095.
            if self.termios.has_local_flags(ICANON)
                && queue.line_buffer.len() + size >= max_bytes
                && !self.termios.is_terminating(&character_bytes)
            {
                buffer = &buffer[size..];
                return_value += size;
                continue;
            }

            if queue.line_buffer.len() + size > max_bytes {
                break;
            }

            buffer = &buffer[size..];
            return_value += size;

            let first_byte = character_bytes[0];

            // If we get EOF, push whatever we have line_buffer to read_queue, then push an empty datagram.
            if self.termios.has_local_flags(ICANON) && self.termios.is_eof(first_byte) {
                if !queue.line_buffer.is_empty() {
                    queue.flush_line_buffer();
                }
                queue.read_queue.push_back(vec![]);
                break;
            }

            let mut maybe_erase_span = None;
            let mut erase_type = None;
            if self.termios.has_local_flags(ICANON) {
                if self.termios.is_erase(first_byte) {
                    maybe_erase_span =
                        Some(compute_last_character_span(&queue.line_buffer[..], &self.termios));
                    erase_type = Some(EraseType::Character);
                } else if self.termios.is_werase(first_byte) {
                    maybe_erase_span =
                        Some(compute_last_word_span(&queue.line_buffer[..], &self.termios));
                    erase_type = Some(EraseType::Word);
                }
                if self.termios.is_kill(first_byte) {
                    maybe_erase_span =
                        Some(compute_last_line_span(&queue.line_buffer[..], &self.termios));
                    erase_type = Some(EraseType::Line);
                }
            }

            let mut erased_bytes = Option::None;
            if let Some(erase_span) = maybe_erase_span {
                if erase_span.bytes == 0 {
                    continue;
                }
                if self.termios.has_local_flags(ECHOPRT) {
                    erased_bytes = Some(
                        queue.line_buffer[queue.line_buffer.len() - erase_span.bytes..].to_vec(),
                    );
                }
                queue.line_buffer.truncate(queue.line_buffer.len() - erase_span.bytes);
            } else if !signal_generated {
                queue.line_buffer.extend_from_slice(&character_bytes);
            }

            // Anything written to the read buffer will have to be echoed.
            let mut echo_bytes = vec![];
            if self.termios.has_local_flags(ECHO) {
                if let Some(erase_span) = maybe_erase_span {
                    match erase_type {
                        Some(EraseType::Character) | Some(EraseType::Word) => {
                            if self.termios.has_local_flags(ECHOPRT) {
                                if let Some(bytes) = erased_bytes {
                                    if !self.erasing {
                                        echo_bytes.push(b'\\');
                                        self.erasing = true;
                                    }
                                    for byte in bytes.iter().rev() {
                                        self.extend_echo_bytes(&mut echo_bytes, *byte);
                                    }
                                }
                            } else if self.termios.has_local_flags(ECHOE) {
                                echo_bytes = generate_erase_echo(&erase_span);
                            }
                        }
                        Some(EraseType::Line) => {
                            if self.termios.has_local_flags(ECHOKE) {
                                echo_bytes = generate_erase_echo(&erase_span);
                            } else if self.termios.has_local_flags(ECHOK) {
                                self.extend_echo_bytes(&mut echo_bytes, first_byte);
                                echo_bytes.push(b'\n');
                            }
                        }
                        None => {
                            unreachable!("Erase type should be Some when maybe_erase_span is Some")
                        }
                    }
                    if self.erasing && queue.line_buffer.is_empty() {
                        echo_bytes.push(b'/');
                        self.erasing = false;
                    }
                } else {
                    if self.erasing && first_byte != b'\n' {
                        echo_bytes.push(b'/');
                        self.erasing = false;
                    }
                }

                let needs_normal_echo =
                    if maybe_erase_span.is_some() { echo_bytes.is_empty() } else { true };

                if needs_normal_echo {
                    let mut char_echo = vec![];
                    if self.termios.has_local_flags(ECHOCTL) {
                        if let Some(control_character_echo) =
                            generate_control_character_echo(first_byte)
                        {
                            char_echo = control_character_echo;
                        }
                    }
                    if char_echo.is_empty() {
                        char_echo = character_bytes.clone();
                    }
                    echo_bytes.extend(char_echo);
                }
            } else if self.termios.has_local_flags(ECHONL) && first_byte == b'\n' {
                echo_bytes.extend_from_slice(&character_bytes);
            }

            if !echo_bytes.is_empty() {
                signals.append(with_queue!(self.output_queue.write_bytes(self, &echo_bytes)));
            }

            // If we finish a line, make it available for reading.
            if self.termios.has_local_flags(ICANON) && self.termios.is_terminating(&character_bytes)
            {
                queue.flush_line_buffer();
            }
        }
        // In noncanonical mode, everything is readable.
        if !self.termios.has_local_flags(ICANON) && !queue.line_buffer.is_empty() {
            queue.flush_line_buffer();
        }

        (return_value, signals)
    }
}

/// Alias used to mark bytes in the queues that have not yet been processed and pushed into the
/// read buffer. See `Queue`.
type RawByte = u8;

#[derive(Debug, Default)]
struct Queue {
    /// The queue of data ready to be read. Each element is a "datagram" (line or chunk).
    /// Empty byte vectors represent EOF markers (read returns 0).
    read_queue: VecDeque<Vec<u8>>,

    /// The incomplete line/chunk being processed but not yet ready for the read_queue.
    /// In Canonical mode, this holds the current line being edited.
    /// In Non-Canonical mode, this holds data until it is pushed to the read_queue.
    line_buffer: Vec<u8>,

    /// Data that can't fit into readBuf. It is put here until it can be loaded into the read
    /// buffer. Contains data that hasn't been processed.
    wait_buffers: VecDeque<Vec<RawByte>>,

    /// The length of the data in `wait_buffers`.
    total_wait_buffer_length: usize,

    /// Whether this queue in the input queue. Needed to know how to transform received data.
    is_input: bool,
}

impl Queue {
    fn output_queue() -> Option<Self> {
        Some(Queue { is_input: false, ..Default::default() })
    }

    fn input_queue() -> Option<Self> {
        Some(Queue { is_input: true, ..Default::default() })
    }

    /// Returns whether the queue is ready to be written to.
    fn write_readiness(&self) -> FdEvents {
        if self.total_wait_buffer_length < WAIT_BUFFER_MAX_BYTES {
            FdEvents::POLLOUT
        } else {
            FdEvents::empty()
        }
    }

    /// Returns whether the queue is ready to be read from.
    fn read_readiness(&self) -> FdEvents {
        // If there's an empty "datagram" in read_queue, it means EOF, which is "readable" (returns 0).
        if !self.read_queue.is_empty() { FdEvents::POLLIN } else { FdEvents::empty() }
    }

    /// Returns the number of bytes ready to be read.
    fn readable_size(&self) -> usize {
        // We sum up everything in the read_queue.
        // NOTE: This might over-report if we only return one datagram at a time, but for poll/FIONREAD it's generally answering "how much is there".
        self.read_queue.iter().map(|v| v.len()).sum()
    }

    /// Read from the queue into `data`. Returns the number of bytes copied.
    fn read(
        &mut self,
        terminal: &mut LineDiscipline,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        if self.read_queue.is_empty() {
            return error!(EAGAIN);
        }

        let mut total_written = 0;
        while let Some(mut packet) = self.read_queue.pop_front() {
            if packet.is_empty() {
                if total_written > 0 {
                    // We've already read some data. We need to complete the read with that data and
                    // leave the empty datagram in the queue to signal EOF on the next read.
                    self.read_queue.push_front(packet);
                }
                break;
            }

            match data.write(&packet) {
                Ok(written) => {
                    total_written += written;
                    if written < packet.len() {
                        // Put back the unread part.
                        let remaining = packet.split_off(written);
                        self.read_queue.push_front(remaining);
                        // Destination full.
                        break;
                    }

                    // If we are in canonical input mode, we stop after one packet (one line).
                    if self.is_input && terminal.termios.has_local_flags(ICANON) {
                        break;
                    }
                }
                Err(e) => {
                    // If write failed, push back the whole packet.
                    self.read_queue.push_front(packet);
                    if total_written > 0 {
                        // If we managed to write something before error, return success.
                        return Ok(total_written);
                    }
                    return Err(e);
                }
            }
        }

        let signals = self.drain_waiting_buffer(terminal);
        assert!(signals.signals().is_empty());
        Ok(total_written)
    }

    /// Writes to the queue from `data`. Returns the number of bytes copied.
    fn write(
        &mut self,
        terminal: &mut LineDiscipline,
        data: &mut dyn InputBuffer,
    ) -> Result<(usize, PendingSignals), Errno> {
        let room = WAIT_BUFFER_MAX_BYTES - self.total_wait_buffer_length;
        let data_length = data.available();
        if room == 0 && data_length > 0 {
            return error!(EAGAIN);
        }
        let buffer = data.read_to_vec_exact(std::cmp::min(room, data_length))?;
        let read_from_userspace = buffer.len();
        let signals = self.push_to_waiting_buffer(terminal, buffer);
        Ok((read_from_userspace, signals))
    }

    /// Writes the given `buffer` to the queue.
    fn write_bytes(&mut self, terminal: &mut LineDiscipline, buffer: &[RawByte]) -> PendingSignals {
        self.push_to_waiting_buffer(terminal, buffer.to_vec())
    }

    /// Pushes the given buffer into the wait_buffers, and process the wait_buffers.
    fn push_to_waiting_buffer(
        &mut self,
        terminal: &mut LineDiscipline,
        buffer: Vec<RawByte>,
    ) -> PendingSignals {
        self.total_wait_buffer_length += buffer.len();
        self.wait_buffers.push_back(buffer);
        self.drain_waiting_buffer(terminal)
    }

    /// Processes the wait_buffers, filling the read buffer.
    fn drain_waiting_buffer(&mut self, terminal: &mut LineDiscipline) -> PendingSignals {
        let mut signals_to_return = PendingSignals::new();
        while let Some(wait_buffer) = self.wait_buffers.pop_front() {
            self.total_wait_buffer_length -= wait_buffer.len();
            let (count, signals) = terminal.transform(self.is_input, self, &wait_buffer);
            signals_to_return.append(signals);
            if count != wait_buffer.len() {
                let remaining = wait_buffer[count..].to_vec();
                self.total_wait_buffer_length += remaining.len();
                self.wait_buffers.push_front(remaining);
                break;
            }
        }
        signals_to_return
    }

    /// Flushed the line buffer to the read queue.
    fn flush_line_buffer(&mut self) {
        self.read_queue.push_back(std::mem::take(&mut self.line_buffer));
    }

    /// Flush the content of the queue.
    fn flush(&mut self) {
        self.read_queue.clear();
        self.line_buffer.clear();
        self.wait_buffers.clear();
        self.total_wait_buffer_length = 0;
    }

    /// Flush only the part of the queue which has not yet been processed.
    fn flush_unprocessed(&mut self) {
        self.wait_buffers.clear();
        self.total_wait_buffer_length = 0;
    }

    /// Called when the queue is moved from canonical mode, to non canonical mode.
    fn on_canon_disabled(&mut self, terminal: &mut LineDiscipline) -> PendingSignals {
        let signals = self.drain_waiting_buffer(terminal);
        if !self.line_buffer.is_empty() {
            self.flush_line_buffer();
        }
        signals
    }
}

// Helper functions (copied from terminal.rs)
// Returns the ASCII representation of the given char. This will assert if the character is not
// ascii.
fn get_ascii(c: char) -> u8 {
    let mut dest: [u8; 1] = [0];
    c.encode_utf8(&mut dest);
    dest[0]
}

// Returns the control character associated with the given letter.
fn get_control_character(c: char) -> cc_t {
    get_ascii(c) - get_ascii('A') + 1
}

// Returns the default control characters of a terminal.
fn get_default_control_characters() -> [cc_t; 19usize] {
    [
        get_control_character('C'),  // VINTR = ^C
        get_control_character('\\'), // VQUIT = ^\
        get_ascii('\x7f'),           // VERASE = DEL
        get_control_character('U'),  // VKILL = ^U
        get_control_character('D'),  // VEOF = ^D
        0,                           // VTIME
        1,                           // VMIN
        0,                           // VSWTC
        get_control_character('Q'),  // VSTART = ^Q
        get_control_character('S'),  // VSTOP = ^S
        get_control_character('Z'),  // VSUSP = ^Z
        0,                           // VEOL
        get_control_character('R'),  // VREPRINT = ^R
        get_control_character('O'),  // VDISCARD = ^O
        get_control_character('W'),  // VWERASE = ^W
        get_control_character('V'),  // VLNEXT = ^V
        0,                           // VEOL2
        0,                           // Remaining data in the array,
        0,                           // Remaining data in the array,
    ]
}

const DEFAULT_SPEED: u32 = 38400;

// Returns the default replica terminal configuration.
pub fn get_default_termios() -> uapi::termios2 {
    uapi::termios2 {
        c_iflag: uapi::ICRNL | uapi::IXON,
        c_oflag: uapi::OPOST | uapi::ONLCR,
        c_cflag: uapi::B38400 | uapi::CS8 | uapi::CREAD,
        c_lflag: uapi::ISIG
            | uapi::ICANON
            | uapi::ECHO
            | uapi::ECHOE
            | uapi::ECHOK
            | uapi::ECHOCTL
            | uapi::ECHOKE
            | uapi::IEXTEN,
        c_line: 0,
        c_cc: get_default_control_characters(),
        c_ispeed: DEFAULT_SPEED,
        c_ospeed: DEFAULT_SPEED,
    }
}

/// Helper trait for termios to help parse the configuration.
trait TermIOS {
    fn has_input_flags(&self, flags: tcflag_t) -> bool;
    fn has_output_flags(&self, flags: tcflag_t) -> bool;
    fn has_local_flags(&self, flags: tcflag_t) -> bool;
    fn is_eof(&self, c: RawByte) -> bool;
    fn is_erase(&self, c: RawByte) -> bool;
    fn is_werase(&self, c: RawByte) -> bool;
    fn is_kill(&self, c: RawByte) -> bool;
    fn is_terminating(&self, character_bytes: &[RawByte]) -> bool;
    fn signal(&self, c: RawByte) -> Option<Signal>;
}

impl TermIOS for uapi::termios2 {
    fn has_input_flags(&self, flags: tcflag_t) -> bool {
        self.c_iflag & flags == flags
    }
    fn has_output_flags(&self, flags: tcflag_t) -> bool {
        self.c_oflag & flags == flags
    }
    fn has_local_flags(&self, flags: tcflag_t) -> bool {
        self.c_lflag & flags == flags
    }
    fn is_eof(&self, c: RawByte) -> bool {
        c == self.c_cc[VEOF as usize] && self.c_cc[VEOF as usize] != DISABLED_CHAR
    }
    fn is_erase(&self, c: RawByte) -> bool {
        c == self.c_cc[VERASE as usize] && self.c_cc[VERASE as usize] != DISABLED_CHAR
    }
    fn is_werase(&self, c: RawByte) -> bool {
        c == self.c_cc[VWERASE as usize]
            && self.c_cc[VWERASE as usize] != DISABLED_CHAR
            && self.has_local_flags(IEXTEN)
    }
    fn is_kill(&self, c: RawByte) -> bool {
        c == self.c_cc[VKILL as usize] && self.c_cc[VKILL as usize] != DISABLED_CHAR
    }
    fn is_terminating(&self, character_bytes: &[RawByte]) -> bool {
        // All terminating characters are 1 byte.
        if character_bytes.len() != 1 {
            return false;
        }
        let c = character_bytes[0];

        // Is this the user-set EOF character?
        if self.is_eof(c) {
            return true;
        }

        if c == DISABLED_CHAR {
            return false;
        }
        if c == b'\n' || c == self.c_cc[VEOL as usize] {
            return true;
        }
        if c == self.c_cc[VEOL2 as usize] {
            return self.has_local_flags(IEXTEN);
        }
        false
    }
    fn signal(&self, c: RawByte) -> Option<Signal> {
        if c == DISABLED_CHAR {
            return None;
        }
        if c == self.c_cc[VINTR as usize] {
            return Some(SIGINT);
        }
        if c == self.c_cc[VQUIT as usize] {
            return Some(SIGQUIT);
        }
        if c == self.c_cc[VSUSP as usize] {
            return Some(SIGSTOP);
        }
        None
    }
}

fn compute_next_character_size(buffer: &[RawByte], termios: &uapi::termios2) -> usize {
    if !termios.has_input_flags(IUTF8) {
        return 1;
    }

    #[derive(Default)]
    struct Receiver {
        done: Option<bool>,
    }

    impl utf8parse::Receiver for Receiver {
        fn codepoint(&mut self, _c: char) {
            self.done = Some(true);
        }
        fn invalid_sequence(&mut self) {
            self.done = Some(false);
        }
    }

    let mut byte_count = 0;
    let mut receiver = Receiver::default();
    let mut parser = utf8parse::Parser::new();
    while receiver.done.is_none() && byte_count < buffer.len() {
        parser.advance(&mut receiver, buffer[byte_count]);
        byte_count += 1;
    }
    if receiver.done == Some(true) { byte_count } else { 1 }
}

fn is_ascii(c: RawByte) -> bool {
    c & 0x80 == 0
}

fn is_utf8_start(c: RawByte) -> bool {
    c & 0xC0 == 0xC0
}

fn generate_erase_echo(erase_span: &BufferSpan) -> Vec<RawByte> {
    let erase_echo = [BACKSPACE_CHAR, b' ', BACKSPACE_CHAR];
    erase_echo.iter().cycle().take(erase_echo.len() * erase_span.characters).map(|c| *c).collect()
}

fn generate_control_character_echo(c: RawByte) -> Option<Vec<RawByte>> {
    if matches!(c, 0..=0x8 | 0xB..=0xC | 0xE..=0x1F) {
        Some(vec![b'^', c + CONTROL_OFFSET])
    } else {
        None
    }
}

#[derive(Default, Debug, Clone, Copy)]
struct BufferSpan {
    bytes: usize,
    characters: usize,
}

impl std::ops::AddAssign<Self> for BufferSpan {
    fn add_assign(&mut self, rhs: Self) {
        self.bytes += rhs.bytes;
        self.characters += rhs.characters;
    }
}

fn compute_last_character_span(buffer: &[RawByte], termios: &uapi::termios2) -> BufferSpan {
    if buffer.is_empty() {
        return BufferSpan::default();
    }
    if termios.has_input_flags(IUTF8) {
        let mut bytes = 0;
        for c in buffer.iter().rev() {
            bytes += 1;
            if is_ascii(*c) || is_utf8_start(*c) {
                return BufferSpan { bytes, characters: 1 };
            }
        }
        BufferSpan::default()
    } else {
        BufferSpan { bytes: 1, characters: 1 }
    }
}

fn compute_last_word_span(buffer: &[RawByte], termios: &uapi::termios2) -> BufferSpan {
    fn is_whitespace(c: RawByte) -> bool {
        c == b' ' || c == b'\t'
    }

    let mut in_word = false;
    let mut word_span = BufferSpan::default();
    let mut remaining = buffer.len();
    loop {
        let span = compute_last_character_span(&buffer[..remaining], termios);
        if span.bytes == 0 {
            break;
        }
        if span.bytes == 1 {
            let c = buffer[remaining - 1];
            if in_word {
                if is_whitespace(c) {
                    break;
                }
            } else {
                if !is_whitespace(c) {
                    in_word = true;
                }
            }
        }
        remaining -= span.bytes;
        word_span += span;
    }

    word_span
}

fn compute_last_line_span(buffer: &[RawByte], termios: &uapi::termios2) -> BufferSpan {
    let mut line_span = BufferSpan::default();
    let mut remaining = buffer.len();

    loop {
        let span = compute_last_character_span(&buffer[..remaining], termios);
        if span.bytes == 0 {
            break;
        }
        if span.bytes == 1 {
            let c = buffer[remaining - 1];
            if c == b'\n' {
                break;
            }
        }
        remaining -= span.bytes;
        line_span += span;
    }

    line_span
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::fuchsia::test]
    fn test_ascii_conversion() {
        assert_eq!(get_ascii(' '), 32);
    }

    #[::fuchsia::test]
    fn test_control_character() {
        assert_eq!(get_control_character('C'), 3);
    }

    #[::fuchsia::test]
    fn test_compute_next_character_size_non_utf8() {
        let termios = get_default_termios();
        for i in 0..=255 {
            let array: &[u8] = &[i, 0xa9, 0];
            assert_eq!(compute_next_character_size(array, &termios), 1);
        }
    }

    #[::fuchsia::test]
    fn test_compute_next_character_size_utf8() {
        let mut termios = get_default_termios();
        termios.c_iflag |= IUTF8;
        for i in 0..128 {
            let array: &[RawByte] = &[i, 0xa9, 0];
            assert_eq!(compute_next_character_size(array, &termios), 1);
        }
        let array: &[RawByte] = &[0xc2, 0xa9, 0];
        assert_eq!(compute_next_character_size(array, &termios), 2);
        let array: &[RawByte] = &[0xc2, 255, 0];
        assert_eq!(compute_next_character_size(array, &termios), 1);
    }

    #[::fuchsia::test]
    fn test_signal_handling_with_disabled_chars() {
        let mut termios = get_default_termios();
        termios.c_cc[VINTR as usize] = DISABLED_CHAR;
        termios.c_cc[VQUIT as usize] = DISABLED_CHAR;
        termios.c_cc[VSUSP as usize] = DISABLED_CHAR;

        assert_eq!(termios.signal(0), None);
        assert_eq!(termios.signal(3), None); // Normally ^C (SIGINT)
        assert_eq!(termios.signal(28), None); // Normally ^\ (SIGQUIT)
        assert_eq!(termios.signal(26), None); // Normally ^Z (SIGSTOP)
    }

    struct TestBuffer {
        data: Vec<u8>,
    }

    impl InputBuffer for TestBuffer {
        fn available(&self) -> usize {
            self.data.len()
        }
        fn read_to_vec_exact(&mut self, size: usize) -> Result<Vec<u8>, Errno> {
            if size > self.data.len() {
                return error!(EAGAIN);
            }
            Ok(self.data.drain(0..size).collect())
        }
    }

    impl OutputBuffer for TestBuffer {
        fn write(&mut self, data: &[u8]) -> Result<usize, Errno> {
            self.data.extend_from_slice(data);
            Ok(data.len())
        }
    }

    #[::fuchsia::test]
    fn test_flush() {
        fn make_ld() -> LineDiscipline {
            let mut ld = LineDiscipline::default();
            ld.main_open();
            ld.replica_open();

            let mut termios = get_default_termios();
            termios.c_lflag &= !ECHO;
            termios.c_oflag &= !OPOST;
            let _ = ld.set_termios(termios);

            // Write some data from main to the replica.
            // This goes to input_queue.
            let mut input = TestBuffer { data: b"ping\n".to_vec() };
            let (written, signals) = ld.main_write(&mut input).unwrap();
            assert_eq!(written, 5);
            assert!(signals.signals().is_empty());

            // Write some data from replica to main.
            // This goes to output_queue.
            let mut output = TestBuffer { data: b"pong\n".to_vec() };
            let written = ld.replica_write(&mut output).unwrap();
            assert_eq!(written, 5);
            ld
        }

        let mut read_buf = TestBuffer { data: vec![] };

        // A TCIFLUSH from the main side should flush only the main's input (output_queue)
        let mut ld = make_ld();
        ld.flush(true, uapi::TCIFLUSH).unwrap();
        read_buf.data.clear();
        assert_eq!(error!(EAGAIN), ld.main_read(&mut read_buf));
        assert!(read_buf.data.is_empty());
        read_buf.data.clear();
        assert!(ld.replica_read(&mut read_buf).is_ok());
        assert_eq!(read_buf.data, b"ping\n");

        // A TCIFLUSH from the replica side should flush only the replica's input (input_queue)
        let mut ld = make_ld();
        ld.flush(false, uapi::TCIFLUSH).unwrap();
        read_buf.data.clear();
        assert!(ld.main_read(&mut read_buf).is_ok());
        assert_eq!(read_buf.data, b"pong\n");
        read_buf.data.clear();
        assert_eq!(error!(EAGAIN), ld.replica_read(&mut read_buf));
        assert!(read_buf.data.is_empty());

        // A TCOFLUSH from the main side should do nothing (instantaneous transmission)
        let mut ld = make_ld();
        ld.flush(true, uapi::TCOFLUSH).unwrap();
        read_buf.data.clear();
        assert!(ld.main_read(&mut read_buf).is_ok());
        assert_eq!(read_buf.data, b"pong\n");
        read_buf.data.clear();
        assert!(ld.replica_read(&mut read_buf).is_ok());
        assert_eq!(read_buf.data, b"ping\n");

        // A TCOFLUSH from the replica side should do nothing
        let mut ld = make_ld();
        ld.flush(false, uapi::TCOFLUSH).unwrap();
        read_buf.data.clear();
        assert!(ld.main_read(&mut read_buf).is_ok());
        assert_eq!(read_buf.data, b"pong\n");
        read_buf.data.clear();
        assert!(ld.replica_read(&mut read_buf).is_ok());
        assert_eq!(read_buf.data, b"ping\n");

        // A TCIOFLUSH from main should flush only main's input (output_queue)
        let mut ld = make_ld();
        ld.flush(true, uapi::TCIOFLUSH).unwrap();
        read_buf.data.clear();
        assert_eq!(error!(EAGAIN), ld.main_read(&mut read_buf));
        read_buf.data.clear();
        assert!(ld.replica_read(&mut read_buf).is_ok());
        assert_eq!(read_buf.data, b"ping\n");

        // A TCIOFLUSH from replica should flush only replica's input (input_queue)
        let mut ld = make_ld();
        ld.flush(false, uapi::TCIOFLUSH).unwrap();
        read_buf.data.clear();
        assert!(ld.main_read(&mut read_buf).is_ok());
        assert_eq!(read_buf.data, b"pong\n");
        read_buf.data.clear();
        assert_eq!(error!(EAGAIN), ld.replica_read(&mut read_buf));
    }
}

pub mod testing;
