// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Definition of the Shell Abstract Syntax Tree (AST) and the serialization builder.
//!
//! The AST nodes are designed to be stored in a contiguous, flat byte buffer
//! to support zero-copy serialization and execution. All pointers within the
//! AST are relocatable relative pointers (from the [`crate::relative`] module).

use bstr::BString;
use zerocopy::{FromBytes, IntoBytes};

pub use crate::fd::Fd;
use crate::relative;

/// Discriminant tag indicating the type of a [`WordPart`].
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    zerocopy::IntoBytes,
    zerocopy::FromBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
)]
#[repr(transparent)]
pub struct WordPartTag(pub u8);

impl WordPartTag {
    /// A raw literal string.
    pub const LITERAL: Self = Self(0);
    /// An unquoted variable expansion (e.g. `$VAR`).
    pub const VAR: Self = Self(1);
    /// A single-quoted or escaped literal string.
    pub const QUOTED_LITERAL: Self = Self(2);
    /// A double-quoted variable expansion (e.g. `"$VAR"`).
    pub const QUOTED_VAR: Self = Self(3);
    /// An unquoted command substitution (e.g. `$(echo hello)`).
    pub const COMMAND_SUBSTITUTION: Self = Self(4);
    /// A double-quoted command substitution (e.g. `"$(echo hello)"`).
    pub const QUOTED_COMMAND_SUBSTITUTION: Self = Self(5);
    /// An unquoted arithmetic expansion (e.g. `$((1 + 2))`).
    pub const ARITHMETIC: Self = Self(6);
    /// A double-quoted arithmetic expansion (e.g. `"$((1 + 2))"`).
    pub const QUOTED_ARITHMETIC: Self = Self(7);
}

/// Discriminant tag indicating the type of redirection.
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    zerocopy::IntoBytes,
    zerocopy::FromBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
)]
#[repr(transparent)]
pub struct RedirectTag(pub u8);

impl RedirectTag {
    /// Redirect stdout to a file (e.g. `> file` or `>> file`).
    pub const TO_FILE: Self = Self(0);
    /// Redirect stdin from a file (e.g. `< file`).
    pub const FROM_FILE: Self = Self(1);
    /// Duplicate a file descriptor (e.g. `2>&1`).
    pub const DUP_FD: Self = Self(2);
    /// Close a file descriptor (e.g. `>&-`).
    pub const CLOSE_FD: Self = Self(3);
    pub const HERE_DOC: Self = Self(4);
}

/// Discriminant tag indicating the type of a [`Command`] node.
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    zerocopy::IntoBytes,
    zerocopy::FromBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
)]
#[repr(transparent)]
pub struct CommandTag(pub u8);

impl CommandTag {
    /// A simple command execution (e.g. `ls -l`).
    pub const SIMPLE: Self = Self(0);
    /// A pipeline of commands (e.g. `a | b`).
    pub const PIPELINE: Self = Self(1);
    /// A command with redirections (e.g. `cmd > file`).
    pub const REDIRECT: Self = Self(2);
    /// A subshell execution (e.g. `(cmd)`).
    pub const SUBSHELL: Self = Self(3);
    /// An `if` conditional statement.
    pub const IF: Self = Self(4);
    /// A `while` loop.
    pub const WHILE: Self = Self(5);
    /// An `until` loop.
    pub const UNTIL: Self = Self(6);
    /// A `for` loop.
    pub const FOR: Self = Self(7);
    /// A `case` match statement.
    pub const CASE: Self = Self(8);
    /// A shell function definition (e.g. `f() { ... }`).
    pub const FUNCTION_DEF: Self = Self(9);
    /// A logical AND chain (e.g. `a && b`).
    pub const LOGICAL_AND: Self = Self(10);
    /// A logical OR chain (e.g. `a || b`).
    pub const LOGICAL_OR: Self = Self(11);
    /// A background command execution (e.g. `cmd &`).
    pub const BACKGROUND: Self = Self(12);
    /// A sequence of commands (e.g. `a; b; c`).
    pub const SEQUENCE: Self = Self(13);

