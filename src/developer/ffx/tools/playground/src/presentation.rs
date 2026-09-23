// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crossterm::style::{Attribute, Color, StyledContent, style};
use futures::future::{FutureExt, LocalBoxFuture};
use futures::io::{AsyncWrite, AsyncWriteExt as _};
use futures::stream::StreamExt;
use playground::interpreter::Interpreter;
use playground::value::{PlaygroundValue, Value};
use std::collections::HashMap;
use std::io;
use unicode_width::UnicodeWidthStr;

use crate::term_util::{printed_length, sanitize_str};

/// Style text as a header.
fn header_style<'a, D>(val: D, depth: usize) -> StyledContent<D>
where
    D: 'a + std::fmt::Display + Clone,
{
    if depth % 2 == 0 {
        style(val).attribute(Attribute::Bold).with(Color::Black).on(Color::Yellow)
    } else {
        style(val).attribute(Attribute::Bold).with(Color::White).on(Color::DarkCyan)
    }
}

/// Given a list of values, determine if all of them are Value::Objects and if
/// they all have the same fields.
fn all_objects_same_fields(values: &[Value]) -> bool {
    if values.is_empty() {
        true
    } else if values.len() == 1 {
        matches!(&values[0], Value::Object(_))
    } else {
        values.array_windows().all(|[a, b]| {
            let Value::Object(a) = a else {
                return false;
            };
            let Value::Object(b) = b else {
                return false;
            };

            if a.len() != b.len() {
                return false;
            }

            let mut keys = a.iter().map(|(x, _)| x).collect::<std::collections::HashSet<_>>();

            b.iter().all(|(x, _)| keys.remove(x)) && keys.is_empty()
        })
    }
}

/// Records the number of lines and columns it will take to print a text string
/// out to the terminal.
struct TextDimensions {
    lines: usize,
    cols: usize,
}

impl TextDimensions {
    /// Get the text dimensions for a given string.
    fn for_text(s: &str) -> TextDimensions {
        let lines = s.lines().count();
        let cols = s.lines().map(|x| x.width()).max().unwrap_or(0);

        TextDimensions { lines, cols }
    }
}

/// Indicates whether a string representation of a value is a normal, linear
/// string or a fancy table made with ANSI art.
enum DisplayedType {
    Text,
    Table,
}

