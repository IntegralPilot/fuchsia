// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module contains the grammar for the FFX playground command language.
//! We've documented the rules with a modified BNF. Some conventions:
//!   * The character `⊔` is the name of the rule that matches whitespace. It
//!     appears as a defined non-terminal where we define what constitutes
//!     whitespace in the grammar.
//!   * `A ← B C` is a normal rule. `A ←⊔ B C` is a rule where there is implicit
//!     whitespace between the elements.
//!   * `/` indicates ordered alternation, meaning an alternation where the left
//!     side of the operator will be parsed first, and only if it fails to match
//!     will the right side be attempted. This is not an unusual convention, but
//!     the operation itself is unusual if one is used to looking at BNF for
//!     context free grammars rather than combinator parsers. The normal
//!     alternation operator, usually denoted `|`, will likely not appear in
//!     this grammar.

use nom::branch::alt;
use nom::bytes::complete::{tag, take, take_while1};
use nom::character::complete::{
    alphanumeric1, anychar, char as chr, digit1, hex_digit1, none_of, one_of,
};
use nom::combinator::{
    all_consuming, cond, eof, flat_map, map, map_parser, not, opt, peek, recognize, rest, verify,
};
use nom::multi::{
    fold_many0, many_till, many0, many0_count, many1, separated_list0, separated_list1,
};
use nom::sequence::{delimited, pair, preceded, separated_pair, terminated};
use nom::{Input, OutputMode, PResult, Parser, error as nom_error};
use nom_locate::LocatedSpan;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::hash::Hash;
use std::rc::Rc;

/// Value indicating whether a variable is mutable or constant
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Mutability {
    Constant,
    Mutable,
}

impl Mutability {
    pub fn is_constant(&self) -> bool {
        matches!(self, Mutability::Constant)
    }
}

#[derive(Debug)]
struct Error;

impl<'a> nom_error::ParseError<ESpan<'a>> for Error {
    fn from_error_kind(_input: ESpan<'a>, _kind: nom_error::ErrorKind) -> Self {
        Error
    }

    fn append(_input: ESpan<'a>, _kind: nom_error::ErrorKind, other: Self) -> Self {
        other
    }
}

type IResult<'a, O> = nom::IResult<ESpan<'a>, O, Error>;

/// Indicates a type of tab completion that might be able to occur at a given position.
#[derive(Debug, Clone)]
pub enum TabHint<'a> {
    Invocable(Span<'a>),
    CommandArgument(Span<'a>),
}

impl<'a> TabHint<'a> {
    pub fn span(&self) -> &Span<'a> {
        match &self {
            TabHint::Invocable(x) => x,
            TabHint::CommandArgument(x) => x,
        }
    }
}

/// Global parser state which keeps track of backtracking as well as tab completion.
#[derive(Debug, Clone)]
struct ParseState<'a> {
    /// Accumulates [`ParseResult::tab_completions`]
    tab_completions: BTreeMap<usize, Vec<TabHint<'a>>>,
    /// Accumulates [`ParseResult::whitespace`]
    whitespace: HashSet<HashSpanWrapper<'a>>,
}

impl<'a> ParseState<'a> {
    fn new() -> Self {
        ParseState { tab_completions: BTreeMap::new(), whitespace: HashSet::new() }
    }
}

#[derive(Debug, Clone)]
struct ErrNode<'a> {
    span: Span<'a>,
    msg: String,
    next: Option<Rc<ErrNode<'a>>>,
}

pub type Span<'a> = LocatedSpan<&'a str>;
type ESpan<'a> = LocatedSpan<&'a str, (Rc<RefCell<ParseState<'a>>>, Option<Rc<ErrNode<'a>>>)>;

/// Wrapper around a [`Span`] that implements [`Hash`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct HashSpanWrapper<'a>(Span<'a>);

impl<'a> Hash for HashSpanWrapper<'a> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.location_offset().hash(state);
        self.0.fragment().hash(state);
    }
}

trait StripParseState<'a> {
    fn strip_parse_state(self) -> Span<'a>;
    fn replace_parse_state(
        self,
        state: (Rc<RefCell<ParseState<'a>>>, Option<Rc<ErrNode<'a>>>),
    ) -> ESpan<'a>;
}

impl<'a> StripParseState<'a> for ESpan<'a> {
    fn strip_parse_state(self) -> Span<'a> {
        self.map_extra(|_| ())
    }

    fn replace_parse_state(
        self,
        state: (Rc<RefCell<ParseState<'a>>>, Option<Rc<ErrNode<'a>>>),
    ) -> ESpan<'a> {
        self.map_extra(move |_| state)
    }
}

