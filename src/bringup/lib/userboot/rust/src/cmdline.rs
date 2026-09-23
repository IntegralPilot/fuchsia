// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use std::borrow::Cow;
use std::fmt::Write;

/// Parses command-line arguments starting with "userboot", passing each key-value pair
/// to `parse_option` and logging recognized options or warnings.
pub fn parse_cmdline<T, F, W>(
    cmdline: &str,
    log: &mut W,
    opts: &mut T,
    mut parse_option: F,
) -> Result<(), Error>
where
    W: Write + ?Sized,
    F: FnMut(&str, &str, &mut T) -> bool,
{
    for raw_opt in cmdline.split_whitespace() {
        let opt = raw_opt.trim_matches('\0');
        if !opt.starts_with("userboot") {
            continue;
        }

        let (key, value) = match opt.split_once('=') {
            Some((k, v)) => (k, v),
            None => (opt, ""),
        };

        if !parse_option(key, value, opts) {
            writeln!(log, "WARNING: unknown option {key} ignored")?;
        } else if value.is_empty() {
            writeln!(log, "OPTION {key}")?;
        } else {
            writeln!(log, "OPTION {key}={value}")?;
        }
    }
    Ok(())
}

/// Information about the program to boot next.
#[derive(Default)]
pub struct ProgramInfo {
    /// Prefix directory path under bootfs.
    pub root: String,
    /// Next command line arguments (split by +).
    pub next: String,
}

impl ProgramInfo {
    /// Returns the program name and target path of the next command.
    pub fn filename(&self) -> (&str, Cow<'_, str>) {
        let name = if let Some(pos) = self.next.find('+') { &self.next[..pos] } else { &self.next };
        let path = if !self.root.is_empty() {
            format!("{}/{}", self.root, name).into()
        } else {
            name.into()
        };
        (name, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_parse_cmdline_valid_options() {
        let cmdline = "userboot.root=bootfs userboot.debugger non_userboot_option=123";
        let mut log = String::new();
        let mut parsed = HashMap::new();

        let result = parse_cmdline(cmdline, &mut log, &mut parsed, |key, val, map| {
            map.insert(key.to_string(), val.to_string());
            true
        });

        assert!(result.is_ok());
        assert_eq!(parsed.get("userboot.root"), Some(&"bootfs".to_string()));
        assert_eq!(parsed.get("userboot.debugger"), Some(&"".to_string()));
        assert!(!parsed.contains_key("non_userboot_option"));

        let expected_log = "OPTION userboot.root=bootfs\nOPTION userboot.debugger\n";
        assert_eq!(log, expected_log);
    }

    #[test]
    fn test_parse_cmdline_unknown_option() {
        let cmdline = "userboot.known=yes userboot.unknown=no";
        let mut log = String::new();
        let mut parsed = Vec::new();

        let result = parse_cmdline(cmdline, &mut log, &mut parsed, |key, val, vec| {
            if key == "userboot.known" {
                vec.push((key.to_string(), val.to_string()));
                true
            } else {
                false
            }
        });

        assert!(result.is_ok());
        assert_eq!(parsed, vec![("userboot.known".to_string(), "yes".to_string())]);

        let expected_log =
            "OPTION userboot.known=yes\nWARNING: unknown option userboot.unknown ignored\n";
        assert_eq!(log, expected_log);
    }

    #[test]
    fn test_parse_cmdline_null_bytes_and_whitespace() {
        let cmdline = "\0\0userboot.foo=bar\0\0   \0userboot.flag\0  ";
        let mut log = String::new();
        let mut parsed = HashMap::new();

        let result = parse_cmdline(cmdline, &mut log, &mut parsed, |key, val, map| {
            map.insert(key.to_string(), val.to_string());
            true
        });

        assert!(result.is_ok());
        assert_eq!(parsed.get("userboot.foo"), Some(&"bar".to_string()));
        assert_eq!(parsed.get("userboot.flag"), Some(&"".to_string()));

        let expected_log = "OPTION userboot.foo=bar\nOPTION userboot.flag\n";
        assert_eq!(log, expected_log);
    }

    #[test]
    fn test_parse_cmdline_empty() {
        let cmdline = "  ";
        let mut log = String::new();
        let mut count = 0;

        let result = parse_cmdline(cmdline, &mut log, &mut count, |_, _, cnt| {
            *cnt += 1;
            true
        });

        assert!(result.is_ok());
        assert_eq!(count, 0);
        assert!(log.is_empty());
    }

    #[test]
    fn test_parse_cmdline_multiple_equals() {
        let cmdline = "userboot.path=a=b=c";
        let mut log = String::new();
        let mut parsed = HashMap::new();

        let result = parse_cmdline(cmdline, &mut log, &mut parsed, |key, val, map| {
            map.insert(key.to_string(), val.to_string());
            true
        });

        assert!(result.is_ok());
        assert_eq!(parsed.get("userboot.path"), Some(&"a=b=c".to_string()));
        assert_eq!(log, "OPTION userboot.path=a=b=c\n");
    }

    #[test]
    fn test_program_info_filename() {
        let info_with_plus =
            ProgramInfo { root: "boot".to_string(), next: "bin/init+arg1+arg2".to_string() };
        assert_eq!(info_with_plus.filename(), ("bin/init", Cow::Borrowed("boot/bin/init")));

        let info_without_plus =
            ProgramInfo { root: "boot".to_string(), next: "bin/init".to_string() };
        assert_eq!(info_without_plus.filename(), ("bin/init", Cow::Borrowed("boot/bin/init")));

        let info_empty = ProgramInfo::default();
        assert_eq!(info_empty.filename(), ("", Cow::Borrowed("")));
    }
}
