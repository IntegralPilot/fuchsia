// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::{OptionItem, OptionParser};
use crate::eval::{EXIT_FAILURE, EXIT_SUCCESS, ShellState};
use crate::string::path_buf_to_bstring;
use bstr::{BStr, BString, ByteSlice};
use std::io::{Read, Write};
use std::os::fuchsia::fs::MetadataExt;

fn modestr(mode: u32) -> &'static str {
    let fmt = mode & (libc::S_IFMT as u32);
    if fmt == (libc::S_IFREG as u32) {
        "-"
    } else if fmt == (libc::S_IFCHR as u32) {
        "c"
    } else if fmt == (libc::S_IFBLK as u32) {
        "b"
    } else if fmt == (libc::S_IFDIR as u32) {
        "d"
    } else {
        "?"
    }
}

pub fn builtin_ls(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut parser = OptionParser::new(args);
    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'l', enable: true }) => {}
            _ => {
                let _ = writeln!(stderr, "usage: ls [ <file_or_directory> ]");
                return EXIT_FAILURE;
            }
        }
    }

    let remaining = parser.rest();

    let dirn_str = if remaining.is_empty() {
        BStr::new(b".")
    } else if remaining.len() == 1 {
        remaining[0].as_bstr()
    } else {
        let _ = writeln!(stderr, "usage: ls [ <file_or_directory> ]");
        return EXIT_FAILURE;
    };

    let dirn_path = match dirn_str.to_path() {
        Ok(p) => p,
        Err(_) => {
            let _ = writeln!(stderr, "error: cannot stat '{}'", dirn_str);
            return EXIT_FAILURE;
        }
    };

    let read_dir = match std::fs::read_dir(&dirn_path) {
        Ok(rd) => rd,
        Err(_) => {
            let metadata = match dirn_path.metadata() {
                Ok(m) => m,
                Err(_) => {
                    let _ = writeln!(stderr, "error: cannot stat '{}'", dirn_str);
                    return EXIT_FAILURE;
                }
            };
            let (mode, size) = (metadata.st_mode() as u32, metadata.st_size() as i64);
            let _ = writeln!(stdout, "{} {:>8} {}", modestr(mode), size, dirn_str);
            return EXIT_SUCCESS;
        }
    };

    for entry_res in read_dir {
        let de = match entry_res {
            Ok(e) => e,
            Err(_) => continue,
        };
        let de_name = de.file_name().to_string_lossy().into_owned();
        let child_path = dirn_path.join(de.file_name());
        let (mode, nlink, size) = match child_path.metadata() {
            Ok(m) => (m.st_mode() as u32, m.st_nlink() as u64, m.st_size() as i64),
            Err(_) => (0u32, 0u64, 0i64),
        };
        let _ = writeln!(stdout, "{} {:>2} {:>8} {}", modestr(mode), nlink, size, de_name);
    }

    EXIT_SUCCESS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandType {
    Cp,
    Mv,
}

impl CommandType {
    fn name(&self) -> &'static str {
        match self {
            CommandType::Cp => "cp",
            CommandType::Mv => "mv",
        }
    }
}

fn verify_file(
    cmd_type: CommandType,
    filename_str: &BString,
    stderr: &mut dyn Write,
) -> Option<std::fs::Metadata> {
    let path = match filename_str.to_path() {
        Ok(p) => p,
        Err(_) => {
            let _ = writeln!(stderr, "{}: Unable to stat {}", cmd_type.name(), filename_str);
            return None;
        }
    };
    let metadata = match path.metadata() {
        Ok(m) => m,
        Err(_) => {
            let _ = writeln!(stderr, "{}: Unable to stat {}", cmd_type.name(), filename_str);
            return None;
        }
    };

    if cmd_type == CommandType::Cp && metadata.is_dir() {
        let _ = writeln!(stderr, "cp: Recursive copy not supported");
        return None;
    }

    Some(metadata)
}

fn cp_here(
    src_str: &BString,
    dest_str: &BString,
    dest_path: &std::path::Path,
    _dest_exists: bool,
    force: bool,
    stderr: &mut dyn Write,
) -> i32 {
    if verify_file(CommandType::Cp, src_str, stderr).is_none() {
        return EXIT_FAILURE;
    }
    let src_path = match src_str.to_path() {
        Ok(p) => p,
        Err(_) => {
            let _ = writeln!(stderr, "cp: cannot open '{}'", src_str);
            return EXIT_FAILURE;
        }
    };

    let mut fdi = match std::fs::File::open(&src_path) {
        Ok(f) => f,
        Err(_) => {
            let _ = writeln!(stderr, "cp: cannot open '{}'", src_str);
            return EXIT_FAILURE;
        }
    };

    let fdo_res =
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(dest_path);

    let mut fdo = match fdo_res {
        Ok(f) => f,
        Err(_) => {
            if force {
                let _ = std::fs::remove_file(dest_path);
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(dest_path)
                {
                    Ok(f) => f,
                    Err(_) => {
                        let _ = writeln!(stderr, "cp: cannot open '{}'", dest_str);
                        return EXIT_FAILURE;
                    }
                }
            } else {
                let _ = writeln!(stderr, "cp: cannot open '{}'", dest_str);
                return EXIT_FAILURE;
            }
        }
    };

    let mut buf = [0u8; 4096];
    loop {
        let r = match fdi.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => {
                let _ = writeln!(stderr, "cp: failed reading from '{}'", src_str);
                return EXIT_FAILURE;
            }
        };
        if fdo.write_all(&buf[..r]).is_err() {
            let _ = writeln!(stderr, "cp: failed writing to '{}'", dest_str);
            return EXIT_FAILURE;
        }
    }

    EXIT_SUCCESS
}