/// Element of a string. `Body` elements contain the text of the string. Escape
/// sequences are still present but are known to be well-formed. `Interpolation`
/// contains an expression from an interpolated value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StringElement<'a> {
    Body(Span<'a>),
    Interpolation(Node<'a>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node<'a> {
    Add(Box<Node<'a>>, Box<Node<'a>>),
    Async(Box<Node<'a>>),
    Assignment(Box<Node<'a>>, Box<Node<'a>>),
    BareString(Span<'a>),
    Block(Vec<Node<'a>>),
    Divide(Box<Node<'a>>, Box<Node<'a>>),
    EQ(Box<Node<'a>>, Box<Node<'a>>),
    FunctionDecl { identifier: Span<'a>, parameters: ParameterList<'a>, body: Box<Node<'a>> },
    GE(Box<Node<'a>>, Box<Node<'a>>),
    GT(Box<Node<'a>>, Box<Node<'a>>),
    Identifier(Span<'a>),
    If { condition: Box<Node<'a>>, body: Box<Node<'a>>, else_: Option<Box<Node<'a>>> },
    Import(Span<'a>, Option<Span<'a>>),
    Integer(Span<'a>),
    Invocation(Span<'a>, Vec<Node<'a>>),
    Iterate(Box<Node<'a>>, Box<Node<'a>>),
    LE(Box<Node<'a>>, Box<Node<'a>>),
    LT(Box<Node<'a>>, Box<Node<'a>>),
    Label(Span<'a>),
    Lambda { parameters: ParameterList<'a>, body: Box<Node<'a>> },
    List(Vec<Node<'a>>),
    LogicalAnd(Box<Node<'a>>, Box<Node<'a>>),
    LogicalNot(Box<Node<'a>>),
    LogicalOr(Box<Node<'a>>, Box<Node<'a>>),
    Lookup(Box<Node<'a>>, Box<Node<'a>>),
    Multiply(Box<Node<'a>>, Box<Node<'a>>),
    NE(Box<Node<'a>>, Box<Node<'a>>),
    Negate(Box<Node<'a>>),
    Object(Option<Span<'a>>, Vec<(Node<'a>, Node<'a>)>),
    Pipe(Box<Node<'a>>, Box<Node<'a>>),
    Program(Vec<Node<'a>>),
    Range(Box<Option<Node<'a>>>, Box<Option<Node<'a>>>, bool),
    Real(Span<'a>),
    String(Vec<StringElement<'a>>),
    Subtract(Box<Node<'a>>, Box<Node<'a>>),
    VariableDecl { identifier: Span<'a>, value: Box<Node<'a>>, mutability: Mutability },
    True,
    False,
    Null,
    Error,
}

#[cfg(test)]
impl<'a> Node<'a> {
    /// Checks whether two nodes refer to equivalent content. This is a rough
    /// sort of comparison suitable for comparing test case output to a
    /// reference node tree. To use a normal derived comparison, we'd have to
    /// have all our spans in our reference outputs have correct
    /// line/column/offset information. The tests are hideous to specify that way.
    fn content_eq<'b>(&self, other: &Node<'b>) -> bool {
        use Node::*;
        match (self, other) {
            (Add(a, b), Add(c, d)) => a.content_eq(c) && b.content_eq(d),
            (Async(a), Async(b)) => a.content_eq(b),
            (Assignment(a, b), Assignment(c, d)) => a.content_eq(c) && b.content_eq(d),
            (BareString(a), BareString(b)) => a.fragment() == b.fragment(),
            (Divide(a, b), Divide(c, d)) => a.content_eq(c) && b.content_eq(d),
            (EQ(a, b), EQ(c, d)) => a.content_eq(c) && b.content_eq(d),
            (GE(a, b), GE(c, d)) => a.content_eq(c) && b.content_eq(d),
            (GT(a, b), GT(c, d)) => a.content_eq(c) && b.content_eq(d),
            (Identifier(a), Identifier(b)) => a.fragment() == b.fragment(),
            (Integer(a), Integer(b)) => a.fragment() == b.fragment(),
            (
                If { condition: condition_a, body: body_a, else_: else_a },
                If { condition: condition_b, body: body_b, else_: else_b },
            ) => {
                condition_a.content_eq(condition_b)
                    && body_a.content_eq(body_b)
                    && match (else_a, else_b) {
                        (Some(a), Some(b)) => a.content_eq(b),
                        (None, None) => true,
                        _ => false,
                    }
            }
            (Invocation(a, a_args), Invocation(b, b_args)) => {
                a.fragment() == b.fragment()
                    && a_args.len() == b_args.len()
                    && a_args.iter().zip(b_args.iter()).all(|(a, b)| a.content_eq(b))
            }
            (Iterate(a, b), Iterate(c, d)) => a.content_eq(c) && b.content_eq(d),
            (LE(a, b), LE(c, d)) => a.content_eq(c) && b.content_eq(d),
            (LT(a, b), LT(c, d)) => a.content_eq(c) && b.content_eq(d),
            (List(a), List(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| a.content_eq(b))
            }
            (LogicalAnd(a, b), LogicalAnd(c, d)) => a.content_eq(c) && b.content_eq(d),
            (LogicalNot(a), LogicalNot(b)) => a.content_eq(b),
            (LogicalOr(a, b), LogicalOr(c, d)) => a.content_eq(c) && b.content_eq(d),
            (Lookup(a, b), Lookup(c, d)) => a.content_eq(c) && b.content_eq(d),
            (Multiply(a, b), Multiply(c, d)) => a.content_eq(c) && b.content_eq(d),
            (NE(a, b), NE(c, d)) => a.content_eq(c) && b.content_eq(d),
            (Negate(a), Negate(b)) => a.content_eq(b),
            (Object(label_a, a), Object(label_b, b)) => {
                label_a
                    .map(|label_a| {
                        label_b
                            .map(|label_b| label_b.fragment() == label_a.fragment())
                            .unwrap_or(false)
                    })
                    .unwrap_or_else(|| label_b.is_none())
                    && a.len() == b.len()
                    && a.iter()
                        .zip(b.iter())
                        .all(|((a1, a2), (b1, b2))| a1.content_eq(b1) && a2.content_eq(b2))
            }
            (Pipe(a, b), Pipe(c, d)) => a.content_eq(c) && b.content_eq(d),
            (Program(a), Program(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| a.content_eq(b))
            }
            (Block(a), Block(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| a.content_eq(b))
            }
            (Real(a), Real(b)) => a.fragment() == b.fragment(),
            (String(a), String(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b.iter()).all(|x| match x {
                        (StringElement::Body(a), StringElement::Body(b)) => {
                            a.fragment() == b.fragment()
                        }
                        (StringElement::Interpolation(a), StringElement::Interpolation(b)) => {
                            a.content_eq(b)
                        }
                        _ => false,
                    })
            }
            (Subtract(a, b), Subtract(c, d)) => a.content_eq(c) && b.content_eq(d),
            (
                FunctionDecl { identifier: identifier_a, parameters: parameters_a, body: body_a },
                FunctionDecl { identifier: identifier_b, parameters: parameters_b, body: body_b },
            ) => {
                identifier_a.fragment() == identifier_b.fragment()
                    && body_a.content_eq(body_b)
                    && parameters_a.content_eq(parameters_b)
            }
            (
                Lambda { parameters: parameters_a, body: body_a },
                Lambda { parameters: parameters_b, body: body_b },
            ) => body_a.content_eq(body_b) && parameters_a.content_eq(parameters_b),
            (
                VariableDecl { identifier: identifier_a, value: value_a, mutability: mutability_a },
                VariableDecl { identifier: identifier_b, value: value_b, mutability: mutability_b },
            ) => {
                identifier_a.fragment() == identifier_b.fragment()
                    && value_a.content_eq(value_b)
                    && mutability_a == mutability_b
            }
            (Error, Error) => true,
            (Import(path_a, ident_a), Import(path_b, ident_b)) => {
                path_a.fragment() == path_b.fragment()
                    && ident_a
                        .map(|ident_a| {
                            ident_b
                                .map(|ident_b| ident_b.fragment() == ident_a.fragment())
                                .unwrap_or(false)
                        })
                        .unwrap_or_else(|| ident_b.is_none())
            }
            _ => false,
        }
    }
}

/// Represents the parameters of a function or lambda.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterList<'a> {
    pub parameters: Vec<Span<'a>>,
    pub optional_parameters: Vec<Span<'a>>,
    pub variadic: Option<Span<'a>>,
}

#[cfg(test)]
impl ParameterList<'_> {
    fn content_eq(&self, other: &Self) -> bool {
        self.parameters.len() == other.parameters.len()
            && self
                .parameters
                .iter()
                .zip(other.parameters.iter())
                .all(|(x, y)| x.fragment() == y.fragment())
            && self.optional_parameters.len() == other.optional_parameters.len()
            && self
                .optional_parameters
                .iter()
                .zip(other.optional_parameters.iter())
                .all(|(x, y)| x.fragment() == y.fragment())
            && match (&self.variadic, &other.variadic) {
                (Some(a), Some(b)) => a.fragment() == b.fragment(),
                (None, None) => true,
                _ => false,
            }
    }
}

/// Result of parsing. Contains a Node which roots the parse tree and a list of errors.
#[derive(Debug)]
pub struct ParseResult<'a> {
    pub tree: Node<'a>,
    pub errors: Vec<(Span<'a>, String)>,

    /// Positions where we might be able to tab-complete
    pub tab_completions: BTreeMap<usize, Vec<TabHint<'a>>>,

    /// Ranges where there is whitespace in the parsed text. Used mostly to move
    /// around hints from tab_completions.
    pub whitespace: Vec<Span<'a>>,
}

impl<'a> From<&'a str> for ParseResult<'a> {
    fn from(text: &'a str) -> ParseResult<'a> {
        let text = ESpan::new_extra(text, (Rc::new(RefCell::new(ParseState::new())), None));
        let (end_pos, tree) = alt((
            terminated(
                map(ws_around(program), Node::Program),
                alt((not(anychar), map(err_skip("Trailing characters {}", rest), |_| ()))),
            ),
            err_skip("Unrecoverable parse error", rest),
        ))
        .parse(text)
        .expect("Incorrectly handled parse error");

        let extra = Rc::try_unwrap(end_pos.extra.0).unwrap().into_inner();

        let mut error_node = end_pos.extra.1;
        let mut errors = Vec::new();

        while let Some(error) = error_node.take() {
            let error = Rc::try_unwrap(error).unwrap_or_else(|e| (*e).clone());
            errors.push((error.span, error.msg));
            error_node = error.next;
        }

        errors.reverse();

        let tab_completions = extra.tab_completions;
        let mut whitespace: Vec<Span<'a>> = extra.whitespace.into_iter().map(|x| x.0).collect();
        whitespace.sort_by_key(|x| x.location_offset());

        ParseResult { tree, errors, tab_completions, whitespace }
    }
}

/// Handle an error by skipping some parsed data. If the skip parser fails the error handling
/// fails.
fn err_skip<'a, F, S, X>(msg: S, f: F) -> impl Parser<ESpan<'a>, Output = Node<'a>, Error = Error>
where
    F: Parser<ESpan<'a>, Output = X, Error = Error>,
    S: ToString + 'a,
{
    let mut f = recognize(f);
    move |input| {
        let (out_span, result) = f.parse(input)?;
        let parse_state = Rc::clone(&out_span.extra.0);
        let msg = msg.to_string().replace("{}", *result.fragment());
        let error = Some(Rc::new(ErrNode {
            span: result.strip_parse_state(),
            msg,
            next: out_span.extra.1.clone(),
        }));

        let out_span = out_span.replace_parse_state((parse_state, error));
        Ok((out_span, Node::Error))
    }
}

/// Handle an error by simply marking it with an error node and moving along.
fn err_insert<'a, S: ToString + 'a>(
    msg: S,
) -> impl Parser<ESpan<'a>, Output = Node<'a>, Error = Error> {
    err_skip(msg, |x| Ok((x, Node::Error)))
}

/// Handle an error by reporting it but introduce no node.
fn err_note<'a, S: ToString + 'a>(msg: S) -> impl Parser<ESpan<'a>, Output = (), Error = Error> {
    map(err_insert(msg), |_| ())
}

/// Same as `tag` but inserts an error if the tag is missing.
fn ex_tag<'a>(s: &'a str) -> impl Parser<ESpan<'a>, Output = (), Error = Error> + 'a {
    alt((map(tag(s), |_| ()), err_note(format!("Expected '{}'", s))))
}

/// Inner state of [`DeferredError`].
struct DeferredErrorInner<'a, O> {
    orig_location: ESpan<'a>,
    output: (ESpan<'a>, O),
}

/// Storage for an error that we didn't allow to be handled but might want to later.
struct DeferredError<'a, O>(RefCell<Option<DeferredErrorInner<'a, O>>>);

impl<'a, O> DeferredError<'a, O> {
    /// Create a new [`DeferredError`].
    fn new() -> Self {
        DeferredError(RefCell::new(None))
    }

    /// Parser wrapper that will defer error handling for the wrapped parser.
    /// I.e. if the parser handles a new error and returns normally, this will
    /// turn that handled error into a hard failure, but save the parse result
    /// with the handled error for later use.
    fn defer<'b>(
        &'b self,
        mut f: impl Parser<ESpan<'a>, Output = O, Error = Error> + 'b,
    ) -> impl Parser<ESpan<'a>, Output = O, Error = Error> + 'b {
        move |input: ESpan<'a>| {
            let err = input.extra.1.clone();
            let res = f.parse(input.clone())?;
            let err_new = res.0.extra.1.clone();

            match (err, err_new) {
                (None, None) => Ok(res),
                (Some(x), Some(y)) if Rc::ptr_eq(&x, &y) => Ok(res),
                (Some(_), None) => unreachable!("Parsing further *removed* errors?!"),
                (_, Some(_)) => {
                    *self.0.borrow_mut() =
                        Some(DeferredErrorInner { orig_location: input, output: res });
                    Err(nom::Err::Error(Error))
                }
            }
        }
    }

    /// If a previous call to `defer` suppressed an error, this parser will
    /// immediately return that error provided the location is the same.
    fn restore<'b>(&'b self) -> impl Parser<ESpan<'a>, Output = O, Error = Error> + 'b {
        |input| {
            if let Some(inner) = self.0.borrow_mut().take() {
                if inner.orig_location == input {
                    return Ok(inner.output);
                }
            }

            Err(nom::Err::Error(Error))
        }
    }
}

/// Runs the passed parser but does not allow it to emit errors.
fn no_errors<'a, F>(f: F) -> NoErrors<F>
where
    F: Parser<ESpan<'a>>,
{
    NoErrors { parser: f }
}

struct NoErrors<F> {
    parser: F,
}

impl<'a, F, O> Parser<ESpan<'a>> for NoErrors<F>
where
    F: Parser<ESpan<'a>, Output = O, Error = Error> + Clone,
{
    type Output = <F as Parser<ESpan<'a>>>::Output;
    type Error = <F as Parser<ESpan<'a>>>::Error;

    fn process<OM: OutputMode>(
        &mut self,
        input: ESpan<'a>,
    ) -> PResult<OM, ESpan<'a>, Self::Output, Self::Error> {
        DeferredError::new().defer(self.parser.clone()).process::<OM>(input)
    }
}

/// Match optional whitespace before the given combinator.
fn ws_before<'a, F: Parser<ESpan<'a>, Output = X, Error = Error>, X>(
    f: F,
) -> impl Parser<ESpan<'a>, Output = X, Error = Error> {
    preceded(opt(whitespace), f)
}

/// Match optional whitespace after the given combinator.
fn ws_after<'a, F: Parser<ESpan<'a>, Output = X, Error = Error>, X>(
    f: F,
) -> impl Parser<ESpan<'a>, Output = X, Error = Error> {
    terminated(f, opt(whitespace))
}

/// Match optional whitespace around the given combinator.
fn ws_around<'a, F: Parser<ESpan<'a>, Output = X, Error = Error>, X>(
    f: F,
) -> impl Parser<ESpan<'a>, Output = X, Error = Error> {
    ws_before(ws_after(f))
}

/// Version of `separated_list1` combinator that matches optional whitespace between each
/// item.
fn ws_separated_nonempty_list<
    'a,
    FS: Parser<ESpan<'a>, Output = Y, Error = Error>,
    F: Parser<ESpan<'a>, Output = X, Error = Error>,
    X,
    Y,
>(
    fs: FS,
    f: F,
) -> impl Parser<ESpan<'a>, Output = Vec<X>, Error = Error> {
    separated_list1(ws_around(fs), f)
}

/// Marks the contained parser as parsing something which could be tab-completed.
fn completion_hint<'a, X>(
    mut f: impl Parser<ESpan<'a>, Output = X, Error = Error>,
    hint: impl Fn(Span<'a>) -> TabHint<'a>,
) -> impl Parser<ESpan<'a>, Output = X, Error = Error> {
    move |span: ESpan<'a>| {
        let hint_span = span.clone();
        let ret = f.parse(span);

        let hint_span = if let Ok((end, _)) = &ret {
            let start = hint_span.location_offset();
            let len = end.location_offset() - start;
            hint_span.take(len)
        } else {
            hint_span.take(0)
        };

        let extra = Rc::clone(&hint_span.extra.0);
        let location = hint_span.location_offset();
        let hint = hint(hint_span.strip_parse_state());
        extra.borrow_mut().tab_completions.entry(location).or_insert_with(Vec::new).push(hint);

        ret
    }
}

/// Left-associative operator parsing.
fn lassoc<
    'a: 'b,
    'b,
    F: Parser<ESpan<'a>, Output = X, Error = Error> + 'b,
    FM: Fn(Y, Y) -> X + 'b,
    X: 'b,
    Y: From<X>,
>(
    f: F,
    oper: &'b str,
    mapper: FM,
) -> impl Parser<ESpan<'a>, Output = X, Error = Error> + 'b {
    map(ws_separated_nonempty_list(tag(oper), f), move |items| {
        let mut items = items.into_iter();
        let first = items.next().unwrap();
        items.fold(first, |a, b| mapper(Y::from(a), Y::from(b)))
    })
}

/// Left-associative operator parsing where the operator may take many forms.
fn lassoc_choice<
    'a: 'b,
    'b,
    F: Parser<ESpan<'a>, Output = X, Error = Error> + 'b + Copy,
    FO: Parser<ESpan<'a>, Output = Y, Error = Error> + 'b,
    FM: Fn(X, Y, X) -> X + 'b,
    X: 'b,
    Y: 'b,
>(
    f: F,
    oper: FO,
    mapper: FM,
) -> impl Parser<ESpan<'a>, Output = X, Error = Error> + 'b {
    map(pair(f, many0(pair(ws_around(oper), f))), move |(first, items)| {
        items.into_iter().fold(first, |a, (op, b)| mapper(a, op, b))
    })
}

const KEYWORDS: [&str; 9] =
    ["let", "const", "def", "if", "else", "true", "false", "null", "import"];

/// Match a keyword.
fn kw<'s, 'a: 's>(kw: &'s str) -> impl Parser<ESpan<'a>, Output = ESpan<'a>, Error = Error> + 's {
    debug_assert!(KEYWORDS.contains(&kw));
    terminated(tag(kw), not(alt((alphanumeric1, tag("_")))))
}