/// Display a list of values which came as the result of a command in a pretty way.
async fn display_result_list<'a, W: AsyncWrite + Unpin + 'a>(
    writer: &'a mut W,
    values: Vec<Value>,
    interpreter: &'a Interpreter,
    depth: usize,
) -> io::Result<(TextDimensions, DisplayedType)> {
    if values.is_empty() {
        writer.write_all(b"[]").await?;
        return Ok((TextDimensions { lines: 1, cols: 2 }, DisplayedType::Text));
    }

    if values.iter().all(|x| matches!(x, Value::U8(_))) {
        let string = values
            .iter()
            .map(|x| {
                let Value::U8(x) = x else { unreachable!() };
                *x
            })
            .collect::<Vec<_>>();
        let string = if let Ok(string) = String::from_utf8(string) {
            sanitize_str(&string)
        } else {
            sanitize_str(&format!("{}", Value::List(values)))
        };
        let dims = TextDimensions::for_text(&string);
        writer.write_all(string.as_bytes()).await?;
        return Ok((dims, DisplayedType::Text));
    }

    let values = if all_objects_same_fields(&values) {
        values
    } else {
        values
            .into_iter()
            .enumerate()
            .map(|(n, v)| {
                Value::Object(vec![
                    ("#".to_owned(), Value::String(format!("{n}"))),
                    ("".to_owned(), v),
                ])
            })
            .collect()
    };

    let mut columns = HashMap::new();
    let mut column_order = Vec::new();
    let row_count = values.len();

    for value in values {
        let Value::Object(value) = value else { unreachable!() };

        if column_order.is_empty() {
            column_order = value.iter().map(|x| sanitize_str(&x.0)).collect();
        }

        for (column, value) in value {
            let column = sanitize_str(&column);
            let mut value_text = Vec::new();
            let mut cursor = futures::io::Cursor::new(&mut value_text);
            let (dims, disp) =
                display_result_inner(&mut cursor, depth + 1, Ok(value), interpreter).await?;
            let value_text = String::from_utf8(value_text).unwrap();
            columns.entry(column).or_insert(Vec::new()).push((value_text, dims, disp));
        }
    }

    let mut header_height = 1;
    let mut column_lengths: HashMap<_, _> = column_order
        .iter()
        .map(|column| {
            let max =
                columns.get(column).unwrap().iter().map(|(_, dims, _)| dims.cols).max().unwrap();
            let column_dims = TextDimensions::for_text(column);
            let max = std::cmp::max(max, column_dims.cols);
            header_height = std::cmp::max(header_height, column_dims.lines);
            (column, max)
        })
        .collect();

    if let Some(last_column) = column_order.last() {
        let last_len =
            column_lengths.get_mut(last_column).expect("Column should be in lengths table!");
        *last_len += 1;
    }

    let mut header = column_order
        .iter()
        .map(|column| {
            let length = column_lengths.get(column).unwrap();
            column
                .lines()
                .chain(std::iter::repeat(""))
                .map(move |column| format!(" {column:<length$}"))
        })
        .collect::<Vec<_>>();

    let mut linebreak = "";
    let mut cols = 0;
    for _ in 0..header_height {
        let header = header.iter_mut().map(|x| x.next().unwrap()).collect::<Vec<_>>().join("  ");
        cols = header.lines().map(|x| x.width()).sum();
        let header = header_style(header, depth);
        writer.write_all(format!("{linebreak}{header}").as_bytes()).await?;
        linebreak = "\n";
    }

    let mut lines = header_height;
    let mut request_break = false;

    for row in 0..row_count {
        if std::mem::replace(&mut request_break, false) {
            let break_line = column_order
                .iter()
                .map(|column| {
                    let length = *column_lengths.get(column).unwrap();
                    format!(" {:<length$}", "")
                })
                .collect::<Vec<_>>()
                .join(" │");
            writer.write_all(format!("\n{break_line}").as_bytes()).await?;
            lines += 1;
        }

        let mut row_height = 1;
        let mut row = column_order
            .iter()
            .map(|column| {
                let (column_text, column_dims, displayed_type) = &columns.get(column).unwrap()[row];
                let length = *column_lengths.get(column).unwrap();
                request_break = request_break || matches!(displayed_type, DisplayedType::Table);
                let (left_gutter, right_gutter) = if matches!(displayed_type, DisplayedType::Text) {
                    (" ", "")
                } else {
                    ("", " ")
                };
                row_height = std::cmp::max(column_dims.lines, row_height);
                column_text.lines().chain(std::iter::repeat("")).map(move |column_text| {
                    let pad_length = length - printed_length(column_text);
                    format!("{left_gutter}{column_text}{:<pad_length$}{right_gutter}", "")
                })
            })
            .collect::<Vec<_>>();
        for _ in 0..row_height {
            let row = row.iter_mut().map(|x| x.next().unwrap()).collect::<Vec<_>>().join(" │");
            writer.write_all(format!("\n{row}").as_bytes()).await?;
            lines += 1;
        }
    }

    Ok((TextDimensions { lines, cols }, DisplayedType::Table))
}

/// Display a result from running a command in a prettified way.
pub async fn display_result<'a, W: AsyncWrite + Unpin + 'a>(
    writer: &'a mut W,
    result: playground::error::Result<Value>,
    interpreter: &'a Interpreter,
) -> io::Result<()> {
    display_result_inner(writer, 0, result, interpreter).await?;
    writer.write_all(b"\n").await?;
    Ok(())
}

