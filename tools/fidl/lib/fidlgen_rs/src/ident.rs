// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_ir::Ident;

pub trait SplitIdent {
    fn split(&self) -> Split<'_>;
}

impl SplitIdent for Ident {
    fn split(&self) -> Split<'_> {
        Split { str: self.non_canonical() }
    }
}

pub struct Split<'a> {
    str: &'a str,
}

impl<'a> Iterator for Split<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let mut char_indices = self.str.char_indices().skip_while(|(_, c)| *c == '_').peekable();

        let (start, mut prev) = char_indices.next()?;
        let mut end = self.str.len();

        while let Some((index, current)) = char_indices.next() {
            if current == '_' {
                end = index;
                break;
            }

            let prev_lower = prev.is_ascii_lowercase();
            let prev_digit = prev.is_ascii_digit();
            let current_upper = current.is_ascii_uppercase();
            let next_lower = char_indices.peek().is_some_and(|(_, c)| c.is_ascii_lowercase());

            let is_first_uppercase = (prev_lower || prev_digit) && current_upper;
            let is_last_uppercase = current_upper && next_lower;
            if is_first_uppercase || is_last_uppercase {
                end = index;
                break;
            }

            prev = current;
        }

        let result = &self.str[start..end];
        self.str = &self.str[end..];
        Some(result)
    }
}