/// We define Whitespace as follows:
///
/// ```
/// ⊔ ← '#' (!<nl> .)* <nl> / AnyUnicodeWhitespace+
/// ```
///
/// Where `AnyUnicodeWhitespace` is any single character classified as whitespace by the Unicode
/// standard.
///
/// Note that our comment syntax is embedded in our whitespace definition:
///
/// ```
/// # This line will parse entirely as whitespace.
/// ```
fn whitespace<'a>(input: ESpan<'a>) -> IResult<'a, ESpan<'a>> {
    map(
        recognize(many1(alt((
            take_while1(char::is_whitespace),
            recognize((chr('#'), many_till(anychar, chr('\n')))),
        )))),
        |span: ESpan<'a>| {
            span.extra
                .0
                .borrow_mut()
                .whitespace
                .insert(HashSpanWrapper(span.clone().strip_parse_state()));
            span
        },
    )
    .parse(input)
}

/// Unescaped Identifiers are defined as follows:
///
/// ```
/// UnescapedIdentifier ← [a-zA-Z0-9_]+
/// ```
///
/// Valid unescaped identifiers might include:
///
/// ```
/// 0
/// 3_bean_salad
/// foo
/// item_0
/// a_Mixed_Bag
/// ```
fn unescaped_identifier<'a>(input: ESpan<'a>) -> IResult<'a, ESpan<'a>> {
    recognize(many1(alt((alphanumeric1, tag("_"))))).parse(input)
}

/// Identifiers are defined as follows:
///
/// ```
/// Identifier ← ![0-9] UnescapedIdentifier
/// ```
///
/// Valid identifiers might include:
///
/// ```
/// foo
/// item_0
/// a_Mixed_Bag
/// ```
fn identifier<'a>(input: ESpan<'a>) -> IResult<'a, ESpan<'a>> {
    verify(preceded(not(digit1), unescaped_identifier), |x| !KEYWORDS.contains(x.fragment()))
        .parse(input)
}