fn mv_here(
    src_str: &BString,
    dest_str: &BString,
    dest_path: &std::path::Path,
    _dest_exists: bool,
    force: bool,
    stderr: &mut dyn Write,
) -> i32 {
    if verify_file(CommandType::Mv, src_str, stderr).is_none() {
        return EXIT_FAILURE;
    }
    let src_path = match src_str.to_path() {
        Ok(p) => p,
        Err(_) => {
            let _ = writeln!(stderr, "mv: failed to create '{}'", dest_str);
            return EXIT_FAILURE;
        }
    };

    if std::fs::rename(&src_path, dest_path).is_err() {
        if force {
            let _ = std::fs::remove_file(dest_path);
            if std::fs::rename(&src_path, dest_path).is_err() {
                let _ = writeln!(stderr, "mv: failed to create '{}'", dest_str);
                return EXIT_FAILURE;
            }
        } else {
            let _ = writeln!(stderr, "mv: failed to create '{}'", dest_str);
            return EXIT_FAILURE;
        }
    }

    EXIT_SUCCESS
}

fn mv_or_cp_to_dir(
    cmd_type: CommandType,
    src_str: &BString,
    dest_str: &BString,
    dest_path: &std::path::Path,
    force: bool,
    stderr: &mut dyn Write,
) -> i32 {
    if verify_file(cmd_type, src_str, stderr).is_none() {
        return EXIT_FAILURE;
    }

    let src_bytes = src_str.as_bytes();
    let filename_start = match src_bytes.iter().rposition(|&b| b == b'/') {
        Some(idx) => &src_bytes[idx + 1..],
        None => src_bytes,
    };

    if filename_start.is_empty() {
        let _ = writeln!(stderr, "{}: Invalid filename \"{}\"", cmd_type.name(), src_str);
        return EXIT_FAILURE;
    }

    if dest_str.is_empty() {
        let _ = writeln!(stderr, "{}: Invalid filename \"{}\"", cmd_type.name(), dest_str);
        return EXIT_FAILURE;
    }

    let full_filename_path = match BStr::new(filename_start).to_path() {
        Ok(p) => dest_path.join(p),
        Err(_) => {
            let _ = writeln!(stderr, "{}: Invalid filename \"{}\"", cmd_type.name(), src_str);
            return EXIT_FAILURE;
        }
    };

    let full_filename_bstr = path_buf_to_bstring(full_filename_path.clone()).unwrap_or_default();
    let dest_exists = full_filename_path.exists();

    match cmd_type {
        CommandType::Mv => {
            mv_here(src_str, &full_filename_bstr, &full_filename_path, dest_exists, force, stderr)
        }
        CommandType::Cp => {
            cp_here(src_str, &full_filename_bstr, &full_filename_path, dest_exists, force, stderr)
        }
    }
}

fn builtin_mv_or_cp(cmd_type: CommandType, args: &[BString], stderr: &mut dyn Write) -> i32 {
    let mut parser = OptionParser::new(args);
    let mut force = false;

    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'f', .. }) => force = true,
            _ => {
                let _ = writeln!(stderr, "usage: {} [-f] <src>... <dst>", cmd_type.name());
                return EXIT_FAILURE;
            }
        }
    }

    let positional = parser.rest();
    if positional.len() < 2 {
        let _ = writeln!(stderr, "usage: {} [-f] <src>... <dst>", cmd_type.name());
        return EXIT_FAILURE;
    }

    let sources = &positional[..positional.len() - 1];
    let dest_str = &positional[positional.len() - 1];

    let dest_path = match dest_str.to_path() {
        Ok(p) => p,
        Err(_) => {
            let _ = writeln!(stderr, "usage: {} [-f] <src>... <dst>", cmd_type.name());
            return EXIT_FAILURE;
        }
    };

    let dest_metadata = dest_path.metadata().ok();
    let dest_exists = dest_metadata.is_some();
    let dest_isdir = dest_metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);

    if dest_isdir {
        for src_str in sources {
            let res = mv_or_cp_to_dir(cmd_type, src_str, dest_str, &dest_path, force, stderr);
            if res != 0 {
                return res;
            }
        }
        EXIT_SUCCESS
    } else if sources.len() > 1 {
        let _ = writeln!(stderr, "{}: destination is not a directory", cmd_type.name());
        EXIT_FAILURE
    } else {
        match cmd_type {
            CommandType::Mv => {
                mv_here(&sources[0], dest_str, &dest_path, dest_exists, force, stderr)
            }
            CommandType::Cp => {
                cp_here(&sources[0], dest_str, &dest_path, dest_exists, force, stderr)
            }
        }
    }
}

