// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::eval::ShellPath;
use bstr::{BStr, BString, ByteSlice};
use std::os::unix::ffi::OsStrExt;

#[derive(Default)]
struct TokenInfo {
    start: usize,
    found_command: bool,
    in_env: bool,
}

fn tokenize_line(line: &[u8]) -> TokenInfo {
    let mut info = TokenInfo::default();
    let mut in_token = false;

    for (i, &ch) in line.iter().enumerate() {
        if ch == b' ' {
            info.start = i + 1;
            if in_token && !info.in_env {
                info.found_command = true;
            }
            in_token = false;
            info.in_env = false;
            continue;
        }
        in_token = true;
        info.in_env = info.in_env || ch == b'=';
    }
    info
}

fn complete_at_dir(
    dir_path: &BStr,
    line_prefix: &BStr,
    line_separator: &BStr,
    file_prefix: &BStr,
    comps: &mut Vec<BString>,
) {
    let dir_path_ref = match dir_path.to_path() {
        Ok(p) => p,
        Err(_) => return,
    };
    let dir = match std::fs::read_dir(dir_path_ref) {
        Ok(d) => d,
        Err(_) => return,
    };

    for entry in dir.flatten() {
        let name = entry.file_name();
        let name_bstr = BStr::new(name.as_bytes());

        if name_bstr == "." || name_bstr == ".." {
            continue;
        }

        if name_bstr.starts_with(file_prefix.as_bytes()) {
            let mut completion = BString::default();
            completion.extend_from_slice(line_prefix);
            completion.extend_from_slice(line_separator);
            completion.extend_from_slice(name_bstr);

            comps.push(completion);
        }
    }
}

