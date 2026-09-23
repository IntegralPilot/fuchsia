// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::{self, Display};

pub struct TitledList<'a, T: Display, L: Display> {
    pub title: T,
    pub list: &'a Vec<L>,
    pub indent: usize,
}

impl<'a, T: Display, L: Display> TitledList<'a, T, L> {
    pub fn new(title: T, list: &'a Vec<L>, indent: usize) -> Self {
        Self { title, list, indent }
    }
}

impl<'a, T: Display, L: Display> Display for TitledList<'a, T, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.list.is_empty() {
            writeln!(f, "{}{}", " ".repeat(self.indent), self.title)?;
            IndentedList::new(self.list, self.indent + 2).fmt(f)?;
        }
        Ok(())
    }
}

pub struct IndentedList<'a, L: Display> {
    pub list: &'a Vec<L>,
    pub indent: usize,
}
impl<'a, L: Display> IndentedList<'a, L> {
    pub fn new(list: &'a Vec<L>, indent: usize) -> Self {
        Self { list, indent }
    }
}

impl<'a, L: Display> Display for IndentedList<'a, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let indent_str = " ".repeat(self.indent);
        for item in self.list {
            writeln!(f, "{}{}", indent_str, item)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    pub fn test_indented_list_empty() {
        let empty_list = Vec::<String>::new();
        let formatted = format!("{}", &IndentedList::new(&empty_list, 0));
        assert_eq!("", formatted);
    }

    #[test]
    pub fn test_indented_list_at_0() {
        let list = vec!["abc", "def", "ghi"];
        let formatted = format!("{}", &IndentedList::new(&list, 0));
        assert_eq!(
            r#"abc
def
ghi
"#,
            formatted
        );
    }

    #[test]
    pub fn test_indented_list_at_6() {
        let list = vec!["abc", "def", "ghi"];
        let formatted = format!("{}", &IndentedList::new(&list, 6));
        assert_eq!(
            r#"      abc
      def
      ghi
"#,
            formatted
        );
    }

    #[test]
    pub fn test_titled_list_empty() {
        let empty_list = Vec::<String>::new();
        let formatted = format!("{}", &TitledList::new("title", &empty_list, 0));
        assert_eq!("", formatted);
    }

    #[test]
    pub fn test_titled_list_at_0() {
        let list = vec!["abc", "def", "ghi"];
        let formatted = format!("{}", &TitledList::new("a title:", &list, 0));
        assert_eq!(
            r#"a title:
  abc
  def
  ghi
"#,
            formatted
        );
    }

    #[test]
    pub fn test_titled_list_at_6() {
        let list = vec!["abc", "def", "ghi"];
        let formatted = format!("{}", &TitledList::new("other title", &list, 6));
        assert_eq!(
            r#"      other title
        abc
        def
        ghi
"#,
            formatted
        );
    }
}