/// Integers are defined as follows:
///
/// ```
/// Digit ← [0-9]
/// HexDigit ← [a-fA-F0-9]
/// DecimalInteger ← '0' !Digit / !'0' Digit+ ( '_' Digit+ )*
/// HexInteger ← '0x' HexDigit+ ( '_' HexDigit+ )*
/// Integer ← DecimalInteger / HexInteger
/// ```
///
/// Valid integers might include:
///
/// ```
/// 0
/// 12345
/// 12_345
/// 0x1234abcd
/// 0x12_abcd
/// ```
fn integer<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(
        alt((
            preceded(not(chr('0')), recognize(separated_list1(chr('_'), digit1))),
            recognize((tag("0x"), separated_list1(chr('_'), hex_digit1))),
            terminated(tag("0"), not(digit1)),
        )),
        |x: ESpan<'a>| Node::Integer(x.strip_parse_state()),
    )
    .parse(input)
}

/// Reals are defined as follows:
///
/// ```
/// Real ← Integer '.' Digit+ ( '_' Digit+ )*
/// ```
///
/// Reals look like:
///
/// ```
/// 3.14
/// 12_345.67
/// 1_2_3.45_6
/// ```
fn real<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(recognize((integer, chr('.'), digit1, many0(pair(chr('_'), digit1)))), |x| {
        Node::Real(x.strip_parse_state())
    })
    .parse(input)
}

/// Strings are defined as follows:
///
/// ```
/// EscapeSequence ← '\n' / '\t' / '\r' / '\' <nl> / '\\' / '\"' / '\u' HexDigit{6}
/// Interpolation ← '$' InterpolationBody
/// InterpolationBody ← Identifier / '{' ⊔ Expression ⊔ '}'
/// StringEntity ← !( '\' / '"' / <nl> ) . / EscapeSequence / Interpolation
/// NormalString ← '"' StringEntity* '"'
/// String ← NormalString / MultiString
/// ```
///
/// TODO: Define `MultiString`
///
/// Valid strings might include:
///
/// ```
/// "The quick brown fox jumped over the lazy dog."
/// "A newline.\nA tab\tA code point\u00264b"
/// "String starts here \
/// and keeps on going"
/// "A string has $interpolation"
/// "A string has ${ bracketed --interpolation }"
/// ```
fn string<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    normal_string(input)
}

/// See `string`
fn normal_string<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    fn escape_sequence<'a>(input: ESpan<'a>) -> IResult<'a, ESpan<'a>> {
        alt((
            tag(r"\n"),
            tag(r"\t"),
            tag(r"\r"),
            tag("\\\n"),
            tag(r"\$"),
            tag(r#"\\"#),
            tag(r#"\""#),
            recognize((tag(r"\u"), map_parser(take(6usize), all_consuming(hex_digit1)))),
            recognize(pair(tag(r"\u"), err_note("'\\u' followed by invalid hex value"))),
            recognize(pair(chr('\\'), err_skip("Bad escape sequence: '\\{}'", anychar))),
            recognize(pair(chr('\\'), err_note("Escape sequence at end of input"))),
        ))
        .parse(input)
    }

    fn interpolation<'a>(input: ESpan<'a>) -> IResult<'a, StringElement<'a>> {
        preceded(
            chr('$'),
            alt((
                map(unescaped_identifier, |x| {
                    StringElement::Interpolation(Node::Identifier(x.strip_parse_state()))
                }),
                map(
                    delimited(chr('{'), ws_around(ex_expression), ex_tag("}")),
                    StringElement::Interpolation,
                ),
                map(tag("$"), |x: ESpan<'a>| StringElement::Body(x.strip_parse_state())),
                map(err_insert("Expected identifier, interpolated block, or $"), |_| {
                    StringElement::Interpolation(Node::Error)
                }),
            )),
        )
        .parse(input)
    }

    map(
        delimited(
            chr('"'),
            many0(alt((
                map(recognize(many1(alt((recognize(none_of("$\\\"\n")), escape_sequence)))), |x| {
                    StringElement::Body(x.strip_parse_state())
                }),
                interpolation,
            ))),
            ex_tag("\""),
        ),
        Node::String,
    )
    .parse(input)
}

/// Variable declarations are defined as follows:
///
/// ```
/// KWVar ← 'let' !( IdentifierCharacter / '$' )
/// KWConst ← 'const' !( IdentifierCharacter / '$' )
/// VariableDecl ←⊔ ( KWVar / KWConst ) ( Identifier / '$' UnescapedIdentifier ) '=' Expression
/// ```
///
/// Valid variable declarations might include:
///
/// ```
/// let foo = 4
/// const foo = "Ham Sandwich"
/// let $0 = "An Zer0e"
/// ```
fn variable_decl<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(
        (
            alt((kw("let"), kw("const"))),
            ws_around(alt((identifier, preceded(ex_tag("$"), unescaped_identifier)))),
            chr('='),
            ws_before(ex_expression),
        ),
        |(keyword, identifier, _, value)| Node::VariableDecl {
            identifier: identifier.strip_parse_state(),
            value: Box::new(value),
            mutability: if *keyword.fragment() == "const" {
                Mutability::Constant
            } else {
                Mutability::Mutable
            },
        },
    )
    .parse(input)
}

/// Parameter Lists are defined as follows
///
/// ```
/// OptionalParameter ← Identifier '?'
/// Variadic ← Identifier '..'
/// ParameterList ←⊔ ( Identifier ![.?] )* OptionalParameter* Variadic?
/// ```
///
/// Valid parameter lists might include:
///
/// ```
/// a b
/// a b..
/// a b?
/// a b? c..
/// ```
fn parameter_list<'a>(input: ESpan<'a>) -> IResult<'a, ParameterList<'a>> {
    map(
        (
            separated_list0(whitespace, terminated(identifier, not(one_of(".?")))),
            many0(ws_before(terminated(identifier, chr('?')))),
            opt(ws_before(terminated(identifier, tag("..")))),
        ),
        |(parameters, optional_parameters, variadic)| ParameterList {
            parameters: parameters.into_iter().map(StripParseState::strip_parse_state).collect(),
            optional_parameters: optional_parameters
                .into_iter()
                .map(StripParseState::strip_parse_state)
                .collect(),
            variadic: variadic.map(StripParseState::strip_parse_state),
        },
    )
    .parse(input)
}

/// Function declarations are defined as follows:
///
/// ```
/// FunctionDecl ←⊔ 'def' Identifier ParameterList Block
/// ```
///
/// Valid function declarations might include:
///
/// ```
/// def dothing (c d) { do_other $c; $d * 7 }
/// def variadic a b.. { print $a; print_list $b }
/// def optional a b? { print $a; print_maybe_null $b }
/// def optional_variadic a b? c.. { etc a b c }
/// ```
fn function_decl<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(
        preceded(
            kw("def"),
            (
                ws_before(identifier),
                delimited(opt(tag("(")), ws_around(parameter_list), opt(tag(")"))),
                block,
            ),
        ),
        |(identifier, parameters, body)| Node::FunctionDecl {
            identifier: identifier.strip_parse_state(),
            parameters,
            body: Box::new(body.into()),
        },
    )
    .parse(input)
}

/// Short function declarations are defined as follows:
///
/// ```
/// ShortFunctionDecl ←⊔ 'def' Identifier '(' ParameterList ')' Expression
/// ```
///
/// Valid function declarations might include:
///
/// ```
/// def frob (a b)  $a + $b
/// def jim () cmd arg argtwo ;
/// def wembly (a..) cmd &
/// def variadic (a b..) { print $a; print_list $b }
/// ```
fn short_function_decl<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(
        preceded(
            kw("def"),
            (
                ws_before(identifier),
                delimited(ws_around(tag("(")), parameter_list, ws_around(tag(")"))),
                ex_expression,
            ),
        ),
        |(identifier, parameters, body)| Node::FunctionDecl {
            identifier: identifier.strip_parse_state(),
            parameters,
            body: Box::new(body),
        },
    )
    .parse(input)
}

