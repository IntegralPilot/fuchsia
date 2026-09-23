// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Structured user interface (SUI).
//!
//! Provides a wrapper around a Text UI (TUI) to support terminal, GUI, and
//! machine wrappers.
//!
//! Note: this is being developed within pbms as a proof of concept. The intent
//!       is to move this code when it's further along. Potentially using it for
//!       all ffx UI.

use fuchsia_sync::Mutex;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, IsTerminal, Read, Write, stdout};
use thiserror::Error;
use unicode_segmentation::UnicodeSegmentation;

// An ANSI escape sequence to clear from the cursor position to the end of the
// screen.
// See the "Erase in Display" table entry in
// https://en.wikipedia.org/wiki/ANSI_escape_code for details.
const CLEAR_TO_END_OF_SCREEN: &'static str = "\x1b[J";

/// Error type returned by the structured user interface (SUI) library.
#[derive(Error, Debug)]
pub enum StructuredUiError {
    /// An error encountered during terminal write or escape sequence operations.
    #[error("Terminal I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// An error encountered while reading interactive prompts from stdin.
    #[error("Failed to read prompt input: {0}")]
    PromptInput(#[source] std::io::Error),
}

pub type Result<T> = std::result::Result<T, StructuredUiError>;

/// Move the terminal cursor 'up' and clear N rows.
fn clear_rows<W: ?Sized>(output: &mut W, rows: usize) -> Result<()>
where
    W: Write + Send + Sync,
{
    write!(output, "\x1b[{}A{}", rows, CLEAR_TO_END_OF_SCREEN)?;
    Ok(())
}

/// Return the ratio of progress over total step in a user readable format.
fn progress_percentage(at_: u64, of_: u64) -> f32 {
    if of_ == 0 {
        return 100.0;
    }
    at_ as f32 / of_ as f32 * 100.0
}

/// A single topic of progress, which can be nested within other topics.
/// E.g. coping file 3 of 100, and being on byte 2000 of 9000 within that file
/// is two ProgressEntry records.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ProgressEntry {
    /// The current task description.
    name: String,

    /// How far along the progress is, as compared to `of`.
    at: u64,

    /// The point at which `at` is 100% complete. E.g. "at 50 of 100 steps".
    of: u64,

    /// What is represented by `at` and `of`. E.g. "bytes", "seconds", "steps".
    units: String,
}

/// A wrapper around one or more ProgressEntry records.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Progress {
    /// Always "progress".
    kind: String,

    /// A overall description. E.g. "Copying files".
    title: String,

    /// An ordered list of progress entries.
    entries: Vec<ProgressEntry>,
}

impl Progress {
    pub fn builder() -> Self {
        Progress { kind: "progress".to_string(), ..Default::default() }
    }

    /// A label shown prominently, such as the dialog or window title in a GUI.
    pub fn title<'a>(&'a mut self, title: &'a str) -> &'a mut Self {
        self.title = title.to_string();
        self
    }

    /// Push another `ProgressEntry` to show nested progress.
    pub fn entry<'a>(
        &'a mut self,
        name: &'a str,
        at: u64,
        of: u64,
        units: &'a str,
    ) -> &'a mut Self {
        let entry = ProgressEntry { name: name.to_string(), at, of, units: units.to_string() };
        self.entries.push(entry);
        self
    }
}

/// A basic presentation used for alerts and short prompts for data.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct SimplePresentation {
    /// One of "string_prompt" or "alert".
    kind: String,

    /// A overall description. E.g. "Copying files".
    title: Option<String>,

    /// The body of the message to the user.
    message: Option<String>,

    /// The specific question or call-to-action to the user.
    prompt: String,
}

impl SimplePresentation {
    pub fn builder() -> Self {
        SimplePresentation { kind: "string_prompt".to_string(), ..Default::default() }
    }