    /// Returns `true` if this command type should be formatted and traced during execution
    /// (`set -x` / `set -v`).
    pub const fn is_traceable(&self) -> bool {
        matches!(*self, Self::SIMPLE | Self::PIPELINE | Self::REDIRECT)
    }
}

impl std::fmt::Display for WordPartTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for RedirectTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for CommandTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A component of a shell word (e.g. a literal fragment, a variable, or a command substitution).
#[derive(
    zerocopy::IntoBytes,
    zerocopy::FromBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
)]
#[repr(C)]
pub struct WordPart {
    /// The tag indicating what kind of word part this is.
    pub tag: WordPartTag,
    /// Padding for C representation alignment.
    pub _padding: [u8; 3],
    /// The text associated with literal or variable word parts.
    pub text: relative::BStr,
    /// The command associated with command substitution parts.
    pub command: relative::Ptr<Command>,
}

/// Represents an I/O redirection operation.
#[derive(
    zerocopy::IntoBytes,
    zerocopy::FromBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
)]
#[repr(C)]
pub struct Redirect {
    /// The redirection type tag.
    pub tag: RedirectTag,
    /// Whether to append to the file (e.g. `>>`).
    pub append: u8,
    /// Whether to clobber the file (e.g. `>|`).
    pub clobber: u8,
    /// Whether to expand variables in the here-doc body.
    pub expand: u8,
    /// The source file descriptor (e.g. 2 in `2>&1`, defaults to 1 for output and 0 for input).
    pub src_fd: Fd,
    /// The destination file descriptor (for DUP_FD).
    pub dest_fd: Fd,
    /// The target filename word (for TO_FILE, FROM_FILE).
    pub filename: relative::Slice<WordPart>,
    /// The raw body of a here-document.
    pub body: relative::BStr,
}

/// A transient template used during parsing to accumulate redirection info.
#[derive(Clone, Debug)]
pub struct RedirectTemplate {
    /// The redirection type tag.
    pub tag: RedirectTag,
    /// Whether to append.
    pub append: u8,
    /// Whether to clobber.
    pub clobber: u8,
    /// Whether to expand.
    pub expand: u8,
    /// The source file descriptor.
    pub src_fd: Fd,
    /// The destination file descriptor.
    pub dest_fd: Fd,
    /// The filename span (offset and length in the parser buffer).
    pub filename: Option<relative::Slice<WordPart>>,
    /// The body of the here-doc.
    pub body: Option<BString>,
}

/// Represents a single pattern-action branch in a `case` block.
#[derive(
    zerocopy::IntoBytes,
    zerocopy::FromBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
)]
#[repr(C)]
pub struct CaseItem {
    /// The patterns to match against (each pattern is a word).
    pub patterns: relative::Slice<relative::Slice<WordPart>>,
    /// The command block to execute if matched.
    pub body: relative::Ptr<Command>,
}

/// A node in the serialized AST representing a shell command or control structure.
///
/// Because this is stored in a flat buffer, all references to child commands,
/// arguments, and strings are represented using relocatable relative pointers.
#[derive(
    zerocopy::IntoBytes,
    zerocopy::FromBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
)]
#[repr(C)]
pub struct Command {
    /// The command type tag.
    pub tag: CommandTag,
    /// Alignment padding.
    pub _padding: [u8; 3],

    /// The list of argument words for SIMPLE commands. Each argument is a slice of WordParts.
    pub simple_args: relative::Slice<relative::Slice<WordPart>>,

    /// The left child command (for pipelines, logical operations, subshells, redirects).
    pub left: relative::Ptr<Command>,
    /// The right child command (for pipelines, logical operations).
    pub right: relative::Ptr<Command>,

    /// Redirections associated with this command.
    pub redirects: relative::Slice<Redirect>,

    /// The loop variable name for FOR loops.
    pub for_var: relative::BStr,
    /// The list of items to iterate over for FOR loops.
    pub for_items: relative::Slice<relative::Slice<WordPart>>,

    /// The word matched in a CASE statement.
    pub case_word: relative::Slice<WordPart>,
    /// The list of branches in a CASE statement.
    pub case_items: relative::Slice<CaseItem>,