/// Object literals are defined as follows:
///
/// ```
/// Object ←⊔ ObjectLabel? '{' ObjectBody? '}'
/// ObjectLabel ← '@' Identifier
/// ObjectBody ←⊔ Field ( ',' Field  )* ','?
/// Field ←⊔ ( NormalString / Identifier  ) ':' SimpleExpression
/// ```
///
/// Valid object literals might include:
///
/// ```
/// {}
/// { foo: 6, "bar & grill": "Open now"  }
/// { foo: { bar: 6  }, "bar & grill": "Open now"  }
/// @Labeled { foo: 5 }
/// ```
fn object<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    fn field<'a>(
        is_first: bool,
    ) -> impl Parser<ESpan<'a>, Output = (Node<'a>, Node<'a>), Error = Error> {
        separated_pair(
            alt((normal_string, map(identifier, |x| Node::Identifier(x.strip_parse_state())))),
            ws_around(alt((
                tag(":"),
                preceded(
                    cond(is_first, not(ws_before(chr('}')))),
                    recognize(err_skip(
                        "Expected ':'",
                        many0(preceded(
                            not(alt((
                                recognize(identifier),
                                recognize(chr('$')),
                                recognize(chr('}')),
                                whitespace,
                            ))),
                            anychar,
                        )),
                    )),
                ),
            ))),
            map(simple_expression, Node::from),
        )
    }

    fn object_body<'a>(input: ESpan<'a>) -> IResult<'a, Vec<(Node<'a>, Node<'a>)>> {
        map(
            opt(terminated(
                flat_map(field(true), |node| {
                    let mut init = Some(vec![node]);
                    fold_many0(
                        preceded(ws_around(chr(',')), field(false)),
                        move || init.take().unwrap(),
                        |mut acc, item| {
                            acc.push(item);
                            acc
                        },
                    )
                }),
                opt(ws_before(tag(","))),
            )),
            |x| x.unwrap_or_default(),
        )
        .parse(input)
    }

    flat_map(opt(ws_after(preceded(chr('@'), identifier))), |name| {
        move |input: ESpan<'a>| {
            let res = {
                let name = name.clone();
                map(delimited(chr('{'), ws_around(object_body), tag("}")), move |body| {
                    Node::Object(name.clone().map(|x| x.strip_parse_state()), body)
                })
                .parse(input.clone())
            };

            if res.is_ok() {
                return res;
            }

            let Some(name) = &name else {
                return res;
            };

            err_insert(format!("Expected object body after @{}", name.fragment())).parse(input)
        }
    })
    .parse(input)
}

/// List literals are defined as follows:
///
/// ```
/// List ←⊔ '[' ListBody? ']'
/// ListBody ←⊔ SimpleExpression ( ',' SimpleExpression  )* ','?
/// ```
///
/// Valid list literals might include:
///
/// ```
/// []
/// [ 6, "Open now"  ]
/// [ { bar: 6 }, "Open now"  ]
/// ```
fn list<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    fn list_body<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
        map(
            opt(terminated(
                flat_map(simple_expression, |node| {
                    let node = node.into();
                    let mut init = Some(vec![node]);
                    fold_many0(
                        preceded(ws_around(chr(',')), simple_expression),
                        move || init.take().unwrap(),
                        |mut acc, item| {
                            acc.push(item.into());
                            acc
                        },
                    )
                }),
                opt(ws_before(tag(","))),
            )),
            |x| Node::List(x.unwrap_or_default()),
        )
        .parse(input)
    }

    delimited(chr('['), ws_around(list_body), ex_tag("]")).parse(input)
}

/// Lambda expressions are defined as follows:
///
/// ```
/// Lambda ←⊔ '\' Identifier Invocation
/// Lambda ←⊔ '\(' ParameterList ')' Invocation
/// ```
///
/// Lambdas look like:
///
/// ```
/// \a  $a * 2
/// \(a b) $a + $b
/// \() doThing $arg $arg
/// \a { let s = 2; $a + $s }
/// ```
fn lambda<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(
        pair(
            alt((
                delimited(tag("\\("), ws_before(parameter_list), ws_around(tag(")"))),
                map(preceded(tag("\\"), ws_around(identifier)), |parameter| ParameterList {
                    parameters: vec![parameter.strip_parse_state()],
                    optional_parameters: Vec::new(),
                    variadic: None,
                }),
            )),
            invocation,
        ),
        |(parameters, body)| Node::Lambda { parameters, body: Box::new(body.into()) },
    )
    .parse(input)
}

/// Lookups are defined as follows:
///
/// ```
/// Lookup ←⊔ Value ( '.' Identifier / '[' Expression ']')*
/// ```
///
/// It looks like this:
///
/// ```
/// $foo.bar
/// $foo["Bar"]
/// $foo[6 + 7]
/// ```
fn lookup<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(
        pair(
            value,
            many0(alt((
                preceded(
                    ws_around(tag(".")),
                    map(identifier, |x| Node::Label(x.strip_parse_state())),
                ),
                delimited(chr('['), ws_around(ex_expression), chr(']')),
            ))),
        ),
        |(x, y)| {
            y.into_iter()
                .fold(x, |prev, ident| Node::Lookup(Box::new(prev.into()), Box::new(ident)).into())
        },
    )
    .parse(input)
}

/// Parentheticals are defined as:
///
/// ```
/// Parenthetical ←⊔ '(' Expression ')'
/// ```
///
/// They look like this:
///
/// ```
/// 2 * ( 2 + 3 )
/// ```
fn parenthetical<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    delimited(chr('('), ws_around(ex_expression), chr(')')).parse(input)
}

/// Blocks are defined as:
///
/// ```
/// Block ←⊔ '{' Program '}'
/// ```
///
/// They look like this:
///
/// ```
/// { let s = 2; 2 + 2; }
/// ```
fn block<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(delimited(chr('{'), ws_around(program), chr('}')), Node::Block).parse(input)
}

/// Values are defined as follows:
///
/// ```
/// Value ← List / Object / Lambda / Parenthetical / Block / If / Atom
/// Atom ← String / Real / Integer / '$' UnescapedIdentifier
///
/// ```
fn value<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    alt((
        list,
        object,
        lambda,
        parenthetical,
        block,
        conditional,
        map(preceded(tag("$"), unescaped_identifier), |x| Node::Identifier(x.strip_parse_state())),
        map(kw("true"), |_| Node::True),
        map(kw("false"), |_| Node::False),
        map(kw("null"), |_| Node::Null),
        string,
        real,
        integer,
        map(preceded(err_insert("Expected '$'"), unescaped_identifier), |x| {
            Node::Identifier(x.strip_parse_state())
        }),
        err_insert("Expected value"),
    ))
    .parse(input)
}

/// Range literals are defined as follows:
///
/// ```
/// Range ←⊔ LogicalOr? ( '..=' / '..' ) LogicalOr? / LogicalOr
/// ```
///
/// Range literals look like so:
///
/// ```
/// 1..2
/// $a .. $a + 2
/// $a ..= $a + 2
/// $a ..
/// .. $b
/// ..
/// ```
fn range<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    let oper = preceded(tag(".."), map(opt(chr('=')), |x| x.is_some()));

    alt((
        map((opt(logical_or), ws_around(oper), opt(logical_or)), |(a, is_inclusive, b)| {
            Node::Range(Box::new(a.map(Node::from)), Box::new(b.map(Node::from)), is_inclusive)
                .into()
        }),
        logical_or,
    ))
    .parse(input)
}

/// Logical "and" and "or" are defined as follows:
///
/// ```
/// LogicalAnd ←⊔ LogicalNot ( '&&' LogicalNot )*
/// LogicalOr ←⊔ LogicalAnd ( '||' LogicalAnd )*
/// ```
///
/// It looks like this:
///
/// ```
/// $a && $b || $c && $d || $e && $f
/// ```
fn logical_or<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    lassoc(logical_and, "||", |x: Node<'a>, y| {
        Node::LogicalOr(Box::new(x.into()), Box::new(y.into()))
    })
    .parse(input)
}

/// See `logical_or`
fn logical_and<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    lassoc(logical_not, "&&", |x: Node<'a>, y| Node::LogicalAnd(Box::new(x), Box::new(y)))
        .parse(input)
}

/// Logical negation is defined as follows:
///
/// ```
/// LogicalNot ←⊔ '!'* Comparison
/// ```
///
/// It looks like this:
///
/// ```
/// !$a
/// !!$b
/// ```
fn logical_not<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(pair(many0_count(ws_after(chr('!'))), comparison), |(count, node)| {
        (0..count).fold(node, |x, _| Node::LogicalNot(Box::new(x.into())))
    })
    .parse(input)
}

/// Comparisons are defined as follows:
///
/// ```
/// CompOp ← '<=' / '<' / '>=' / '>' / '!=' / '=='
/// Comparison ←⊔ AddSub ( CompOp AddSub )*
/// ```
///
/// They look like this:
///
/// ```
/// $a > $b
/// $a < $b
/// $a <= $b
/// $a == $b
/// ```
fn comparison<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    fn comp_op<'a>(input: ESpan<'a>) -> IResult<'a, ESpan<'a>> {
        terminated(
            alt((tag("<="), tag("<"), tag(">="), tag(">"), tag("!="), tag("=="))),
            opt(ws_before(err_skip("Logical not can't occur here", chr('!')))),
        )
        .parse(input)
    }

    lassoc_choice(add_subtract, comp_op, |a, op, b| {
        match *op.fragment() {
            "<=" => Node::LE(Box::new(a), Box::new(b)),
            "<" => Node::LT(Box::new(a), Box::new(b)),
            ">=" => Node::GE(Box::new(a), Box::new(b)),
            ">" => Node::GT(Box::new(a), Box::new(b)),
            "!=" => Node::NE(Box::new(a), Box::new(b)),
            "==" => Node::EQ(Box::new(a), Box::new(b)),
            _ => unreachable!(),
        }
        .into()
    })
    .parse(input)
}