    /// A label shown prominently, such as the dialog or window title in a GUI.
    pub fn title<'a, S>(&'a mut self, title: S) -> &'a mut Self
    where
        S: Into<String>,
    {
        self.title = Some(title.into());
        self
    }

    pub fn message<'a, S>(&'a mut self, message: S) -> &'a mut Self
    where
        S: Into<String>,
    {
        self.message = Some(message.into());
        self
    }

    pub fn prompt<'a>(&'a mut self, prompt: &'a str) -> &'a mut Self {
        self.prompt = prompt.to_string();
        self
    }
}

/// A single horizontal row of a table.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RowPresentation {
    /// If the row is used in a menu or selectable list, the `id` is used to
    /// identify which selection was made. Note: if the `id` is not unique, it
    /// may be difficult to know which selection was made.
    id: Option<String>,

    /// The entries that make up the row.
    columns: Vec<String>,
}

/// A menu or table of one or more rows.
///
/// It's advisable, though not required, to set the `header` and each of the
/// `rows` with the same number of columns.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TableRows {
    /// Always "table_rows".
    kind: String,

    /// A short label presented at the top (or prominently). In a GUI, often the
    /// title of the tab, dialog, or window displaying the table.
    title: Option<String>,

    /// Appears above the table. Often descriptive text.
    /// Compare to `note`.
    message: Option<String>,

    /// The header is the top row of the table. It's commonly used to label the
    /// columns in the table and normally doesn't contain data itself.
    header: RowPresentation,

    /// Rows are the body of the table. This is where the actual data in the
    /// table is presented.
    rows: Vec<RowPresentation>,

    /// Appears below the table. Often a list and description of special
    /// symbols used in the table. Common examples are asterisk,
    /// double-asterisk, dagger, etc.
    /// Compare to 'message'.
    note: Option<String>,

    /// The `id` of the `rows` entry which is selected by default.
    default: Option<String>,

    /// Often a question for the user, especially if the table shows a menu of
    /// options to choose from.
    prompt: Option<String>,

    /// The highest number of columns in any of the rows/header for this table.
    max_columns: usize,
}

impl TableRows {
    pub fn builder() -> Self {
        TableRows::default()
    }

    /// Add a title.
    pub fn title<'a, S>(&'a mut self, title: S) -> &'a mut Self
    where
        S: Into<String>,
    {
        self.title = Some(title.into());
        self
    }

    /// Add a header to the top of the table.
    ///
    /// It's advisable, though not required, to set the `header` and each `row`
    /// with the same number of columns.
    pub fn header<'a, S>(&'a mut self, columns: Vec<S>) -> &'a mut Self
    where
        S: AsRef<str>,
    {
        let columns = columns.iter().map(|s| s.as_ref().to_string()).collect::<Vec<String>>();
        self.max_columns = std::cmp::max(self.max_columns, columns.len());
        self.header = RowPresentation { id: None, columns };
        self
    }

    /// Append a row.
    ///
    /// This call may be repeated. The rows will be displayed in the order they
    /// are added with this call.
    ///
    /// It's advisable, though not required, to set the `header` and each `row`
    /// with the same number of columns.
    pub fn row<'a, S>(&'a mut self, columns: Vec<S>) -> &'a mut Self
    where
        S: AsRef<str>,
    {
        let columns = columns.iter().map(|s| s.as_ref().to_string()).collect::<Vec<String>>();
        self.max_columns = std::cmp::max(self.max_columns, columns.len());
        self.rows.push(RowPresentation { id: None, columns });
        self
    }

    /// Append a row with an id value.
    ///
    /// The `id` should be unique and provides a way to refer to a row.
    ///
    /// This call may be repeated. The rows will be displayed in the order they
    /// are added with this call.
    ///
    /// It's advisable, though not required, to set the `header` and each `row`
    /// with the same number of columns.
    pub fn row_with_id<'a, S>(&'a mut self, id: S, columns: Vec<S>) -> &'a mut Self
    where
        S: AsRef<str>,
    {
        let columns = columns.iter().map(|s| s.as_ref().to_string()).collect::<Vec<String>>();
        self.rows.push(RowPresentation { id: Some(id.as_ref().to_string()), columns });
        self
    }

    /// Add a note.
    pub fn note<'a, S>(&'a mut self, note: S) -> &'a mut Self
    where
        S: Into<String>,
    {
        self.note = Some(note.into());
        self
    }
}