    /// The name of a function definition.
    pub name: relative::BStr,
    /// The conditional command (for IF, WHILE, UNTIL).
    pub cond: relative::Ptr<Command>,
    /// The main branch/body command (for IF, WHILE, UNTIL, FOR, FUNCTION_DEF).
    pub then_branch: relative::Ptr<Command>,
    /// The else branch command (for IF).
    pub else_branch: relative::Ptr<Command>,

    /// The sequence of commands (for SEQUENCE).
    pub sequence: relative::Slice<relative::Ptr<Command>>,
}

#[derive(
    zerocopy::IntoBytes,
    zerocopy::FromBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
)]
#[repr(C)]
pub struct FlatASTHeader {
    root_cmd: relative::Ptr<Command>,
}

impl FlatASTHeader {
    /// Returns a pointer to the root command in the serialized buffer.
    #[cfg(test)]
    pub const fn root_cmd(&self) -> relative::Ptr<Command> {
        self.root_cmd
    }
}

#[derive(Clone, Debug)]
struct AlignedVec {
    inner: Vec<u64>,
    len: usize,
}

impl AlignedVec {
    fn new() -> Self {
        Self { inner: Vec::new(), len: 0 }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn resize(&mut self, new_len: usize, value: u8) {
        let old_len = self.len;
        let new_u64_cap = (new_len + 7) / 8;
        if new_u64_cap > self.inner.len() {
            self.inner.resize(new_u64_cap, 0);
        }
        self.len = new_len;
        if new_len > old_len {
            self[old_len..new_len].fill(value);
        }
    }

    fn push(&mut self, val: u8) {
        let old_len = self.len;
        self.resize(old_len + 1, 0);
        self[old_len] = val;
    }

    fn extend_from_slice(&mut self, slice: &[u8]) {
        let old_len = self.len;
        self.resize(old_len + slice.len(), 0);
        self[old_len..].copy_from_slice(slice);
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.len = 0;
    }
}

impl std::ops::Deref for AlignedVec {
    type Target = relative::Buffer;
    fn deref(&self) -> &relative::Buffer {
        relative::Buffer::from_bytes(&self.inner.as_slice().as_bytes()[..self.len])
    }
}

impl std::ops::DerefMut for AlignedVec {
    fn deref_mut(&mut self) -> &mut relative::Buffer {
        relative::Buffer::from_bytes_mut(&mut self.inner.as_mut_slice().as_mut_bytes()[..self.len])
    }
}

/// A fully parsed word part containing resolved strings or offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedWordPart {
    /// Literal string.
    Literal(BString),
    /// Variable name.
    Var(BString),
    /// Double-quoted literal.
    QuotedLiteral(BString),
    /// Double-quoted variable name.
    QuotedVar(BString),
    /// Command substitution pointer.
    CommandSubstitution(relative::Ptr<Command>),
    /// Double-quoted command substitution pointer.
    QuotedCommandSubstitution(relative::Ptr<Command>),
    /// Arithmetic expression string.
    Arithmetic(BString),
    /// Double-quoted arithmetic expression string.
    QuotedArithmetic(BString),
}

/// A builder for constructing a serialized flat AST in a contiguous memory buffer.
///
/// # Memory Arrangement
/// The AST is stored in a single byte buffer (`AlignedVec`) to enable zero-copy
/// serialization and deserialization.
///
/// The buffer layout is arranged as follows:
/// 1. **Header**: Begins with a `FlatASTHeader` at offset 0, which contains a
///    [`relative::Ptr<Command>`] pointing to the root command of the AST.
/// 2. **Nodes**: Command nodes, case items, redirections, and word parts are
///    written into the buffer. They reference each other using relative pointers
///    (e.g., child commands via [`relative::Ptr<Command>`], argument list via
///    [`relative::Slice<relative::Slice<WordPart>>`]).
/// 3. **Leaf Data**: Raw strings (e.g. literal text, variable names) are appended
///    as raw byte segments and referenced by [`relative::BStr`].
///
/// # Alignment
/// The builder maintains correct memory alignment for all serialized structs. Before
/// writing any type `T`, the buffer size is aligned to `align_of::<T>()` by appending
/// zero bytes.
///
/// # Resizing and Relocation
/// As the parser processes the input tokens, the AST is constructed incrementally and
/// the `AlignedVec` buffer grows. When the vector capacity is exceeded, the vector
/// reallocates, which moves the buffer to a new address in virtual memory.
///
/// Under standard pointers, this reallocation would require updating (patching) all
/// existing pointers in the buffer to refer to the new addresses.
///
/// Because `ASTBuilder` uses **relative pointers**, no pointer patching is necessary.
/// A relative pointer stores the distance between its own address and its target. Since
/// reallocation moves the entire buffer as a single contiguous block, the distance
/// between any two elements remains constant. This makes the serialization layout completely
/// relocatable and safe against buffer resizing.
///
/// Note that once a reference to any object inside the buffer is obtained (e.g. via
/// [`relative::Ptr::as_ref`]), the Rust borrow checker enforces that the reference's lifetime
/// is tied to the borrow of the buffer. This ensures that the buffer cannot be mutated or
/// reallocated (which would invalidate the reference) until that reference is dropped.
pub struct ASTBuilder {
    buf: AlignedVec,
}