/// Addition is defined as follows:
///
/// ```
/// AddSub ←⊔ MulDiv ( [+-] MulDiv )*
/// ```
///
/// It looks as you'd expect:
///
/// ```
/// $a + $b
/// $a - $b
/// ```
fn add_subtract<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    lassoc_choice(multiply_divide, one_of("+-"), |a, op, b| match op {
        '-' => Node::Subtract(Box::new(a.into()), Box::new(b.into())),
        '+' => Node::Add(Box::new(a.into()), Box::new(b.into())),
        _ => unreachable!(),
    })
    .parse(input)
}

/// Multiplication/division is defined as follows:
///
/// ```
/// MulDiv ←⊔ Negate ( ( '*' / '//' ) Negate )*
/// ```
///
/// It looks like this:
///
/// ```
/// $a * $b
/// $a // $b
/// ```
fn multiply_divide<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    lassoc_choice(negate, alt((tag("*"), tag("//"))), |a, op, b| match *op.fragment() {
        "*" => Node::Multiply(Box::new(a.into()), Box::new(b.into())),
        "//" => Node::Divide(Box::new(a.into()), Box::new(b.into())),
        _ => unreachable!(),
    })
    .parse(input)
}

/// Arithmetic negation is defined as follows:
///
/// ```
/// Negate ←⊔ '-' Negate / Lookup
/// ```
///
/// It looks like this:
///
/// ```
/// -$a
/// ```
fn negate<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(pair(many0_count(ws_after(chr('-'))), lookup), |(count, node)| {
        (0..count).fold(node, |x, _| Node::Negate(Box::new(x)))
    })
    .parse(input)
}

/// We allow bare strings in a few places in the grammar, most notably as invocation arguments.
///
/// Bare strings are defined as:
///
/// ```
/// BareString ← ( !⊔ ![@{}|&;()] . )+
/// ```
///
/// Bare strings look like:
///
/// ```
/// /foo/bar
/// taco
/// 123abc
/// some.kinda.thing
/// ```
fn bare_string<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(
        recognize(many1(preceded(
            not(alt((recognize(whitespace), recognize(one_of("@{}|&;()"))))),
            anychar,
        ))),
        |x| Node::BareString(x.strip_parse_state()),
    )
    .parse(input)
}

/// Invocation is defined as:
///
/// ```
/// Invocation ←⊔ Identifier ( SimpleExpression / BareString )* / SimpleExpression
/// ```
///
/// Invocation looks like:
///
/// ```
/// a $b $c
/// foo.bar 16 -7
/// do_thing --my_arg 3
/// ```
fn invocation<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    alt((
        map(
            pair(
                completion_hint(identifier, TabHint::Invocable),
                many0(preceded(
                    whitespace,
                    completion_hint(
                        alt((
                            delimited(
                                not(tag(".")),
                                no_errors(simple_expression),
                                peek(alt((
                                    recognize(whitespace),
                                    recognize(one_of("|&;)}")),
                                    recognize(not(anychar)),
                                ))),
                            ),
                            bare_string,
                        )),
                        TabHint::CommandArgument,
                    ),
                )),
            ),
            |(first, args)| {
                Node::Invocation(
                    first.strip_parse_state(),
                    args.into_iter().map(Node::from).collect(),
                )
            },
        ),
        map(simple_expression, Node::from),
    ))
    .parse(input)
}

/// Assignment is defined as:
///
/// ```
/// Assignment ←⊔ ( SimpleExpression '=' )* Invocation
/// ```
///
/// Assignment looks like:
///
/// ```
/// $a = $b
/// $a = $b = $c
/// ```
fn assignment<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(pair(many0(terminated(simple_expression, ws_around(tag("=")))), invocation), |(a, b)| {
        a.into_iter().rev().fold(b, |b, a| Node::Assignment(Box::new(a.into()), Box::new(b)))
    })
    .parse(input)
}

/// Conditionals are defined as:
///
/// ```
/// If ←⊔ 'if' SimpleExpression Block ( 'else' ( If / Block ) )?
/// ```
///
/// Conditionals look like:
///
/// ```
/// if $a { b }
/// if $a { b } else { c }
/// if $a { b } else if $c { d } else { e }
/// ```
fn conditional<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(
        (
            kw("if"),
            ws_before(simple_expression),
            ws_before(block),
            opt(preceded(ws_around(kw("else")), alt((conditional, block)))),
        ),
        |(_, condition, body, else_)| Node::If {
            condition: Box::new(condition.into()),
            body: Box::new(body),
            else_: else_.map(Box::new),
        },
    )
    .parse(input)
}

/// Expressions are defined as follows:
///
/// ```
/// SimpleExpression ← Range
/// Expression ←⊔ ( Assignment ( '|>' / '|' ) )* Assignment
/// ```
///
/// Expressions look like:
///
/// ```
/// abc | def |> ghi
/// ```
fn expression<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    lassoc_choice(assignment, alt((tag("|>"), tag("|"))), |a, op, b| match *op.fragment() {
        "|>" => Node::Iterate(Box::new(a), Box::new(b)),
        "|" => Node::Pipe(Box::new(a), Box::new(b)),
        _ => unreachable!(),
    })
    .parse(input)
}

/// Identical to [`expression`] but injects an error if an expression doesn't parse here.
fn ex_expression<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    alt((expression, err_insert("Expected expression"))).parse(input)
}

/// See `expression`
fn simple_expression<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    range(input)
}

/// Import statemets are defined as follows:
///
/// ```
/// Import ←⊔ 'import' BareString ( 'as' ( Identifier / '$' UnescapedIdentifier ) )?
/// ```
///
/// Import statements look like:
///
/// ```
/// import /foo/bar
/// import /foo/bar/baz as baz_module
/// import /foo/bar/123abc as $123abc
/// ```
fn import<'a>(input: ESpan<'a>) -> IResult<'a, Node<'a>> {
    map(
        pair(
            preceded(kw("import"), ws_before(bare_string)),
            opt(preceded(
                ws_around(tag("as")),
                alt((identifier, preceded(ex_tag("$"), unescaped_identifier))),
            )),
        ),
        |(path, name)| {
            let Node::BareString(path) = path else {
                unreachable!();
            };
            Node::Import(path, name.map(|x| x.strip_parse_state()))
        },
    )
    .parse(input)
}

/// A program is defined as:
///
/// ```
/// Program ←⊔ ( ( ( Import / FunctionDecl ) ( [;&]? Program )? ) /
///     ( ( VariableDecl / ShortFunctionDecl / Expression ) ( [;&] Program )? ) )?
/// ```
fn program<'a>(input: ESpan<'a>) -> IResult<'a, Vec<Node<'a>>> {
    let mut input_next = input;
    let mut vec = Vec::new();

    fn program_item<'a>(
        is_first: bool,
    ) -> impl Parser<ESpan<'a>, Output = (Node<'a>, Option<char>), Error = Error> {
        move |input| {
            let defer_expr = DeferredError::new();
            let defer = defer_expr.defer(expression);
            let mut parser = preceded(
                cond(!is_first, opt(whitespace)),
                alt((
                    pair(
                        alt((import, function_decl)),
                        map(opt(ws_before(one_of(";&"))), |x| x.or(Some(';'))),
                    ),
                    pair(
                        alt((
                            variable_decl,
                            short_function_decl,
                            defer,
                            preceded(
                                peek(not(ws_before(alt((recognize(chr('}')), eof))))),
                                defer_expr.restore(),
                            ),
                        )),
                        opt(ws_before(one_of(";&"))),
                    ),
                )),
            );
            parser.parse(input)
        }
    }

    while let Ok((tail, (node, terminator))) =
        program_item(vec.is_empty()).parse(input_next.clone())
    {
        let node = if let Some('&') = terminator {
            if let Node::FunctionDecl { identifier, parameters, body } = node {
                Node::FunctionDecl { identifier, parameters, body: Box::new(Node::Async(body)) }
            } else {
                Node::Async(Box::new(node))
            }
        } else {
            node
        };

        input_next = tail;
        vec.push(node);
        if terminator.is_none() {
            break;
        }
    }

    Ok((input_next, vec))
}

#[cfg(test)]
mod test {
    use super::*;

