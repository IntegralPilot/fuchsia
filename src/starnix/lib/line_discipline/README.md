# Line Discipline

This crate implements the terminal line discipline logic for Starnix.

## Purpose

The line discipline is responsible for the intermediate processing of characters between the terminal device (e.g., a PTY master or a real serial port) and the reading process (e.g., bash). Its responsibilities include:

*   **Canonical Mode Processing**: Buffering input line-by-line, handling backspace (`\x08` or `\x7f`), line kill (`^U`), etc.
*   **Echoing**: Echoing typed characters back to the output, potentially transforming them (e.g., echoing `^C` for `SIGINT`).
*   **Signal Generation**: Detecting special control characters (like `^C`, `^\`, `^Z`) and generating corresponding signals (`SIGINT`, `SIGQUIT`, `SIGSTOP`).
*   **Output Processing**: Transforming output characters (e.g., converting `\n` to `\r\n`).

## Notes on Flow Control

While the `LineDiscipline` struct maintains `IXON` and `IXOFF` flags, automatic input flow control (throttling) via `IXOFF` (sending `VSTOP`/`VSTART` when the input buffer fills/empties) is **not implemented**. This matches the behavior of Linux PTYs, which only apply throttling logic to hardware serial devices, not pseudo-terminals. PTYs rely on producer-consumer backpressure rather than in-band flow control characters.

## Architecture

This logic was extracted from `starnix_core` to decouple it from the kernel structures and allow for easier testing and potential reusability.

Key components:
*   `LineDiscipline`: The main state struct holding the termios configuration, queues, and cursor state.
*   `Queue`: Manages the flow of data, handling wait buffers (raw data) and read buffers (processed data).
*   `InputBuffer` / `OutputBuffer` traits: Abstractions for the data sources/sinks to avoid direct dependencies on Starnix kernel buffer types.

## Testing

Tests are defined in `lib.rs` and run as part of the `line_discipline_tests` package.