impl ASTBuilder {
    /// Creates a new empty `ASTBuilder`.
    pub fn new() -> Self {
        let mut buf = AlignedVec::new();
        buf.resize(std::mem::size_of::<FlatASTHeader>(), 0);
        Self { buf }
    }

    fn add_uninit<
        T: zerocopy::FromBytes + zerocopy::IntoBytes + zerocopy::Immutable + zerocopy::KnownLayout,
    >(
        &mut self,
    ) -> (&mut T, relative::Ptr<T>) {
        self.align(std::mem::align_of::<T>());
        let offset = self.buf.len();
        self.buf.resize(offset + std::mem::size_of::<T>(), 0);
        let bytes = &mut self.buf[offset..offset + std::mem::size_of::<T>()];
        let val = T::mut_from_bytes(bytes).unwrap();
        (val, relative::Ptr::new(offset))
    }

    fn add_slice_uninit<
        T: zerocopy::FromBytes + zerocopy::IntoBytes + zerocopy::Immutable + zerocopy::KnownLayout,
    >(
        &mut self,
        count: usize,
    ) -> (&mut [T], relative::Slice<T>) {
        self.align(std::mem::align_of::<T>());
        let offset = self.buf.len();
        self.buf.resize(offset + count * std::mem::size_of::<T>(), 0);
        let bytes = &mut self.buf[offset..];
        let (slice, _) = <[T]>::mut_from_prefix_with_elems(bytes, count).unwrap();
        (slice, relative::Slice::new(offset, count))
    }

    pub fn add_command_uninit(
        &mut self,
        tag: CommandTag,
    ) -> (&mut Command, relative::Ptr<Command>) {
        let (cmd, ptr) = self.add_uninit::<Command>();
        cmd.tag = tag;
        (cmd, ptr)
    }

    fn align(&mut self, alignment: usize) {
        while self.buf.len() % alignment != 0 {
            self.buf.push(0);
        }
    }

    /// Appends raw bytes data to the buffer and returns its start offset.
    pub fn add_bytes_data(&mut self, bytes: &[u8]) -> usize {
        let start = self.buf.len();
        self.buf.extend_from_slice(bytes);
        start
    }

    /// Appends raw bytes data and returns a relative `BStr`.
    pub fn add_bstr(&mut self, bytes: &[u8]) -> relative::BStr {
        let start = self.add_bytes_data(bytes);
        relative::BStr::new(start, bytes.len())
    }

    /// Appends a slice of argument pointers based on a list of word slices.
    pub fn add_argument_refs(
        &mut self,
        refs: &[relative::Slice<WordPart>],
    ) -> relative::Slice<relative::Slice<WordPart>> {
        if refs.is_empty() {
            return relative::Slice::empty();
        }
        let (slice, result_slice) = self.add_slice_uninit::<relative::Slice<WordPart>>(refs.len());

        for (i, &item) in refs.iter().enumerate() {
            slice[i] = item;
        }
        result_slice
    }