/// A message used for informing the user of important information.
///
/// A notice differs from an alert in that a notice does not ask for
/// acknowledgement. Instead the notice is presented until the action or state
/// described by the notice completes.
///
/// This is similar to a progress where the progress is unknown (like spinner).
/// The notice is shown until the user cancels the action or the action
/// completes.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Notice {
    /// Always "notice".
    kind: String,

    /// A overall description. E.g. "Copying files".
    title: Option<String>,

    /// The body of the message to the user.
    message: Option<String>,
}

impl Notice {
    pub fn builder() -> Self {
        Notice { kind: "notice".to_string(), ..Default::default() }
    }

    /// A label shown prominently, such as the dialog or window title in a GUI.
    pub fn title<'a, S>(&'a mut self, title: S) -> &'a mut Self
    where
        S: Into<String>,
    {
        self.title = Some(title.into());
        self
    }

    pub fn message<'a, S>(&'a mut self, message: S) -> &'a mut Self
    where
        S: Into<String>,
    {
        self.message = Some(message.into());
        self
    }

    pub fn get_title(&self) -> Option<String> {
        self.title.clone()
    }
    pub fn get_message(&self) -> Option<String> {
        self.message.clone()
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub enum Presentation {
    Notice(Notice),
    Progress(Progress),
    StringPrompt(SimplePresentation),
    Table(TableRows),
}

/// User response to a request.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Response {
    /// The default action was chosen, i.e. pressed "Enter" or "Return".
    Default,

    /// Leave/close without making a choice, i.e. "Cancel" or "Esc".
    NoChoice,

    /// One of the choice keys passed into the presentation.
    Choice(String),

    /// All further steps are to be skipped (aka abort or terminate).
    Quit,
}

pub trait Interface {
    fn present(&self, output: &Presentation) -> Result<Response>;
}

/// A text based UI, likely a terminal.
pub struct InnerTextUi<'a> {
    /// E.g. stdin.
    #[allow(unused)]
    input: &'a mut (dyn Read + Send + Sync + 'a),

    /// E.g. stdout.
    output: &'a mut (dyn Write + Send + Sync + 'a),

    /// E.g. stderr.
    #[allow(unused)]
    error_output: &'a mut (dyn Write + 'a),

    /// Some text UI overwrites itself at each iteration other than the first.
    /// Track how many lines to overwrite.
    overwrite_line_count: usize,

    /// Whether the output device is a TTY.
    is_tty: bool,
}

pub struct TextUi<'a> {
    inner: Mutex<InnerTextUi<'a>>,
}