/// Display a result from running a command in a prettified way.
fn display_result_inner<'a, W: AsyncWrite + Unpin + 'a>(
    writer: &'a mut W,
    depth: usize,
    result: playground::error::Result<Value>,
    interpreter: &'a Interpreter,
) -> LocalBoxFuture<'a, io::Result<(TextDimensions, DisplayedType)>> {
    async move {
        match result {
            Ok(Value::OutOfLine(PlaygroundValue::Iterator(x))) => {
                let stream = interpreter.replayable_iterator_to_stream(x);
                let values = stream
                    .collect::<Vec<_>>()
                    .await
                    .into_iter()
                    .collect::<playground::error::Result<Vec<_>>>();
                match values {
                    Ok(values) => display_result_list(writer, values, interpreter, depth).await,
                    Err(other) => display_result_inner(writer, 0, Err(other), interpreter).await,
                }
            }
            Ok(Value::List(values)) => {
                display_result_list(writer, values, interpreter, depth).await
            }
            Ok(Value::String(s)) => {
                let s = sanitize_str(&s);
                let ret = TextDimensions::for_text(&s);
                writer.write_all(s.as_bytes()).await?;
                Ok((ret, DisplayedType::Text))
            }
            Ok(Value::Object(items)) => {
                let items: Vec<_> =
                    items.into_iter().map(|(name, val)| (sanitize_str(&name), val)).collect();
                let name_width =
                    items.iter().map(|x| x.0.lines().map(|y| y.width()).sum()).max().unwrap_or(0)
                        + 1;
                let mut cols = 0;
                let mut lines = 0;
                for (name, value) in items {
                    if lines > 0 {
                        writer.write_all(b"\n").await?;
                    }

                    let mut value_text = Vec::new();
                    let name_dims = TextDimensions::for_text(&name);
                    let (value_dims, display_type) = display_result_inner(
                        &mut futures::io::Cursor::new(&mut value_text),
                        depth + 1,
                        Ok(value),
                        interpreter,
                    )
                    .await?;
                    let value = String::from_utf8(value_text).unwrap();
                    let left_gutter =
                        if matches!(display_type, DisplayedType::Text) { " " } else { "" };

                    cols =
                        std::cmp::max(cols, name_width + value_dims.cols + 1 + left_gutter.len());
                    let field_lines = std::cmp::max(name_dims.lines, value_dims.lines)
                        + if name_dims.lines > value_dims.lines { 1 } else { 0 };
                    lines += field_lines;

                    let name_lines = name.lines().chain(std::iter::repeat(""));
                    let value_lines = value.lines().chain(std::iter::repeat(""));
                    let break_before = std::iter::once("").chain(std::iter::repeat("\n"));
                    let by_line = name_lines.zip(value_lines).zip(break_before).take(field_lines);

                    for ((name_line, value_line), break_before) in by_line {
                        let name_line = header_style(format!("{name_line:>name_width$} "), depth);
                        writer
                            .write_all(
                                format!("{break_before}{name_line}{left_gutter}{value_line}")
                                    .as_bytes(),
                            )
                            .await?;
                    }
                }

                Ok((TextDimensions { lines, cols }, DisplayedType::Table))
            }
            Ok(x) => {
                let string = sanitize_str(&format!("{x}"));
                let ret = TextDimensions::for_text(&string);
                writer.write_all(string.as_bytes()).await?;
                Ok((ret, DisplayedType::Text))
            }
            Err(x) => {
                let content_string = sanitize_str(&format!("{x:#}"));
                let dim_string = format!("Err: {content_string}");
                let ret = TextDimensions::for_text(&dim_string);
                let string = format!(
                    "{} {}",
                    style("Err:").with(Color::Red).attribute(Attribute::Bold),
                    style(content_string).attribute(Attribute::Bold)
                );
                writer.write_all(string.as_bytes()).await?;
                Ok((ret, DisplayedType::Table))
            }
        }
    }
    .boxed_local()
}

#[cfg(test)]
mod test {
    use super::*;