    pub fn add_command_refs(
        &mut self,
        cmd_ptrs: &[relative::Ptr<Command>],
    ) -> relative::Slice<relative::Ptr<Command>> {
        if cmd_ptrs.is_empty() {
            return relative::Slice::empty();
        }
        let (slice, result_slice) = self.add_slice_uninit::<relative::Ptr<Command>>(cmd_ptrs.len());

        for (i, &ptr) in cmd_ptrs.iter().enumerate() {
            slice[i] = ptr;
        }
        result_slice
    }

    /// Append a serialized AST buffer to this builder and return the root command pointer.
    pub fn import_serialized_ast(&mut self, bytes: &[u8]) -> relative::Ptr<Command> {
        self.buf.clear();
        self.buf.extend_from_slice(bytes);

        let header_size = std::mem::size_of::<FlatASTHeader>();
        let header_bytes = &self.buf[0..header_size];
        let header = FlatASTHeader::ref_from_bytes(header_bytes).unwrap();

        header.root_cmd
    }

    /// Serializes an empty simple command.
    pub fn add_empty_simple_command(&mut self) -> relative::Ptr<Command> {
        let (_, cmd_ptr) = self.add_command_uninit(CommandTag::SIMPLE);
        cmd_ptr
    }

    /// Serializes a simple command node using pre-allocated argument refs.
    pub fn add_simple_command(
        &mut self,
        arg_refs: &[relative::Slice<WordPart>],
    ) -> relative::Ptr<Command> {
        let simple_args = self.add_argument_refs(arg_refs);
        let (cmd_mut, cmd_ptr) = self.add_command_uninit(CommandTag::SIMPLE);
        cmd_mut.simple_args = simple_args;
        cmd_ptr
    }

    /// Serializes a unary command node (e.g. subshell, background execution).
    pub fn add_unary_command(
        &mut self,
        tag: CommandTag,
        sub_ptr: relative::Ptr<Command>,
    ) -> relative::Ptr<Command> {
        let (cmd_mut, cmd_ptr) = self.add_command_uninit(tag);
        cmd_mut.left = sub_ptr;
        cmd_ptr
    }

    /// Serializes a binary command node (e.g. pipeline, logical operations).
    pub fn add_binary_command(
        &mut self,
        tag: CommandTag,
        left_ptr: relative::Ptr<Command>,
        right_ptr: relative::Ptr<Command>,
    ) -> relative::Ptr<Command> {
        let (cmd_mut, cmd_ptr) = self.add_command_uninit(tag);
        cmd_mut.left = left_ptr;
        cmd_mut.right = right_ptr;
        cmd_ptr
    }

    /// Serializes a redirection modifier command node.
    pub fn add_redirect_command(
        &mut self,
        sub_ptr: relative::Ptr<Command>,
        redirects: relative::Slice<Redirect>,
    ) -> relative::Ptr<Command> {
        let (cmd_mut, cmd_ptr) = self.add_command_uninit(CommandTag::REDIRECT);
        cmd_mut.left = sub_ptr;
        cmd_mut.redirects = redirects;
        cmd_ptr
    }

    /// Serializes an `if` conditional command node.
    pub fn add_if_command(
        &mut self,
        cond_ptr: relative::Ptr<Command>,
        then_ptr: relative::Ptr<Command>,
        else_ptr: Option<relative::Ptr<Command>>,
    ) -> relative::Ptr<Command> {
        let (cmd_mut, cmd_ptr) = self.add_command_uninit(CommandTag::IF);
        cmd_mut.cond = cond_ptr;
        cmd_mut.then_branch = then_ptr;
        if let Some(else_p) = else_ptr {
            cmd_mut.else_branch = else_p;
        } else {
            cmd_mut.else_branch.clear();
        }
        cmd_ptr
    }

    /// Serializes a loop command node (WHILE or UNTIL).
    pub fn add_loop_command(
        &mut self,
        tag: CommandTag,
        cond_ptr: relative::Ptr<Command>,
        body_ptr: relative::Ptr<Command>,
    ) -> relative::Ptr<Command> {
        let (cmd_mut, cmd_ptr) = self.add_command_uninit(tag);
        cmd_mut.cond = cond_ptr;
        cmd_mut.then_branch = body_ptr;
        cmd_ptr
    }

