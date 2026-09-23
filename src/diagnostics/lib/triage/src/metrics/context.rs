// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use nom::error::{ErrorKind, ParseError};
use nom::{AsBytes, Compare, CompareResult, Err, IResult, Input, Needed, Offset, ParseTo};
use std::num::NonZero;
use std::str::{CharIndices, Chars};

/// Parsing context used to store additional information.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParsingContext<'a> {
    /// The input of the parser.
    input: &'a str,
    // The current namespace.
    namespace: &'a str,
}

impl<'a> ParsingContext<'a> {
    pub fn new(input: &'a str, namespace: &'a str) -> Self {
        Self { input, namespace }
    }
    pub fn into_inner(self) -> &'a str {
        self.input
    }
}

impl AsBytes for ParsingContext<'_> {
    fn as_bytes(&self) -> &[u8] {
        self.input.as_bytes()
    }
}

impl<'a, T> Compare<T> for ParsingContext<'a>
where
    &'a str: Compare<T>,
{
    fn compare(&self, t: T) -> CompareResult {
        self.input.compare(t)
    }
    fn compare_no_case(&self, t: T) -> CompareResult {
        self.input.compare_no_case(t)
    }
}

impl<'a> Input for ParsingContext<'a> {
    type Item = char;
    type IterIndices = CharIndices<'a>;
    type Iter = Chars<'a>;

    fn iter_indices(&self) -> Self::IterIndices {
        self.input.char_indices()
    }

    fn iter_elements(&self) -> Self::Iter {
        self.input.chars()
    }

    fn position<P>(&self, predicate: P) -> Option<usize>
    where
        P: Fn(Self::Item) -> bool,
    {
        self.input.position(predicate)
    }

    fn slice_index(&self, count: usize) -> Result<usize, Needed> {
        self.input.slice_index(count)
    }

    fn input_len(&self) -> usize {
        self.input.len()
    }

    fn take(&self, count: usize) -> Self {
        Self::new(&self.input[..count], self.namespace)
    }

    fn take_from(&self, index: usize) -> Self {
        Self::new(&self.input[index..], self.namespace)
    }

    fn take_split(&self, count: usize) -> (Self, Self) {
        let (s0, s1) = self.input.split_at(count);
        (ParsingContext::new(s1, self.namespace), ParsingContext::new(s0, self.namespace))
    }

    fn split_at_position<P, E: ParseError<Self>>(&self, predicate: P) -> IResult<Self, Self, E>
    where
        P: Fn(Self::Item) -> bool,
    {
        self.input
            .position(predicate)
            .map(|idx| Self::take_split(self, idx))
            .ok_or(Err::Incomplete(Needed::Size(NonZero::new(1).unwrap())))
    }

    fn split_at_position1<P, E: ParseError<Self>>(
        &self,
        predicate: P,
        e: ErrorKind,
    ) -> IResult<Self, Self, E>
    where
        P: Fn(Self::Item) -> bool,
    {
        match self.input.position(predicate) {
            Some(0) => Err(Err::Error(E::from_error_kind(*self, e))),
            Some(idx) => Ok(Self::take_split(self, idx)),
            None => Err(Err::Incomplete(Needed::Size(NonZero::new(1).unwrap()))),
        }
    }

    fn split_at_position_complete<P, E: ParseError<Self>>(
        &self,
        predicate: P,
    ) -> IResult<Self, Self, E>
    where
        P: Fn(Self::Item) -> bool,
    {
        match self.split_at_position(predicate) {
            Err(Err::Incomplete(_)) => Ok(Self::take_split(self, self.input.input_len())),
            elt => elt,
        }
    }
    fn split_at_position1_complete<P, E: ParseError<Self>>(
        &self,
        predicate: P,
        e: ErrorKind,
    ) -> IResult<Self, Self, E>
    where
        P: Fn(Self::Item) -> bool,
    {
        match self.input.position(predicate) {
            Some(0) => Err(Err::Error(E::from_error_kind(*self, e))),
            Some(idx) => Ok(Self::take_split(self, idx)),
            None => Ok(Self::take_split(self, self.input.input_len())),
        }
    }
}

impl Offset for ParsingContext<'_> {
    fn offset(&self, second: &Self) -> usize {
        self.input.offset(second.input)
    }
}

impl<'a, R> ParseTo<R> for ParsingContext<'a>
where
    &'a str: ParseTo<R>,
{
    fn parse_to(&self) -> Option<R> {
        self.input.parse_to()
    }
}