    #[fuchsia::test]
    async fn table() {
        let mut got = Vec::<u8>::new();
        let ns = fidl_codec_fdomain::library::Namespace::new();
        let fs_root = fdomain_client::Channel::from(fdomain_client::Handle::invalid()).into();
        let (interpreter, task) = playground::interpreter::Interpreter::new(ns, fs_root).await;
        fuchsia_async::Task::spawn(task).detach();

        display_result(
            &mut got,
            Ok(Value::List(vec![
                Value::Object(vec![
                    ("a".to_owned(), Value::U8(1)),
                    ("b".to_owned(), Value::String("Hi".to_owned())),
                ]),
                Value::Object(vec![
                    ("a".to_owned(), Value::U8(2)),
                    ("b".to_owned(), Value::String("Hello".to_owned())),
                ]),
                Value::Object(vec![
                    ("a".to_owned(), Value::U8(3)),
                    ("b".to_owned(), Value::String("Good Evening".to_owned())),
                ]),
            ])),
            &interpreter,
        )
        .await
        .unwrap();
        let got = String::from_utf8(got).unwrap();
        let header = header_style(" a     b            ", 0);
        assert_eq!(
            format!("{header}\n 1u8 │ Hi           \n 2u8 │ Hello        \n 3u8 │ Good Evening \n"),
            got
        );
    }

    #[fuchsia::test]
    async fn short_list() {
        let mut got = Vec::<u8>::new();
        let ns = fidl_codec_fdomain::library::Namespace::new();
        let fs_root = fdomain_client::Channel::from(fdomain_client::Handle::invalid()).into();
        let (interpreter, task) = playground::interpreter::Interpreter::new(ns, fs_root).await;
        fuchsia_async::Task::spawn(task).detach();

        display_result(&mut got, Ok(Value::List(vec![Value::U32(2)])), &interpreter).await.unwrap();
        let got = String::from_utf8(got).unwrap();
        let header = header_style(" #        ", 0);
        assert_eq!(format!("{header}\n 0 │ 2u32 \n"), got);
    }

    #[fuchsia::test]
    async fn short_data() {
        let mut got = Vec::<u8>::new();
        let ns = fidl_codec_fdomain::library::Namespace::new();
        let fs_root = fdomain_client::Channel::from(fdomain_client::Handle::invalid()).into();
        let (interpreter, task) = playground::interpreter::Interpreter::new(ns, fs_root).await;
        fuchsia_async::Task::spawn(task).detach();

        display_result(
            &mut got,
            Ok(Value::List(vec![
                Value::Object(vec![
                    ("aa".to_owned(), Value::U8(1)),
                    ("bb".to_owned(), Value::U8(2)),
                ]),
                Value::Object(vec![
                    ("aa".to_owned(), Value::U8(2)),
                    ("bb".to_owned(), Value::U8(3)),
                ]),
                Value::Object(vec![
                    ("aa".to_owned(), Value::U8(3)),
                    ("bb".to_owned(), Value::U8(4)),
                ]),
            ])),
            &interpreter,
        )
        .await
        .unwrap();
        let got = String::from_utf8(got).unwrap();
        let header = header_style(" aa    bb  ", 0);
        assert_eq!(format!("{header}\n 1u8 │ 2u8 \n 2u8 │ 3u8 \n 3u8 │ 4u8 \n"), got);
    }