    /// Serializes a `for` loop command node.
    pub fn add_for_command(
        &mut self,
        var: relative::BStr,
        items: relative::Slice<relative::Slice<WordPart>>,
        body_ptr: relative::Ptr<Command>,
    ) -> relative::Ptr<Command> {
        let (cmd_mut, cmd_ptr) = self.add_command_uninit(CommandTag::FOR);
        cmd_mut.for_var = var;
        cmd_mut.for_items = items;
        cmd_mut.then_branch = body_ptr;
        cmd_ptr
    }

    /// Serializes a `case` match statement command node.
    pub fn add_case_command(
        &mut self,
        word: relative::Slice<WordPart>,
        case_items: relative::Slice<CaseItem>,
    ) -> relative::Ptr<Command> {
        let (cmd_mut, cmd_ptr) = self.add_command_uninit(CommandTag::CASE);
        cmd_mut.case_word = word;
        cmd_mut.case_items = case_items;
        cmd_ptr
    }

    /// Serializes case pattern-action branches from raw offsets.
    pub fn add_case_items_from_refs(
        &mut self,
        items: &[(relative::Slice<relative::Slice<WordPart>>, relative::Ptr<Command>)],
    ) -> relative::Slice<CaseItem> {
        if items.is_empty() {
            return relative::Slice::empty();
        }
        let (case_items_mut, result_slice) = self.add_slice_uninit::<CaseItem>(items.len());

        for i in 0..items.len() {
            let item_mut = &mut case_items_mut[i];
            item_mut.patterns = items[i].0;
            item_mut.body = items[i].1;
        }
        result_slice
    }

    /// Serializes redirection rules from parser templates.
    pub fn add_redirects_from_templates(
        &mut self,
        templates: &[RedirectTemplate],
    ) -> relative::Slice<Redirect> {
        if templates.is_empty() {
            return relative::Slice::empty();
        }
        let (redirects_mut, result_slice) = self.add_slice_uninit::<Redirect>(templates.len());
        let mut deferred_bodies = Vec::new();

        for (i, template) in templates.iter().enumerate() {
            let red_mut = &mut redirects_mut[i];
            red_mut.tag = template.tag;
            red_mut.append = template.append;
            red_mut.clobber = template.clobber;
            red_mut.expand = template.expand;
            red_mut.src_fd = template.src_fd;
            red_mut.dest_fd = template.dest_fd;

            if let Some(fn_slice) = template.filename {
                red_mut.filename = fn_slice;
            }

            if let Some(body) = &template.body {
                deferred_bodies.push((result_slice.at(i), body.clone()));
            }
        }

        for (red_ptr, body) in deferred_bodies {
            let body_off = self.add_bytes_data(&body);
            let red_mut = self.get_mut(red_ptr);
            red_mut.body.set_offset(body_off, body.len());
        }

        result_slice
    }

    /// Serializes a function definition command node.
    pub fn add_function_def_command(
        &mut self,
        name: relative::BStr,
        body_ptr: relative::Ptr<Command>,
    ) -> relative::Ptr<Command> {
        let (cmd_mut, cmd_ptr) = self.add_command_uninit(CommandTag::FUNCTION_DEF);
        cmd_mut.name = name;
        cmd_mut.then_branch = body_ptr;
        cmd_ptr
    }

    /// Serializes a command sequence node (SEQUENCE).
    pub fn add_sequence_command(
        &mut self,
        cmd_ptrs: &[relative::Ptr<Command>],
    ) -> relative::Ptr<Command> {
        let sequence = self.add_command_refs(cmd_ptrs);
        let (cmd_mut, cmd_ptr) = self.add_command_uninit(CommandTag::SEQUENCE);
        cmd_mut.sequence = sequence;
        cmd_ptr
    }

    /// Helper to serialize a sequence if multiple, or return single offset directly.
    pub fn add_sequence_or_single(
        &mut self,
        cmd_ptrs: &[relative::Ptr<Command>],
    ) -> relative::Ptr<Command> {
        if cmd_ptrs.len() == 1 { cmd_ptrs[0] } else { self.add_sequence_command(cmd_ptrs) }
    }

