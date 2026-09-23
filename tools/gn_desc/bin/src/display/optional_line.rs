// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::{self, Display};

pub struct OptionalTitledLine<'a, T, L>
where
    T: Display,
    L: Display,
{
    pub title: T,
    pub line: &'a Option<L>,
}

impl<'a, T, L> OptionalTitledLine<'a, T, L>
where
    T: Display,
    L: Display,
{
    pub fn new(title: T, line: &'a Option<L>) -> Self {
        Self { title, line }
    }

    pub fn format(f: &mut fmt::Formatter<'_>, title: T, line: &'a Option<L>) -> fmt::Result {
        Self::new(title, line).fmt(f)
    }
}

impl<'a, T, L> Display for OptionalTitledLine<'a, T, L>
where
    T: Display,
    L: Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(line_value) = self.line {
            writeln!(f, "{}{}", self.title, line_value)?;
        }
        Ok(())
    }
}
