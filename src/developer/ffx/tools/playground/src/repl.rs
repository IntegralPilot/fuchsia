// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_fs::File;
use crossterm::QueueableCommand;
use crossterm::cursor::{MoveLeft, MoveRight, MoveUp};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use futures::FutureExt as _;
use futures::future::{Either, LocalBoxFuture, poll_fn, select};
use futures::io::{AsyncReadExt as _, AsyncWrite};
use futures::task::AtomicWaker;
use std::cell::Cell;
use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, Write as _, stdin, stdout};
use std::os::unix::io::{AsRawFd as _, FromRawFd as _};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::strict_mutex::StrictMutex;
use crate::term_util::printed_length;

static DOUBLE_TAB_TIME: Duration = Duration::from_millis(500);

/// Get the longest common prefix of two strings.
fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let len = a
        .char_indices()
        .zip(b.char_indices())
        .find(|((_, x), (_, y))| x != y)
        .map(|((x, _), _)| x)
        .unwrap_or_else(|| std::cmp::min(a.len(), b.len()));
    &a[..len]
}

/// Lock-protected portion of `Repl`
struct ReplInner {
    /// Input file. Usually wraps stdin.
    file: File,

    /// Command history. Most recent command last.
    history: Vec<String>,

    /// When we start scrolling through history, the command buffer we left behind acts like it's at
    /// the end of the history list. It lives here during those times.
    original_cmd_buf: Option<Vec<char>>,

    /// Position when we are scrolling through the command history. Represents an index from the
    /// *end* of the `history` vec.
    history_pos: usize,

    /// The command currently being typed by the user. A Vec of chars makes it easier to describe
    /// where the cursor is (strings index by bytes not chars).
    cmd_buf: Vec<char>,

    /// Commands that were typed in faster than we could return them. Subsequent calls to `get_cmd`
    /// will return commands from this queue rather than issuing a new prompt until it is exhausted.
    cmd_overflow: VecDeque<String>,

    /// Where the cursor is. An index of `cmd_buf`. May equal, but not exceed, `cmd_buf.len()`.
    cursor_pos: usize,

    /// The prompt string. This is set to `Some(..)` when we call `get_cmd` and set to `None` when
    /// we have accepted a command to return.
    prompt: Option<String>,

    /// Length of the last line of output. We track this so that we can insert a newline after the
    /// output regardless of whether it has one, but also resume from the end of that line when
    /// further output is submitted.
    last_line_len: usize,

    /// Terminal size. TODO(https://fxbug.dev/331656990): This should update on SIGWINCH
    terminal_size: (u16, u16),

    /// Character width of the prompt, and the value of cursor_pos at the last time of repaint.
    cursor_pos_info: Cell<Option<(usize, usize)>>,
}

impl ReplInner {
    /// Clear and rewrite the input line, updating the contents of the current command and the
    /// cursor position.
    fn repaint(&self) {
        let mut stdout = stdout();
        if let Some(prompt) = self.prompt.as_ref() {
            if let Some((prompt_len, last_cursor_pos)) = self.cursor_pos_info.get() {
                let cursor_line = (prompt_len + last_cursor_pos) / (self.terminal_size.0 as usize);
                let _ = stdout.queue(MoveUp(cursor_line.try_into().expect("command too large!")));
                print!("\r\x1b[J{prompt} {}", String::from_iter(self.cmd_buf.iter()));
                self.cursor_pos_info.set(Some((prompt_len, self.cursor_pos)));
            } else {
                print!("\r\x1b[J{prompt} ");
                let _ = stdout.flush();
                if let Ok((col, _)) = crossterm::cursor::position() {
                    self.cursor_pos_info.set(Some((col as usize, self.cursor_pos)));
                }
                print!("{}", String::from_iter(self.cmd_buf.iter()));
            };

            let (prompt_size, _) = self.cursor_pos_info.get().unwrap();
            if self.cmd_buf.len() != self.cursor_pos {
                let eol_pos = prompt_size + self.cmd_buf.len();
                let cursor_pos = prompt_size + self.cursor_pos;
                let eol_line = eol_pos / (self.terminal_size.0 as usize);
                let eol_col = eol_pos % (self.terminal_size.0 as usize);
                let cursor_line = cursor_pos / (self.terminal_size.0 as usize);
                let cursor_col = cursor_pos % (self.terminal_size.0 as usize);
                let move_up = eol_line - cursor_line;
                let _ = stdout.queue(MoveUp(move_up.try_into().expect("command too large!")));
                if eol_col > cursor_col {
                    let move_left = eol_col - cursor_col;
                    let _ =
                        stdout.queue(MoveLeft(move_left.try_into().expect("command too large!")));
                } else if eol_col < cursor_col {
                    let move_right = cursor_col - eol_col;
                    let _ =
                        stdout.queue(MoveRight(move_right.try_into().expect("command too large!")));
                }
            } else if (prompt_size + self.cursor_pos) % (self.terminal_size.0 as usize) == 0 {
                print!("\r\n");
            }
        }
        let _ = stdout.flush();
    }