    /// Serializes a resolved word (array of [`ResolvedWordPart`]).
    pub fn add_resolved_word(&mut self, parts: &[ResolvedWordPart]) -> relative::Slice<WordPart> {
        if parts.is_empty() {
            return relative::Slice::empty();
        }
        let mut gathered = Vec::new();
        for part in parts {
            match part {
                ResolvedWordPart::Literal(s)
                | ResolvedWordPart::Var(s)
                | ResolvedWordPart::QuotedLiteral(s)
                | ResolvedWordPart::QuotedVar(s)
                | ResolvedWordPart::Arithmetic(s)
                | ResolvedWordPart::QuotedArithmetic(s) => {
                    let str_off = self.add_bytes_data(s);
                    gathered.push(str_off);
                }
                ResolvedWordPart::CommandSubstitution(cmd_ptr)
                | ResolvedWordPart::QuotedCommandSubstitution(cmd_ptr) => {
                    gathered.push(cmd_ptr.to_usize());
                }
            }
        }

        let (parts_mut, result_slice) = self.add_slice_uninit::<WordPart>(parts.len());

        for (i, part) in parts.iter().enumerate() {
            let part_mut = &mut parts_mut[i];
            let target_off = gathered[i];
            match part {
                ResolvedWordPart::Literal(s) => {
                    part_mut.tag = WordPartTag::LITERAL;
                    part_mut.text.set_offset(target_off, s.len());
                }
                ResolvedWordPart::Var(s) => {
                    part_mut.tag = WordPartTag::VAR;
                    part_mut.text.set_offset(target_off, s.len());
                }
                ResolvedWordPart::QuotedLiteral(s) => {
                    part_mut.tag = WordPartTag::QUOTED_LITERAL;
                    part_mut.text.set_offset(target_off, s.len());
                }
                ResolvedWordPart::QuotedVar(s) => {
                    part_mut.tag = WordPartTag::QUOTED_VAR;
                    part_mut.text.set_offset(target_off, s.len());
                }
                ResolvedWordPart::CommandSubstitution(_) => {
                    part_mut.tag = WordPartTag::COMMAND_SUBSTITUTION;
                    part_mut.command.set_offset(target_off);
                }
                ResolvedWordPart::QuotedCommandSubstitution(_) => {
                    part_mut.tag = WordPartTag::QUOTED_COMMAND_SUBSTITUTION;
                    part_mut.command.set_offset(target_off);
                }
                ResolvedWordPart::Arithmetic(s) => {
                    part_mut.tag = WordPartTag::ARITHMETIC;
                    part_mut.text.set_offset(target_off, s.len());
                }
                ResolvedWordPart::QuotedArithmetic(s) => {
                    part_mut.tag = WordPartTag::QUOTED_ARITHMETIC;
                    part_mut.text.set_offset(target_off, s.len());
                }
            }
        }
        result_slice
    }
}

impl std::ops::Deref for ASTBuilder {
    type Target = relative::Buffer;
    fn deref(&self) -> &relative::Buffer {
        &self.buf
    }
}

impl std::ops::DerefMut for ASTBuilder {
    fn deref_mut(&mut self) -> &mut relative::Buffer {
        &mut self.buf
    }
}

impl Command {
    /// Serializes this command AST into the provided byte vector.
    pub fn serialize_into(&self, out: &mut Vec<u8>, source_buf: &relative::Buffer) {
        let start_len = out.len();
        out.extend_from_slice(source_buf.as_bytes());

        let self_offset = (self as *const Command as *const u8 as usize)
            - (source_buf.as_bytes().as_ptr() as usize);
        let header_slice = &mut out[start_len..start_len + std::mem::size_of::<FlatASTHeader>()];
        let header: &mut FlatASTHeader = zerocopy::FromBytes::mut_from_bytes(header_slice).unwrap();
        header.root_cmd = relative::Ptr::new(self_offset);
    }

    /// Serializes this command AST into a new byte vector.
    pub fn serialize(&self, source_buf: &relative::Buffer) -> Vec<u8> {
        let mut out = Vec::new();
        self.serialize_into(&mut out, source_buf);
        out
    }
}