pub fn builtin_cp(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    builtin_mv_or_cp(CommandType::Cp, args, stderr)
}

pub fn builtin_mv(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    builtin_mv_or_cp(CommandType::Mv, args, stderr)
}

pub fn builtin_mkdir(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut parser = OptionParser::new(args);
    let mut parents = false;

    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'p', enable: true }) => parents = true,
            _ => {
                let _ = writeln!(stderr, "usage: mkdir <path>");
                return EXIT_FAILURE;
            }
        }
    }

    let remaining = parser.rest();
    if remaining.is_empty() {
        let _ = writeln!(stderr, "usage: mkdir <path>");
        return EXIT_FAILURE;
    }

    for dir_str in remaining {
        let dir_bytes = dir_str.as_bytes();
        if parents {
            for slash in 1..dir_bytes.len() {
                if dir_bytes[slash] == b'/' {
                    let sub_bstr = BStr::new(&dir_bytes[..slash]);
                    if let Ok(sub_path) = sub_bstr.to_path() {
                        match std::fs::create_dir(&sub_path) {
                            Ok(_) => {}
                            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                            Err(_) => {
                                let _ = writeln!(
                                    stderr,
                                    "error: failed to make directory '{}'",
                                    sub_bstr
                                );
                                return EXIT_SUCCESS;
                            }
                        }
                    }
                }
            }
        }

        let path = match dir_str.to_path() {
            Ok(p) => p,
            Err(_) => {
                let _ = writeln!(stderr, "error: failed to make directory '{}'", dir_str);
                continue;
            }
        };

        match std::fs::create_dir(&path) {
            Ok(_) => {}
            Err(e) if parents && e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => {
                let _ = writeln!(stderr, "error: failed to make directory '{}'", dir_str);
            }
        }
    }

    EXIT_SUCCESS
}

fn rm_recursive(path: &std::path::Path, force: bool) -> i32 {
    let metadata = match path.symlink_metadata() {
        Ok(m) => m,
        Err(e) => {
            if force && e.kind() == std::io::ErrorKind::NotFound {
                return EXIT_SUCCESS;
            } else {
                return EXIT_FAILURE;
            }
        }
    };

    if metadata.is_dir() {
        let read_dir = match std::fs::read_dir(path) {
            Ok(rd) => rd,
            Err(_) => return EXIT_FAILURE,
        };
        for entry_res in read_dir {
            let entry = match entry_res {
                Ok(e) => e,
                Err(_) => return EXIT_FAILURE,
            };
            let entry_path = entry.path();
            if rm_recursive(&entry_path, force) != EXIT_SUCCESS {
                return EXIT_FAILURE;
            }
        }
        if std::fs::remove_dir(path).is_err() {
            return EXIT_FAILURE;
        }
    } else {
        if std::fs::remove_file(path).is_err() {
            return EXIT_FAILURE;
        }
    }
    EXIT_SUCCESS
}

pub fn builtin_rm(
    args: &[BString],
    _env: &mut ShellState,
    _stdin: &mut dyn Read,
    _stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut parser = OptionParser::new(args);
    let mut recursive = false;
    let mut force = false;

    while let Some(opt_res) = parser.next_option(|_| false) {
        match opt_res {
            Ok(OptionItem::Flag { flag: b'r' | b'R', .. }) => recursive = true,
            Ok(OptionItem::Flag { flag: b'f', .. }) => force = true,
            _ => {
                let _ = writeln!(stderr, "usage: rm [-frR]... <filename>...");
                return EXIT_FAILURE;
            }
        }
    }

    let targets = parser.rest();
    if targets.is_empty() {
        let _ = writeln!(stderr, "usage: rm [-frR]... <filename>...");
        return EXIT_FAILURE;
    }

    for target_str in targets {
        let path = match target_str.to_path() {
            Ok(p) => p,
            Err(_) => {
                let _ = writeln!(stderr, "error: failed to delete '{}'", target_str);
                return EXIT_FAILURE;
            }
        };

        if recursive {
            if rm_recursive(&path, force) != 0 {
                let _ = writeln!(stderr, "error: failed to delete '{}'", target_str);
                return EXIT_FAILURE;
            }
        } else {
            match std::fs::remove_file(&path) {
                Ok(_) => {}
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound && force {
                        // ignore ENOENT if force
                    } else {
                        let _ = writeln!(stderr, "error: failed to delete '{}'", target_str);
                        return EXIT_FAILURE;
                    }
                }
            }
        }
    }

    EXIT_SUCCESS
}