    /// Print some output. Suspends the input prompt if active so that output flows above the prompt
    /// without disrupting it.
    pub async fn output(&mut self, output: &str) {
        let mut chars_before = 0;
        if self.last_line_len > 0 {
            chars_before = self.last_line_len;
            print!("\r\x1b[A\x1b[{}C\x1b[K", self.last_line_len);
        } else {
            print!("\r\x1b[K");
        }

        let (output, ends_in_newline) = if let Some(no_nl) = output.strip_suffix("\n") {
            (no_nl, true)
        } else {
            (output, false)
        };

        for line in output.split('\n') {
            self.last_line_len = printed_length(line) + chars_before;
            chars_before = 0;
            print!("{line}\r\n");
        }

        if ends_in_newline {
            self.last_line_len = 0;
        }

        self.repaint();
    }

    /// Add a command to the history list. Filters out empty commands and repeats.
    fn add_to_history(&mut self, cmd: &String) {
        if !cmd.is_empty() && self.history.last() != Some(cmd) {
            self.history.push(cmd.clone());
        }
    }

    /// Replace the current `cmd_buf` with the current history item.
    fn load_cmd_from_history(&mut self) {
        if self.history_pos == 0 {
            self.cmd_buf = self.original_cmd_buf.take().unwrap();
        } else {
            let idx = self.history.len() - self.history_pos;
            self.cmd_buf = self.history[idx].chars().collect();
        }
        self.cursor_pos = self.cmd_buf.len();
    }

    /// Consume the command being typed and reset the buffer state to start the next one.
    fn take_cmd(&mut self) -> String {
        let got = String::from_iter(std::mem::replace(&mut self.cmd_buf, Vec::new()).into_iter());
        self.cursor_pos = 0;
        self.history_pos = 0;
        self.original_cmd_buf = None;
        got
    }
}

/// An asynchronous repl. This takes control of the output TTY and allows us to display a prompt and
/// also simultaneously output data. The data flows above the prompt without disrupting it.
pub struct Repl {
    /// Lock-protected fields.
    inner: StrictMutex<ReplInner>,

    /// When this is nonzero, we do not sleep waiting for IO when we are at a prompt. Usually that
    /// means we're trying to display some output, and we need the prompt code to stop sitting on
    /// the lock.
    unblocked: AtomicUsize,

    /// Wakers that are set every time `unblocked` becomes nonzero, so the effects thereof can be
    /// applied immediately.
    wait_unblocked: AtomicWaker,
}

impl Repl {
    /// Create a new Repl. At the moment this squats on stdin and assumes stdin is a TTY.
    pub fn new() -> io::Result<Self> {
        let fd = stdin().as_raw_fd();
        enable_raw_mode().map_err(|x| match x {
            crossterm::ErrorKind::IoError(e) => e,
            other => panic!("Unexpected crossterm error {other:?}"),
        })?;

        let terminal_size = crossterm::terminal::size().map_err(|x| match x {
            crossterm::ErrorKind::IoError(e) => e,
            other => panic!("Unexpected crossterm error {other:?}"),
        })?;

        // SAFETY: We just obtained this file from stdout(), which should guarantee its validity.
        let owned_fd = unsafe { std::os::unix::io::OwnedFd::from_raw_fd(fd) };
        let file = File::from(owned_fd);

        Ok(Repl {
            inner: StrictMutex::new(ReplInner {
                file,
                cmd_buf: Vec::new(),
                cmd_overflow: VecDeque::new(),
                cursor_pos: 0,
                history: Vec::new(),
                original_cmd_buf: None,
                history_pos: 0,
                prompt: None,
                last_line_len: 0,
                terminal_size,
                cursor_pos_info: Cell::new(None),
            }),
            unblocked: AtomicUsize::new(0),
            wait_unblocked: AtomicWaker::new(),
        })
    }