    #[fuchsia::test]
    async fn nested_object() {
        let mut got = Vec::<u8>::new();
        let ns = fidl_codec_fdomain::library::Namespace::new();
        let fs_root = fdomain_client::Channel::from(fdomain_client::Handle::invalid()).into();
        let (interpreter, task) = playground::interpreter::Interpreter::new(ns, fs_root).await;
        fuchsia_async::Task::spawn(task).detach();

        display_result(
            &mut got,
            Ok(Value::Object(vec![
                (
                    "aaaa\nbb".to_owned(),
                    Value::Object(vec![("qq".to_owned(), Value::String("rr".to_owned()))]),
                ),
                (
                    "cc".to_owned(),
                    Value::Object(vec![
                        ("qq".to_owned(), Value::String("rr".to_owned())),
                        ("ss".to_owned(), Value::String("tt\nuu\nvv".to_owned())),
                    ]),
                ),
            ])),
            &interpreter,
        )
        .await
        .unwrap();
        let got = String::from_utf8(got).unwrap();
        let aaaa = header_style("   aaaa ", 0);
        let qq = header_style(" qq ", 1);
        let bb = header_style("     bb ", 0);
        let blank = header_style("        ", 0);
        let cc = header_style("     cc ", 0);
        let ss = header_style(" ss ", 1);
        let blank2 = header_style("    ", 1);
        assert_eq!(
            format!(
                "{aaaa}{qq} rr\n{bb}\n{blank}\n{cc}{qq} rr\n{blank}{ss} tt\n{blank}{blank2} uu\n{blank}{blank2} vv\n"
            ),
            got
        );
    }

    #[fuchsia::test]
    async fn object_with_lists() {
        let mut got = Vec::<u8>::new();
        let ns = fidl_codec_fdomain::library::Namespace::new();
        let fs_root = fdomain_client::Channel::from(fdomain_client::Handle::invalid()).into();
        let (interpreter, task) = playground::interpreter::Interpreter::new(ns, fs_root).await;
        fuchsia_async::Task::spawn(task).detach();

        display_result(
            &mut got,
            Ok(Value::Object(vec![
                (
                    "aaaa\nbb".to_owned(),
                    Value::List(vec![Value::U16(1), Value::U16(2), Value::U16(3)]),
                ),
                ("cc".to_owned(), Value::List(vec![Value::U16(1), Value::U16(2), Value::U16(3)])),
            ])),
            &interpreter,
        )
        .await
        .unwrap();
        let got = String::from_utf8(got).unwrap();

        let aaaa = header_style("   aaaa ", 0);
        let num_header = header_style(" #        ", 1);
        let bb = header_style("     bb ", 0);
        let blank = header_style("        ", 0);
        let cc = header_style("     cc ", 0);
        assert_eq!(
            format!(
                "{aaaa}{num_header}\n{bb} 0 │ 1u16 \n{blank} 1 │ 2u16 \n{blank} 2 │ 3u16 \n{cc}{num_header}\n{blank} 0 │ 1u16 \n{blank} 1 │ 2u16 \n{blank} 2 │ 3u16 \n"
            ),
            got
        );
    }

    #[fuchsia::test]
    async fn list_of_lists_of_object() {
        let mut got = Vec::<u8>::new();
        let ns = fidl_codec_fdomain::library::Namespace::new();
        let fs_root = fdomain_client::Channel::from(fdomain_client::Handle::invalid()).into();
        let (interpreter, task) = playground::interpreter::Interpreter::new(ns, fs_root).await;
        fuchsia_async::Task::spawn(task).detach();

        display_result(
            &mut got,
            Ok(Value::List(vec![
                Value::Object(vec![
                    (
                        "a\nb".to_owned(),
                        Value::List(vec![
                            Value::Object(vec![
                                ("c".to_owned(), Value::U16(7)),
                                ("d".to_owned(), Value::U16(8)),
                            ]),
                            Value::Object(vec![
                                ("c".to_owned(), Value::U16(9)),
                                ("d".to_owned(), Value::U16(10)),
                            ]),
                        ]),
                    ),
                    (
                        "e".to_owned(),
                        Value::Object(vec![
                            ("f".to_owned(), Value::U16(11)),
                            ("g".to_owned(), Value::U16(12)),
                        ]),
                    ),
                ]),
                Value::Object(vec![
                    (
                        "a\nb".to_owned(),
                        Value::List(vec![
                            Value::Object(vec![
                                ("c".to_owned(), Value::U16(1)),
                                ("d".to_owned(), Value::U16(2)),
                            ]),
                            Value::Object(vec![
                                ("c".to_owned(), Value::U16(3)),
                                ("d".to_owned(), Value::U16(4)),
                            ]),
                        ]),
                    ),
                    (
                        "e".to_owned(),
                        Value::Object(vec![
                            ("f".to_owned(), Value::U16(5)),
                            ("g".to_owned(), Value::U16(6)),
                        ]),
                    ),
                ]),
            ])),
            &interpreter,
        )
        .await
        .unwrap();
        let got = String::from_utf8(got).unwrap();
        println!("{got}");

        let a_e = header_style(" a                e         ", 0);
        let b = header_style(" b                          ", 0);
        let c_d_1 = header_style(" c      d     ", 1);
        let c_d_2 = header_style(" c      d    ", 1);
        let f = header_style(" f ", 1);
        let g = header_style(" g ", 1);
        assert_eq!(
            format!(
                "{a_e}\n{b}\n{c_d_1}  │{f} 11u16  \n 7u16 │ 8u16    │{g} 12u16  \n 9u16 │ 10u16   │           \n                │           \n{c_d_2}   │{f} 5u16   \n 1u16 │ 2u16    │{g} 6u16   \n 3u16 │ 4u16    │           \n"
            ),
            got
        );
    }