/// Generates interactive tab-completion suggestions matching commands, variables, aliases, or
/// file paths.
pub fn tab_complete(line: &BStr, path: &ShellPath) -> Vec<BString> {
    let mut comps = Vec::new();
    let line_bytes = line.as_bytes();
    let token = tokenize_line(line_bytes);
    if token.in_env {
        return comps;
    }

    let token_bytes = &line_bytes[token.start..];
    let token_bstr = BStr::new(token_bytes);

    // Determine completion strategy based on slashes
    let slash_pos = token_bytes.iter().rposition(|&x| x == b'/');

    if let Some(pos) = slash_pos {
        // Case 1: Slash in the last token (e.g. "foo bar baz/quu")
        // Split token_bytes at the last slash
        let (dir_bytes, prefix_bytes) = token_bytes.split_at(pos);
        // prefix_bytes starts with '/'
        let prefix_bytes = &prefix_bytes[1..];

        let dir_bstr = if dir_bytes.is_empty() { BStr::new(b"/") } else { BStr::new(dir_bytes) };
        let prefix_bstr = BStr::new(prefix_bytes);

        // Construct line prefix
        let prefix_len = token.start + dir_bytes.len();
        let line_prefix = BStr::new(&line_bytes[..prefix_len]);

        complete_at_dir(dir_bstr, line_prefix, BStr::new(b"/"), prefix_bstr, &mut comps);
    } else {
        // No slash in the last token
        let file_prefix = token_bstr;
        if token.found_command {
            // Case 2: Argument completion (e.g. "foo bar ba")
            // We search current directory '.'
            // Prefix is the line up to the space before the last token
            if token.start > 0 && line_bytes[token.start - 1] == b' ' {
                let line_prefix = BStr::new(&line_bytes[..token.start - 1]);
                complete_at_dir(
                    BStr::new(b"."),
                    line_prefix,
                    BStr::new(b" "),
                    file_prefix,
                    &mut comps,
                );
            }
        } else {
            // Case 3: Command name completion (e.g. "fo")
            // Search directories in PATH
            for path_segment in path.entries() {
                complete_at_dir(
                    path_segment,
                    BStr::new(b""),
                    BStr::new(b""),
                    file_prefix,
                    &mut comps,
                );
            }
        }
    }
    comps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::ShellState;

    #[test]
    fn test_tokenize_line() {
        let t = tokenize_line(b"foo");
        assert_eq!(t.start, 0);
        assert_eq!(t.found_command, false);
        assert_eq!(t.in_env, false);

        let t = tokenize_line(b"foo bar");
        assert_eq!(t.start, 4);
        assert_eq!(t.found_command, true);
        assert_eq!(t.in_env, false);

        let t = tokenize_line(b"FOO=BAR");
        assert_eq!(t.start, 0);
        assert_eq!(t.found_command, false);
        assert_eq!(t.in_env, true);

        let t = tokenize_line(b"FOO=BAR baz");
        assert_eq!(t.start, 8);
        assert_eq!(t.found_command, false);
        assert_eq!(t.in_env, false);

        let t = tokenize_line(b"FOO=BAR baz quux");
        assert_eq!(t.start, 12);
        assert_eq!(t.found_command, true);
        assert_eq!(t.in_env, false);
    }

    #[test]
    fn test_tab_complete_in_env() {
        let path = ShellPath::default();
        assert!(tab_complete(BStr::new("FOO=BAR"), &path).is_empty());
    }

    #[test]
    fn test_complete_at_dir_invalid_and_nonexistent() {
        let mut comps = Vec::new();

        // Invalid path (fails to_path)
        complete_at_dir(
            BStr::new(b"\xFF\xFE"),
            BStr::new(""),
            BStr::new(""),
            BStr::new(""),
            &mut comps,
        );
        assert!(comps.is_empty());

        // Non-existent directory (fails read_dir)
        complete_at_dir(
            BStr::new("/nonexistent_dir_zxsh_12345"),
            BStr::new(""),
            BStr::new(""),
            BStr::new(""),
            &mut comps,
        );
        assert!(comps.is_empty());
    }

    #[test]
    fn test_tab_complete_slash_path() {
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir.join("zxsh_complete_test");
        let _ = std::fs::create_dir_all(&test_dir);
        let file1 = test_dir.join("alpha.txt");
        let file2 = test_dir.join("beta.txt");
        let _ = std::fs::write(&file1, "a");
        let _ = std::fs::write(&file2, "b");

        let dir_str = test_dir.to_str().unwrap();

        // Slash in last token: dir_str/al
        let line = format!("{}/al", dir_str);
        let path = ShellPath::default();
        let items = tab_complete(BStr::new(line.as_bytes()), &path);

        let expected = format!("{}/alpha.txt", dir_str);
        assert!(items.contains(&BString::from(expected)), "Items: {:?}", items);

        // Root slash /
        let _items2 = tab_complete(BStr::new("/al"), &path);

        let _ = std::fs::remove_file(file1);
        let _ = std::fs::remove_file(file2);
        let _ = std::fs::remove_dir(test_dir);
    }

    #[test]
    fn test_tab_complete_command_path() {
        let temp_dir = std::env::temp_dir();
        let bin_dir = temp_dir.join("zxsh_bin_test");
        let _ = std::fs::create_dir_all(&bin_dir);
        let cmd1 = bin_dir.join("mytool_one");
        let cmd2 = bin_dir.join("mytool_two");
        let cmd3 = bin_dir.join("other_cmd");
        let _ = std::fs::write(&cmd1, "1");
        let _ = std::fs::write(&cmd2, "2");
        let _ = std::fs::write(&cmd3, "3");

        let mut state = ShellState::new();
        state.set_var("PATH", bin_dir.to_str().unwrap());
        let path = state.path();
        let items = tab_complete(BStr::new("mytool_"), &path);

        assert_eq!(items.len(), 2, "Items: {:?}", items);
        assert!(items.contains(&BString::from("mytool_one")));
        assert!(items.contains(&BString::from("mytool_two")));

        let _ = std::fs::remove_file(cmd1);
        let _ = std::fs::remove_file(cmd2);
        let _ = std::fs::remove_file(cmd3);
        let _ = std::fs::remove_dir(bin_dir);
    }

    #[test]
    fn test_tab_complete_argument_space() {
        let path = ShellPath::default();
        let _ = tab_complete(BStr::new("echo "), &path);
    }
}