    /// Retrieve a command. Displays the given prompt and waits for input. Supports basic editing
    /// keys.
    pub async fn get_cmd<F: Future<Output = Vec<(String, usize)>>>(
        &self,
        prompt: &str,
        completer: impl Fn(String, usize) -> F,
    ) -> io::Result<Option<String>> {
        const UP_ARROW: &[u8] = b"\x1b[A";
        const DOWN_ARROW: &[u8] = b"\x1b[B";
        const RIGHT_ARROW: &[u8] = b"\x1b[C";
        const LEFT_ARROW: &[u8] = b"\x1b[D";
        const HOME: &[u8] = b"\x1b[H";
        const END: &[u8] = b"\x1b[F";
        const PGUP: &[u8] = b"\x1b[5~";
        const PGDN: &[u8] = b"\x1b[6~";
        let mut buf = [0; 4];
        let mut ret = None;
        let mut tab_time: Option<Instant> = None;

        {
            let mut inner = self.inner.lock().await;
            inner.prompt = Some(prompt.to_owned());
            if let Some(stored) = inner.cmd_overflow.pop_front() {
                inner.add_to_history(&stored);
                return Ok(Some(stored));
            }

            inner.repaint();
        }

        'outer: while ret.is_none() {
            let mut inner = self.inner.lock().await;

            let wait_unblocked = poll_fn(|ctx| {
                self.wait_unblocked.register(ctx.waker());

                if self.unblocked.load(Ordering::Acquire) != 0 {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            });

            let size = match select(inner.file.read(&mut buf), wait_unblocked).await {
                Either::Left((size, _)) => size?,
                Either::Right((_, read)) => {
                    if let Some(size) = read.now_or_never() {
                        size?
                    } else {
                        continue 'outer;
                    }
                }
            };

            if size == 0 {
                inner.prompt = None;
                return Ok(None);
            }

            let mut buf = &buf[..size];
            fn try_strip_prefix(buf: &mut &[u8], prefix: &[u8]) -> bool {
                if let Some(rest) = buf.strip_prefix(prefix) {
                    *buf = rest;
                    true
                } else {
                    false
                }
            }

            while buf.len() > 0 {
                let mut set_tab_time = None;
                if try_strip_prefix(&mut buf, UP_ARROW) {
                    if inner.history_pos == inner.history.len() {
                        continue;
                    }

                    if inner.history_pos == 0 {
                        inner.original_cmd_buf = Some(inner.cmd_buf.split_off(0));
                    }

                    inner.history_pos += 1;
                    inner.load_cmd_from_history();
                } else if try_strip_prefix(&mut buf, DOWN_ARROW) {
                    if inner.history_pos == 0 {
                        continue;
                    }

                    inner.history_pos -= 1;
                    inner.load_cmd_from_history();
                } else if try_strip_prefix(&mut buf, RIGHT_ARROW) {
                    if inner.cursor_pos < inner.cmd_buf.len() {
                        inner.cursor_pos += 1;
                    }
                } else if try_strip_prefix(&mut buf, LEFT_ARROW) {
                    if inner.cursor_pos > 0 {
                        inner.cursor_pos -= 1;
                    }
                } else if try_strip_prefix(&mut buf, HOME) {
                    inner.cursor_pos = 0;
                } else if try_strip_prefix(&mut buf, END) {
                    inner.cursor_pos = inner.cmd_buf.len();
                } else if try_strip_prefix(&mut buf, PGUP) {
                    // Ignore
                } else if try_strip_prefix(&mut buf, PGDN) {
                    // Ignore
                } else {
                    let byte = buf[0];
                    buf = &buf[1..];

                    if byte == 0x03 {
                        // Control-C
                        inner.cursor_pos = inner.cmd_buf.len();
                        inner.repaint();
                        print!("^C\r\n");
                        inner.cursor_pos = 0;
                        inner.cmd_buf.clear();
                    } else if byte == 0x04 {
                        inner.cursor_pos = inner.cmd_buf.len();
                        inner.repaint();
                        // Control-D
                        print!("^D\r\n");
                        inner.cursor_pos = 0;
                        inner.cmd_buf.clear();
                        break 'outer;
                    } else if byte == 0x7f {
                        if inner.cursor_pos != 0 {
                            inner.cursor_pos -= 1;
                            let pos = inner.cursor_pos;
                            inner.cmd_buf.remove(pos);
                        }
                    } else if byte == b'\r' {
                        if ret.is_none() {
                            inner.cursor_pos = inner.cmd_buf.len();
                            inner.repaint();
                            print!("\r\n");
                            let _ = stdout().flush();
                            let got = inner.take_cmd();
                            inner.add_to_history(&got);
                            ret = Some(got);
                            inner.prompt = None;
                        } else {
                            let got = inner.take_cmd();
                            inner.cmd_overflow.push_back(got);
                        }
                    } else if byte == b'\t' {
                        let got = String::from_iter(inner.cmd_buf.iter().copied());
                        let pos = got
                            .char_indices()
                            .skip(inner.cursor_pos)
                            .next()
                            .map(|x| x.0)
                            .unwrap_or(got.len());
                        let completions = completer(got.clone(), pos).await;
                        let double_tab = tab_time
                            .take()
                            .map(|x| Instant::now().duration_since(x) <= DOUBLE_TAB_TIME)
                            .unwrap_or(false);

                        if double_tab && completions.len() > 1 {
                            print!(
                                "\r\n{}\r\n",
                                completions
                                    .into_iter()
                                    .map(|x| x.0.trim().escape_default().to_string())
                                    .collect::<Vec<_>>()
                                    .join("  ")
                            );
                        } else {
                            set_tab_time = Some(Instant::now());

                            let singular = completions
                                .into_iter()
                                .reduce(|prev, e| {
                                    if e.1 != prev.1 {
                                        (String::new(), 0)
                                    } else {
                                        (common_prefix(&e.0, &prev.0).to_owned(), prev.1)
                                    }
                                })
                                .filter(|x| !x.0.is_empty());

                            if let Some((completion, start)) = singular {
                                let start = got[..start].chars().count();
                                let cursor_pos = inner.cursor_pos;
                                assert!(start <= cursor_pos, "Completion starts after cursor!");
                                inner.cmd_buf.splice(start..cursor_pos, completion.chars());
                                inner.cursor_pos = start + completion.chars().count();
                            }
                        }
                    } else if byte <= 0x7f {
                        let ch = std::str::from_utf8(&[byte]).unwrap().chars().next().unwrap();

                        if !ch.is_control() {
                            let pos = inner.cursor_pos;
                            inner.cmd_buf.insert(pos, ch);
                            inner.cursor_pos += 1;
                        }
                    }
                }

                tab_time = set_tab_time;
            }
            inner.repaint();
        }

        Ok(ret)
    }
}