impl<'a> TextUi<'a> {
    pub fn new<R, W, E>(input: &'a mut R, output: &'a mut W, error_output: &'a mut E) -> Self
    where
        R: Read + Send + Sync + 'a,
        W: Write + Send + Sync + 'a,
        E: Write + 'a,
    {
        Self {
            inner: Mutex::new(InnerTextUi {
                input,
                output,
                error_output,
                overwrite_line_count: 0,
                is_tty: stdout().is_terminal(),
            }),
        }
    }

    /// Alternate constructor for tests allowing TTY to be mocked.
    ///
    /// This allows features such as progress bars and prompts to behave
    /// consistently when tests are run locally/interactively versus in
    /// infrastructure.
    pub fn new_for_test<R, W, E>(
        input: &'a mut R,
        output: &'a mut W,
        error_output: &'a mut E,
        is_tty: bool,
    ) -> Self
    where
        R: Read + Send + Sync + 'a,
        W: Write + Send + Sync + 'a,
        E: Write + 'a,
    {
        Self {
            inner: Mutex::new(InnerTextUi {
                input,
                output,
                error_output,
                overwrite_line_count: 0,
                is_tty,
            }),
        }
    }

    /// Clears the current progress text.
    ///
    /// This can be used before printing to append output that appears directly
    /// above the next `Progress` presentation update.
    /// Otherwise, the next `Progress` presentation will overwrite printed lines
    /// when attempting to clobber the previous progress element.
    pub fn clear_progress(&self) -> Result<()> {
        let mut inner = self.inner.lock();
        // We only clear the progress presentation if it's going to a TTY
        // terminal, since the shell control sequences don't make sense
        // otherwise.
        if !inner.is_tty {
            return Ok(());
        }
        // Move back to clear the previous presentation.
        let lines_to_overwrite = inner.overwrite_line_count;
        if lines_to_overwrite > 0 {
            clear_rows(inner.output, lines_to_overwrite)?;
        }
        inner.overwrite_line_count = 0;
        Ok(())
    }

    fn present_progress(&self, progress: &Progress) -> Result<Response> {
        // Move back to overwrite the previous progress rendering.
        self.clear_progress()?;

        let mut inner = self.inner.lock();
        // We only print the progress text if it's going to a TTY terminal,
        // since the shell control sequences don't make sense otherwise.
        if !inner.is_tty {
            return Ok(Response::Default);
        }
        write!(inner.output, "Progress for \"{}\"\n", progress.title)?;
        inner.overwrite_line_count += 1;
        let term_width = termion::terminal_size().unwrap_or((80, 40)).0 as usize;
        const MARGINS: usize = /*indent=*/ 2 + /*right_side=*/ 1;
        let limit = term_width.saturating_sub(MARGINS);
        for entry in &progress.entries {
            write!(inner.output, "  {}\n", ellipsis(&entry.name, limit, Some('/')),)?;
            write!(
                inner.output,
                "    {} of {} {} ({:.2}%)\n",
                entry.at,
                entry.of,
                entry.units,
                progress_percentage(entry.at, entry.of),
            )?;
            inner.overwrite_line_count += 2;
        }
        Ok(Response::Default)
    }

    fn present_notice(&self, element: &Notice) -> Result<Response> {
        let mut inner = self.inner.lock();
        if let Some(title) = &element.title {
            writeln!(inner.output, "{}", title)?;
        }
        if let Some(message) = &element.message {
            writeln!(inner.output, "{}", message)?;
        }
        Ok(Response::Default)
    }

    fn present_string_prompt(&self, element: &SimplePresentation) -> Result<Response> {
        let mut inner = self.inner.lock();
        // If the terminal is non-interactive, it's not reasonable to prompt
        // the user.
        if !inner.is_tty {
            return Ok(Response::NoChoice);
        }
        if let Some(title) = &element.title {
            writeln!(inner.output, "{}", title)?;
        }
        if let Some(message) = &element.message {
            writeln!(inner.output, "{}", message)?;
        }
        writeln!(inner.output, "{}: ", element.prompt)?;
        let mut buf_reader = BufReader::new(&mut inner.input);
        let mut choice = String::new();
        buf_reader.read_line(&mut choice).map_err(StructuredUiError::PromptInput)?;
        if choice.is_empty() { Ok(Response::Default) } else { Ok(Response::Choice(choice)) }
    }

    fn present_table(&self, table: &TableRows) -> Result<Response> {
        let mut inner = self.inner.lock();
        if let Some(title) = &table.title {
            writeln!(inner.output, "{}", title)?;
        }
        if let Some(message) = &table.message {
            inner.output.write_all(message.as_bytes())?;
        }
        let mut max_lengths = vec![0; table.max_columns];
        for (index, column) in table.header.columns.iter().enumerate() {
            max_lengths[index] = std::cmp::max(0, column.len());
        }
        for row in &table.rows {
            for (index, column) in row.columns.iter().enumerate() {
                max_lengths[index] = std::cmp::max(max_lengths[index], column.len());
            }
        }
        for row in &table.rows {
            for (index, column) in row.columns.iter().enumerate() {
                write!(inner.output, "{:width$} ", column, width = max_lengths[index])?;
            }
            writeln!(inner.output, "")?;
        }
        if let Some(note) = &table.note {
            inner.output.write_all(note.as_bytes())?;
        }
        Ok(Response::Default)
    }
}

impl<'a> Interface for TextUi<'a> {
    fn present(&self, presentation: &Presentation) -> Result<Response> {
        match presentation {
            Presentation::Notice(p) => self.present_notice(p),
            Presentation::Progress(p) => self.present_progress(p),
            Presentation::StringPrompt(p) => self.present_string_prompt(p),
            Presentation::Table(p) => self.present_table(p),
        }
    }
}

/// If the string is longer than `limit`, ellipsis the string in the middle so
/// that the overall len is `limit` in length.
fn ellipsis(s: &str, limit: usize, prefer: Option<char>) -> String {
    // UX has determined that this should be Chicago manual style "..." (without
    // extra spaces) rather than MLA style "[...]".
    const ELLIPSE: &str = "...";
    // Optimization: if the byte length is less than limit, it's very unlikely
    // that the grapheme count will be larger. (I think that's not possible.)
    // i.e. there would need to be a single byte utf8 which is rendered in
    // multiple fixed-width font cells.
    if s.len() <= limit {
        return s.to_string();
    }
    let total = s.graphemes(/*is_extended=*/ true).count();
    if total <= limit {
        return s.to_string();
    }
    if limit < ELLIPSE.len() {
        return ELLIPSE[..limit].to_string();
    }
    // Determine offsets (end of first piece and start of second piece).
    let mut first = (limit - ELLIPSE.len()) / 2;
    if let Some(ch) = prefer {
        if let Some(n) = s[..first].rfind(ch) {
            first = n + 1;
        }
    }
    let mut second = total - (limit - ELLIPSE.len() - first);
    if let Some(ch) = prefer {
        if let Some(n) = s[second..].find(ch) {
            second += n;
        }
    }
    // Build the new string.
    s.graphemes(/*is_extended=*/ true)
        .take(first)
        .chain(ELLIPSE.graphemes(/*is_extended=*/ true))
        .chain(s.graphemes(true).skip(second).take(total))
        .collect()
}

pub struct MockUi {}

impl MockUi {
    pub fn new() -> Self {
        Self {}
    }