    /// Quick helper for testing parsing.
    fn test_parse_err(
        text: &str,
        expected: Node<'_>,
        expected_errors: Vec<(usize, &'_ str, &'_ str)>,
    ) {
        let result = ParseResult::from(text);
        let actual = result.tree;
        let mut errors = result.errors;
        assert!(
            actual.content_eq(&expected),
            "Unexpected result\nexpected: {expected:#?}\ngot     : {actual:#?}\nerrors: {errors:?}",
        );

        let mut missing_errors = Vec::new();

        for expected_error in expected_errors {
            if let Some((idx, _)) = errors.iter().enumerate().find(|(_, (e, m))| {
                *m == expected_error.1
                    && e.location_offset() == expected_error.0
                    && *e.fragment() == expected_error.2
            }) {
                errors.remove(idx);
            } else {
                missing_errors.push(expected_error);
            }
        }

        assert!(
            errors.is_empty() && missing_errors.is_empty(),
            "Unexpected errors: {errors:#?}\nMissing errors: {missing_errors:#?}"
        );
    }

    /// Quick helper for testing parsing.
    fn test_parse(text: &str, expected: Node<'_>) {
        test_parse_err(text, expected, Vec::new())
    }

    /// Shorthand for `Span::new`
    fn sp(s: &str) -> Span<'_> {
        Span::new(s)
    }

    /// Quick constructor for an identifier node.
    fn ident(s: &str) -> Node<'_> {
        Node::Identifier(sp(s))
    }

    /// Quick constructor for a string literal node.
    fn string(s: &str) -> Node<'_> {
        Node::String(vec![StringElement::Body(sp(s))])
    }

    #[test]
    fn deferred() {
        fn parser<'a>(input: ESpan<'a>) -> IResult<'a, ESpan<'a>> {
            let defer = DeferredError::new();
            // This will never match just because we'll never try to parse what it matches. It's not actually a parser that never matches.
            let never_match = defer.defer(preceded(ex_tag("never_match"), tag("")));

            let mut parser = alt((tag("abc"), never_match, tag("def"), defer.restore()));
            parser.parse(input)
        }

        let abc = ESpan::new_extra("abc", (Rc::new(RefCell::new(ParseState::new())), None));
        let abc = parser(abc).unwrap();
        assert!(abc.0.fragment().is_empty());
        let abc = abc.1;
        assert!(*abc.fragment() == "abc");
        assert!(abc.extra.1.is_none());

        let def = ESpan::new_extra("def", (Rc::new(RefCell::new(ParseState::new())), None));
        let def = parser(def).unwrap();
        assert!(def.0.fragment().is_empty());
        let def = def.1;
        assert!(*def.fragment() == "def");
        assert!(def.extra.1.is_none());

        let ghi = ESpan::new_extra("ghi", (Rc::new(RefCell::new(ParseState::new())), None));
        let ghi = parser(ghi).unwrap();
        assert!(*ghi.0.fragment() == "ghi");
        let ghi = ghi.1;
        assert!(*ghi.fragment() == "");
        let error = ghi.extra.1.unwrap();
        assert!(error.next.is_none());
        assert!(error.span.fragment().is_empty());
        assert_eq!("Expected 'never_match'", error.msg);
    }

    #[test]
    fn variable_decl() {
        test_parse(
            "let s = 0",
            Node::Program(vec![Node::VariableDecl {
                identifier: sp("s"),
                value: Box::new(Node::Integer(sp("0"))),
                mutability: Mutability::Mutable,
            }]),
        );
    }

    #[test]
    fn labeled_object_one_field() {
        test_parse(
            r#"@Foo { bar: "baz" }"#,
            Node::Program(vec![Node::Object(
                Some(sp("Foo")),
                vec![(ident("bar"), string(r#"baz"#))],
            )]),
        );
    }

    #[test]
    fn labeled_empty_object() {
        test_parse(r#"@Foo {}"#, Node::Program(vec![Node::Object(Some(sp("Foo")), vec![])]));
    }

    #[test]
    fn label_but_no_object() {
        test_parse_err(
            r#"@Foo"#,
            Node::Program(vec![Node::Error]),
            vec![(4, "Expected object body after @Foo", "")],
        );
    }

    #[test]
    fn object_field_specified_with_identifier_missing_sigil() {
        test_parse_err(
            r#"@Foo { bar: a }"#,
            Node::Program(vec![Node::Object(Some(sp("Foo")), vec![(ident("bar"), ident("a"))])]),
            vec![(12, "Expected '$'", "")],
        );
    }

    #[test]
    fn object_field_specified_with_identifier_field_has_equals() {
        test_parse_err(
            r#"@Foo { bar = $a }"#,
            Node::Program(vec![Node::Object(Some(sp("Foo")), vec![(ident("bar"), ident("a"))])]),
            vec![(11, "Expected ':'", "=")],
        );
    }

    #[test]
    fn function_decl() {
        test_parse(
            r#"def foo() bar"#,
            Node::Program(vec![Node::FunctionDecl {
                identifier: sp("foo"),
                parameters: ParameterList {
                    parameters: vec![],
                    optional_parameters: vec![],
                    variadic: None,
                },
                body: Box::new(Node::Invocation(sp("bar"), vec![])),
            }]),
        );
    }

    #[test]
    fn function_decl_block() {
        test_parse(
            r#"def foo { bar }"#,
            Node::Program(vec![Node::FunctionDecl {
                identifier: sp("foo"),
                parameters: ParameterList {
                    parameters: vec![],
                    optional_parameters: vec![],
                    variadic: None,
                },
                body: Box::new(Node::Block(vec![Node::Invocation(sp("bar"), vec![])])),
            }]),
        );
    }

    #[test]
    fn function_decl_block_terminated() {
        test_parse(
            r#"def foo { bar };"#,
            Node::Program(vec![Node::FunctionDecl {
                identifier: sp("foo"),
                parameters: ParameterList {
                    parameters: vec![],
                    optional_parameters: vec![],
                    variadic: None,
                },
                body: Box::new(Node::Block(vec![Node::Invocation(sp("bar"), vec![])])),
            }]),
        );
    }

    #[test]
    fn function_decl_block_async() {
        test_parse(
            r#"def foo { bar }&"#,
            Node::Program(vec![Node::FunctionDecl {
                identifier: sp("foo"),
                parameters: ParameterList {
                    parameters: vec![],
                    optional_parameters: vec![],
                    variadic: None,
                },
                body: Box::new(Node::Async(Box::new(Node::Block(vec![Node::Invocation(
                    sp("bar"),
                    vec![],
                )])))),
            }]),
        );
    }

    #[test]
    fn function_decl_args() {
        test_parse(
            r#"def foo (a) bar"#,
            Node::Program(vec![Node::FunctionDecl {
                identifier: sp("foo"),
                parameters: ParameterList {
                    parameters: vec![sp("a")],
                    optional_parameters: vec![],
                    variadic: None,
                },
                body: Box::new(Node::Invocation(sp("bar"), vec![])),
            }]),
        );
    }

    #[test]
    fn function_decl_opt_args() {
        test_parse(
            r#"def foo (a?) bar"#,
            Node::Program(vec![Node::FunctionDecl {
                identifier: sp("foo"),
                parameters: ParameterList {
                    parameters: vec![],
                    optional_parameters: vec![sp("a")],
                    variadic: None,
                },
                body: Box::new(Node::Invocation(sp("bar"), vec![])),
            }]),
        );
    }

    #[test]
    fn function_decl_variadic_arg() {
        test_parse(
            r#"def foo (a..) bar"#,
            Node::Program(vec![Node::FunctionDecl {
                identifier: sp("foo"),
                parameters: ParameterList {
                    parameters: vec![],
                    optional_parameters: vec![],
                    variadic: Some(sp("a")),
                },
                body: Box::new(Node::Invocation(sp("bar"), vec![])),
            }]),
        );
    }

    #[test]
    fn function_decl_all_arg() {
        test_parse(
            r#"def foo (a b? c..) bar"#,
            Node::Program(vec![Node::FunctionDecl {
                identifier: sp("foo"),
                parameters: ParameterList {
                    parameters: vec![sp("a")],
                    optional_parameters: vec![sp("b")],
                    variadic: Some(sp("c")),
                },
                body: Box::new(Node::Invocation(sp("bar"), vec![])),
            }]),
        );
    }

    #[test]
    fn lambda() {
        test_parse(
            r#"\foo bar"#,
            Node::Program(vec![Node::Lambda {
                parameters: ParameterList {
                    parameters: vec![sp("foo")],
                    optional_parameters: vec![],
                    variadic: None,
                },
                body: Box::new(Node::Invocation(sp("bar"), vec![])),
            }]),
        );
    }

    #[test]
    fn lambda_fancy_args() {
        test_parse(
            r#"\(a b? c..) bar"#,
            Node::Program(vec![Node::Lambda {
                parameters: ParameterList {
                    parameters: vec![sp("a")],
                    optional_parameters: vec![sp("b")],
                    variadic: Some(sp("c")),
                },
                body: Box::new(Node::Invocation(sp("bar"), vec![])),
            }]),
        );
    }

    #[test]
    fn list() {
        test_parse(
            r#"[1, 2, 3]"#,
            Node::Program(vec![Node::List(vec![
                Node::Integer(sp("1")),
                Node::Integer(sp("2")),
                Node::Integer(sp("3")),
            ])]),
        );
    }

    #[test]
    fn fancy_integers() {
        test_parse(
            r#"[1_234_5, 0x2abcd, 3]"#,
            Node::Program(vec![Node::List(vec![
                Node::Integer(sp("1_234_5")),
                Node::Integer(sp("0x2abcd")),
                Node::Integer(sp("3")),
            ])]),
        );
    }

    #[test]
    fn reals() {
        test_parse(
            r#"[3.14, 1_23.4_5, 3_4.5_6]"#,
            Node::Program(vec![Node::List(vec![
                Node::Real(sp("3.14")),
                Node::Real(sp("1_23.4_5")),
                Node::Real(sp("3_4.5_6")),
            ])]),
        );
    }

    #[test]
    fn string_test() {
        test_parse(
            r#""$$\$straang\t\r\n\
\\abcd\u00264b\"""#,
            Node::Program(vec![Node::String(vec![
                StringElement::Body(sp("$")),
                StringElement::Body(sp("\\$straang\\t\\r\\n\\\n\\\\abcd\\u00264b\\\"")),
            ])]),
        );
    }

    #[test]
    fn pipes() {
        test_parse(
            r#"a | b |> c | d |> e"#,
            Node::Program(vec![Node::Iterate(
                Box::new(Node::Pipe(
                    Box::new(Node::Iterate(
                        Box::new(Node::Pipe(
                            Box::new(Node::Invocation(sp("a"), vec![])),
                            Box::new(Node::Invocation(sp("b"), vec![])),
                        )),
                        Box::new(Node::Invocation(sp("c"), vec![])),
                    )),
                    Box::new(Node::Invocation(sp("d"), vec![])),
                )),
                Box::new(Node::Invocation(sp("e"), vec![])),
            )]),
        );
    }

    #[test]
    fn paren_pipes() {
        test_parse(
            r#"q (a | b |> c | d |> e) $f"#,
            Node::Program(vec![Node::Invocation(
                sp("q"),
                vec![
                    Node::Iterate(
                        Box::new(Node::Pipe(
                            Box::new(Node::Iterate(
                                Box::new(Node::Pipe(
                                    Box::new(Node::Invocation(sp("a"), vec![])),
                                    Box::new(Node::Invocation(sp("b"), vec![])),
                                )),
                                Box::new(Node::Invocation(sp("c"), vec![])),
                            )),
                            Box::new(Node::Invocation(sp("d"), vec![])),
                        )),
                        Box::new(Node::Invocation(sp("e"), vec![])),
                    ),
                    Node::Identifier(sp("f")),
                ],
            )]),
        );
    }

    #[test]
    fn conditional_test() {
        test_parse(
            r#"if $a { b } else if $q { c } else { e }"#,
            Node::Program(vec![Node::If {
                condition: Box::new(Node::Identifier(sp("a"))),
                body: Box::new(Node::Block(vec![Node::Invocation(sp("b"), vec![])])),
                else_: Some(Box::new(Node::If {
                    condition: Box::new(Node::Identifier(sp("q"))),
                    body: Box::new(Node::Block(vec![Node::Invocation(sp("c"), vec![])])),
                    else_: Some(Box::new(Node::Block(vec![Node::Invocation(sp("e"), vec![])]))),
                })),
            }]),
        );
    }

    #[test]
    fn bare_string_starts_with_numbers() {
        test_parse(
            r#"foo 123abc"#,
            Node::Program(vec![Node::Invocation(sp("foo"), vec![Node::BareString(sp("123abc"))])]),
        );
    }

    #[test]
    fn args_that_start_with_dot_are_strings() {
        test_parse(
            r#"cd .."#,
            Node::Program(vec![Node::Invocation(sp("cd"), vec![Node::BareString(sp(".."))])]),
        );
    }

    #[test]
    fn lambda_with_variable() {
        test_parse(
            r#"\() { $fs_root }"#,
            Node::Program(vec![Node::Lambda {
                parameters: ParameterList {
                    parameters: vec![],
                    optional_parameters: vec![],
                    variadic: None,
                },
                body: Box::new(Node::Block(vec![Node::Identifier(sp("fs_root"))])),
            }]),
        );
    }

    #[test]
    fn invocable_starts_with_keyword() {
        test_parse(
            r#"imported_command"#,
            Node::Program(vec![Node::Invocation(sp("imported_command"), vec![])]),
        );
    }

    #[test]
    fn tab_complete_empty_start() {
        let result = ParseResult::from("  ");

        assert_eq!(1, result.tab_completions.len());
        let (key, completions) = result.tab_completions.last_key_value().unwrap();
        assert_eq!(2, *key);
        assert_eq!(1, completions.len());
        let TabHint::Invocable(arg) = &completions[0] else { panic!() };
        assert_eq!("", *arg.fragment());
        assert_eq!(1, result.whitespace.len());
        assert_eq!("  ", *result.whitespace[0].fragment());
    }

    #[test]
    fn tab_complete_cmd() {
        let result = ParseResult::from("  open /ab ");

        assert_eq!(3, result.tab_completions.len());
        let tab_completions = result
            .tab_completions
            .into_iter()
            .map(|(x, mut y)| {
                assert!(y.len() == 1);
                (x, y.pop().unwrap())
            })
            .collect::<Vec<_>>();
        assert_eq!(3, tab_completions.len());
        assert_eq!(2, tab_completions[0].0);
        assert_eq!(7, tab_completions[1].0);
        assert_eq!(11, tab_completions[2].0);
        let TabHint::Invocable(cmd) = &tab_completions[0].1 else { panic!() };
        assert_eq!("open", *cmd.fragment());
        let TabHint::CommandArgument(arg) = &tab_completions[1].1 else { panic!() };
        assert_eq!("/ab", *arg.fragment());
        let TabHint::CommandArgument(arg) = &tab_completions[2].1 else { panic!() };
        assert_eq!("", *arg.fragment());

        assert_eq!(3, result.whitespace.len());
        assert_eq!("  ", *result.whitespace[0].fragment());
        assert_eq!(" ", *result.whitespace[1].fragment());
        assert_eq!(" ", *result.whitespace[2].fragment());
    }

    #[test]
    fn string_interpolation() {
        test_parse(
            r#""I am $age years old""#,
            Node::Program(vec![Node::String(vec![
                StringElement::Body(sp("I am ")),
                StringElement::Interpolation(Node::Identifier(sp("age"))),
                StringElement::Body(sp(" years old")),
            ])]),
        );
    }

    #[test]
    fn string_interpolation_block() {
        test_parse(
            r#""I am ${ (get_age) - $vanity_adj } years old""#,
            Node::Program(vec![Node::String(vec![
                StringElement::Body(sp("I am ")),
                StringElement::Interpolation(Node::Subtract(
                    Box::new(Node::Invocation(sp("get_age"), vec![])),
                    Box::new(Node::Identifier(sp("vanity_adj"))),
                )),
                StringElement::Body(sp(" years old")),
            ])]),
        );
    }

    #[test]
    fn string_interpolation_err_end_of_string() {
        test_parse_err(
            r#""I am $""#,
            Node::Program(vec![Node::String(vec![
                StringElement::Body(sp("I am ")),
                StringElement::Interpolation(Node::Error),
            ])]),
            vec![(7, "Expected identifier, interpolated block, or $", "")],
        );
    }

    #[test]
    fn string_interpolation_err_space() {
        test_parse_err(
            r#""I am $ ""#,
            Node::Program(vec![Node::String(vec![
                StringElement::Body(sp("I am ")),
                StringElement::Interpolation(Node::Error),
                StringElement::Body(sp(" ")),
            ])]),
            vec![(7, "Expected identifier, interpolated block, or $", "")],
        );
    }

    #[test]
    fn string_interpolation_err_end_of_input() {
        test_parse_err(
            r#""I am $"#,
            Node::Program(vec![Node::String(vec![
                StringElement::Body(sp("I am ")),
                StringElement::Interpolation(Node::Error),
            ])]),
            vec![
                (7, "Expected identifier, interpolated block, or $", ""),
                (7, "Expected '\"'", ""),
            ],
        );
    }
}