impl Drop for Repl {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Implementation of AsyncWrite that outputs to a repl.
pub struct ReplWriter<'a> {
    repl: &'a Repl,
    fut: Option<LocalBoxFuture<'a, usize>>,
}

impl<'a> ReplWriter<'a> {
    /// Create a new ReplWriter for the given Repl.
    pub fn new(repl: &'a Repl) -> Self {
        ReplWriter { repl, fut: None }
    }
}

impl<'a> AsyncWrite for ReplWriter<'a> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        ctx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            if let Some(mut fut) = self.fut.take() {
                return if let Poll::Ready(len) = fut.poll_unpin(ctx) {
                    Poll::Ready(Ok(len))
                } else {
                    self.fut = Some(fut);
                    Poll::Pending
                };
            } else {
                let len = buf.len();
                let data = String::from_utf8_lossy(buf).into_owned();
                let repl = self.repl;
                self.fut = Some(
                    async move {
                        let lock_fut = repl.inner.lock();
                        repl.unblocked.fetch_add(1, Ordering::Acquire);
                        repl.wait_unblocked.wake();
                        let mut inner = lock_fut.await;
                        repl.unblocked.fetch_sub(1, Ordering::Release);
                        inner.output(&data).await;
                        len
                    }
                    .boxed_local(),
                );
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _ctx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _ctx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn common_prefix_test() {
        assert_eq!("abc", common_prefix("abcd", "abcf"));
    }

    #[test]
    fn common_prefix_test_one_empty() {
        assert_eq!("", common_prefix("abcd", ""));
    }

    #[test]
    fn common_prefix_test_two_empty() {
        assert_eq!("", common_prefix("", ""));
    }

    #[test]
    fn common_prefix_test_all() {
        assert_eq!("abc", common_prefix("abc", "abc"));
    }

    #[test]
    fn common_prefix_test_none() {
        assert_eq!("", common_prefix("abc", "def"));
    }
}