    fn present_notice(&self, _notice: &Notice) -> Result<Response> {
        Ok(Response::Default)
    }

    fn present_progress(&self, _progress: &Progress) -> Result<Response> {
        Ok(Response::Default)
    }

    fn present_string_prompt(&self, _element: &SimplePresentation) -> Result<Response> {
        Ok(Response::Default)
    }

    fn present_table(&self, _table: &TableRows) -> Result<Response> {
        Ok(Response::Default)
    }
}

impl Interface for MockUi {
    fn present(&self, presentation: &Presentation) -> Result<Response> {
        match presentation {
            Presentation::Notice(p) => self.present_notice(p),
            Presentation::Progress(p) => self.present_progress(p),
            Presentation::StringPrompt(p) => self.present_string_prompt(p),
            Presentation::Table(p) => self.present_table(p),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notice() {
        let mut input = "".as_bytes();
        let mut output: Vec<u8> = Vec::new();
        let mut err_out: Vec<u8> = Vec::new();
        let ui = TextUi::new_for_test(&mut input, &mut output, &mut err_out, false);
        let mut notice = Notice::builder();
        notice.title("foo");
        notice.message("Test message for notice.");
        ui.present(&Presentation::Notice(notice)).expect("present notice");
        let output = String::from_utf8(output).expect("string from utf8");
        assert!(output.contains("foo"));
        assert!(output.contains("Test message for notice"));
        assert!(output.contains("notice"));
    }

    #[test]
    fn test_progress() {
        let mut input = "".as_bytes();
        let mut output: Vec<u8> = Vec::new();
        let mut err_out: Vec<u8> = Vec::new();
        let ui = TextUi::new_for_test(&mut input, &mut output, &mut err_out, true);
        let mut progress = Progress::builder();
        progress.title("foo");
        progress.entry("bushel", /*at=*/ 20, /*of=*/ 100, "pieces");
        progress.entry("apple", /*at=*/ 5, /*of=*/ 10, "bites");
        ui.present(&Presentation::Progress(progress)).expect("present progress");
        let output = String::from_utf8(output).expect("string from utf8");
        assert!(output.contains("foo"));
        assert!(output.contains("bushel"));
        assert!(output.contains("apple"));
        assert!(output.contains("pieces"));
        assert!(output.contains("bites"));
    }

    #[test]
    fn test_clear_progress() {
        let present = {
            let mut input = "".as_bytes();
            let mut output: Vec<u8> = Vec::new();
            let mut err_out: Vec<u8> = Vec::new();
            let ui = TextUi::new_for_test(&mut input, &mut output, &mut err_out, true);
            let mut progress = Progress::builder();
            progress.title("foo");
            progress.entry("bushel", /*at=*/ 20, /*of=*/ 100, "pieces");
            progress.entry("apple", /*at=*/ 5, /*of=*/ 10, "bites");
            ui.present(&Presentation::Progress(progress)).expect("present progress");
            String::from_utf8(output).expect("string from utf8")
        };

        let present_and_clear = {
            let mut input = "".as_bytes();
            let mut output: Vec<u8> = Vec::new();
            let mut err_out: Vec<u8> = Vec::new();
            let ui = TextUi::new_for_test(&mut input, &mut output, &mut err_out, true);
            let mut progress = Progress::builder();
            progress.title("foo");
            progress.entry("bushel", /*at=*/ 20, /*of=*/ 100, "pieces");
            progress.entry("apple", /*at=*/ 5, /*of=*/ 10, "bites");
            ui.present(&Presentation::Progress(progress)).expect("present progress");
            ui.clear_progress().expect("clear progress");
            String::from_utf8(output).expect("string from utf8")
        };

        let present_and_clear_and_clear = {
            let mut input = "".as_bytes();
            let mut output: Vec<u8> = Vec::new();
            let mut err_out: Vec<u8> = Vec::new();
            let ui = TextUi::new_for_test(&mut input, &mut output, &mut err_out, true);
            let mut progress = Progress::builder();
            progress.title("foo");
            progress.entry("bushel", /*at=*/ 20, /*of=*/ 100, "pieces");
            progress.entry("apple", /*at=*/ 5, /*of=*/ 10, "bites");
            ui.present(&Presentation::Progress(progress)).expect("present progress");
            ui.clear_progress().expect("clear progress");
            ui.clear_progress().expect("clear progress");
            String::from_utf8(output).expect("string from utf8")
        };

        let present_and_present = {
            let mut input = "".as_bytes();
            let mut output: Vec<u8> = Vec::new();
            let mut err_out: Vec<u8> = Vec::new();
            let ui = TextUi::new_for_test(&mut input, &mut output, &mut err_out, true);
            let mut progress = Progress::builder();
            progress.title("foo");
            progress.entry("bushel", /*at=*/ 20, /*of=*/ 100, "pieces");
            progress.entry("apple", /*at=*/ 5, /*of=*/ 10, "bites");
            ui.present(&Presentation::Progress(progress.clone())).expect("present progress #1");
            ui.present(&Presentation::Progress(progress)).expect("present progress #2");
            String::from_utf8(output).expect("string from utf8")
        };

        let present_and_clear_and_present = {
            let mut input = "".as_bytes();
            let mut output: Vec<u8> = Vec::new();
            let mut err_out: Vec<u8> = Vec::new();
            let ui = TextUi::new_for_test(&mut input, &mut output, &mut err_out, true);
            let mut progress = Progress::builder();
            progress.title("foo");
            progress.entry("bushel", /*at=*/ 20, /*of=*/ 100, "pieces");
            progress.entry("apple", /*at=*/ 5, /*of=*/ 10, "bites");
            ui.present(&Presentation::Progress(progress.clone())).expect("present progress #1");
            ui.clear_progress().expect("clear progress");
            ui.present(&Presentation::Progress(progress)).expect("present progress #2");
            String::from_utf8(output).expect("string from utf8")
        };

        // TextUI::clear_output() should write extra ANSI characters to output.
        assert!(present.len() < present_and_clear.len());

        // A TextUI::clear_output() invoked immediately after
        // TextUI::clear_output() should be noop.
        assert_eq!(present_and_clear, present_and_clear_and_clear);

        // A TextUI::present() invoked immediately after TextUI::present()
        // should write ANSI characters to clear output before the second
        // progress message.
        assert!(present_and_present.starts_with(&present_and_clear));

        // A TextUI::present() invoked after TextUI::present() shouldn't write
        // any extra ANSI characters to clear output if TextUI::clear_output()
        // has already been invoked in between.
        assert_eq!(present_and_present, present_and_clear_and_present);

        // The second TextUI::present() output should be equivalent to
        // TextUI::clear_output() concatenated with the first TextUI::present()
        // output.
        assert_eq!(present_and_present, present_and_clear.clone() + &present);
    }

    #[test]
    fn test_table() {
        let mut input = "".as_bytes();
        let mut output: Vec<u8> = Vec::new();
        let mut err_out: Vec<u8> = Vec::new();
        let ui = TextUi::new_for_test(&mut input, &mut output, &mut err_out, false);
        let mut table = TableRows::builder();
        table.title("foo");
        table.header(vec!["type", "count", "notes"]);
        table.row(vec!["fruit", "5", "orange"]);
        table.row_with_id(/*id=*/ "a", vec!["car", "10", "red"]);
        table.note("bar");
        ui.present(&Presentation::Table(table)).expect("present table");
        let output = String::from_utf8(output).expect("string from utf8");
        println!("{}", output);
        assert!(output.contains("foo"));
        assert!(output.contains("bar"));
        assert!(output.contains("fruit"));
        assert!(output.contains("orange"));
        assert!(output.contains("car"));
        assert!(output.contains("red"));
    }

    #[test]
    fn test_ellipsis() {
        assert_eq!(ellipsis("cake/drought/fins", 100, /*prefer=*/ None), "cake/drought/fins");
        assert_eq!(ellipsis("cake/drought/fins", 17, /*prefer=*/ None), "cake/drought/fins");
        assert_eq!(ellipsis("cake/drought/fins", 16, /*prefer=*/ None), "cake/d...ht/fins");
        assert_eq!(ellipsis("cake/drought/fins", 15, /*prefer=*/ None), "cake/d...t/fins");
        assert_eq!(ellipsis("cake/drought/fins", 14, /*prefer=*/ None), "cake/...t/fins");
        assert_eq!(ellipsis("cake/drought/fins", 12, /*prefer=*/ None), "cake.../fins");
        assert_eq!(ellipsis("cake/drought/fins", 11, /*prefer=*/ None), "cake...fins");
        assert_eq!(ellipsis("cake/drought/fins", 5, /*prefer=*/ None), "c...s");
        assert_eq!(ellipsis("cake/drought/fins", 4, /*prefer=*/ None), "...s");
        assert_eq!(ellipsis("cake/drought/fins", 3, /*prefer=*/ None), "...");
        assert_eq!(ellipsis("cake/drought/fins", 1, /*prefer=*/ None), ".");
        assert_eq!(ellipsis("cake/drought/fins", 0, /*prefer=*/ None), "");

        assert_eq!(ellipsis("cake/drought/fins", 100, Some('/')), "cake/drought/fins");
        assert_eq!(ellipsis("cake/drought/fins", 17, Some('/')), "cake/drought/fins");
        assert_eq!(ellipsis("cake/drought/fins", 16, Some('/')), "cake/.../fins");
        assert_eq!(ellipsis("cake/drought/fins", 15, Some('/')), "cake/.../fins");
        assert_eq!(ellipsis("cake/drought/fins", 14, Some('/')), "cake/.../fins");
        assert_eq!(ellipsis("cake/drought/fins", 12, Some('/')), "cake.../fins");
        assert_eq!(ellipsis("cake/drought/fins", 11, Some('/')), "cake...fins");
        assert_eq!(ellipsis("cake/drought/fins", 5, Some('/')), "c...s");
        assert_eq!(ellipsis("cake/drought/fins", 4, Some('/')), "...s");
        assert_eq!(ellipsis("cake/drought/fins", 3, Some('/')), "...");
        assert_eq!(ellipsis("cake/drought/fins", 1, Some('/')), ".");
        assert_eq!(ellipsis("cake/drought/fins", 0, Some('/')), "");

        assert_eq!(ellipsis("/x/cake/drought_fins", 20, Some('/')), "/x/cake/drought_fins");
        assert_eq!(ellipsis("/x/cake/drought_fins", 19, Some('/')), "/x/cake/...ght_fins");
        assert_eq!(ellipsis("/x/cake/drought_fins", 18, Some('/')), "/x/...drought_fins");
        assert_eq!(ellipsis("/x/cake/drought_fins", 17, Some('/')), "/x/...rought_fins");
        assert_eq!(ellipsis("/x/cake/drought_fins", 16, Some('/')), "/x/...ought_fins");
        assert_eq!(ellipsis("/x/cake/drought_fins", 15, Some('/')), "/x/...ught_fins");
        assert_eq!(ellipsis("/x/cake/drought_fins", 14, Some('/')), "/x/...ght_fins");
        assert_eq!(ellipsis("/x/cake/drought_fins", 13, Some('/')), "/x/...ht_fins");
        assert_eq!(ellipsis("/x/cake/drought_fins", 12, Some('/')), "/x/...t_fins");
        assert_eq!(ellipsis("/x/cake/drought_fins", 11, Some('/')), "/x/..._fins");
    }

    #[test]
    fn test_string_prompt_success() {
        let mut input = "my choice\n".as_bytes();
        let mut output: Vec<u8> = Vec::new();
        let mut err_out: Vec<u8> = Vec::new();
        let ui = TextUi::new_for_test(&mut input, &mut output, &mut err_out, true);
        let mut prompt = SimplePresentation::builder();
        prompt.title("Prompt Title");
        prompt.message("Prompt Message");
        prompt.prompt("Enter choice");

        let res = ui.present(&Presentation::StringPrompt(prompt)).unwrap();
        assert_eq!(res, Response::Choice("my choice\n".to_string()));

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("Prompt Title"));
        assert!(output_str.contains("Prompt Message"));
        assert!(output_str.contains("Enter choice:"));
    }

    struct FailingReader;
    impl std::io::Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::Other, "mock read error"))
        }
    }

    #[test]
    fn test_string_prompt_read_error() {
        let mut input = FailingReader;
        let mut output: Vec<u8> = Vec::new();
        let mut err_out: Vec<u8> = Vec::new();
        let ui = TextUi::new_for_test(&mut input, &mut output, &mut err_out, true);
        let mut prompt = SimplePresentation::builder();
        prompt.prompt("Enter choice");

        let res = ui.present(&Presentation::StringPrompt(prompt));
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), StructuredUiError::PromptInput(_)));
    }
}