    #[fuchsia::test]
    async fn string_sanitizes_escape_sequences() {
        let mut got = Vec::<u8>::new();
        let ns = fidl_codec_fdomain::library::Namespace::new();
        let fs_root = fdomain_client::Channel::from(fdomain_client::Handle::invalid()).into();
        let (interpreter, task) = playground::interpreter::Interpreter::new(ns, fs_root).await;
        fuchsia_async::Task::spawn(task).detach();

        display_result(
            &mut got,
            Ok(Value::String("\x1b[31mmalicious\x1b[0m\r\ntest".to_owned())),
            &interpreter,
        )
        .await
        .unwrap();

        let got = String::from_utf8(got).unwrap();
        assert_eq!(got, "\\u{1b}[31mmalicious\\u{1b}[0m\\r\ntest\n");
    }

    #[fuchsia::test]
    async fn u8_list_sanitizes_escape_sequences() {
        let mut got = Vec::<u8>::new();
        let ns = fidl_codec_fdomain::library::Namespace::new();
        let fs_root = fdomain_client::Channel::from(fdomain_client::Handle::invalid()).into();
        let (interpreter, task) = playground::interpreter::Interpreter::new(ns, fs_root).await;
        fuchsia_async::Task::spawn(task).detach();

        display_result(
            &mut got,
            Ok(Value::List(b"\x1b[6n".iter().map(|&b| Value::U8(b)).collect())),
            &interpreter,
        )
        .await
        .unwrap();

        let got = String::from_utf8(got).unwrap();
        assert_eq!(got, "\\u{1b}[6n\n");
    }

    #[fuchsia::test]
    async fn table_sanitizes_escape_sequences() {
        let mut got = Vec::<u8>::new();
        let ns = fidl_codec_fdomain::library::Namespace::new();
        let fs_root = fdomain_client::Channel::from(fdomain_client::Handle::invalid()).into();
        let (interpreter, task) = playground::interpreter::Interpreter::new(ns, fs_root).await;
        fuchsia_async::Task::spawn(task).detach();

        display_result(
            &mut got,
            Ok(Value::List(vec![Value::Object(vec![(
                "\x1b[31mheader\x1b[0m".to_owned(),
                Value::String("\x1b[32mvalue\x1b[0m".to_owned()),
            )])])),
            &interpreter,
        )
        .await
        .unwrap();

        let got = String::from_utf8(got).unwrap();
        let header = header_style(" \\u{1b}[31mheader\\u{1b}[0m ", 0);
        assert_eq!(format!("{header}\n \\u{{1b}}[32mvalue\\u{{1b}}[0m  \n"), got);
    }
}
