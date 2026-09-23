// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builtins::file_utils::{builtin_cp, builtin_ls, builtin_mkdir, builtin_mv, builtin_rm};
use crate::builtins::run_builtin;
use crate::eval::{EvalOutcome, ExecutionContext, ShellState};
use bstr::BString;

fn to_bstr(p: &std::path::Path) -> BString {
    BString::from(p.to_str().unwrap())
}

#[test]
fn test_builtin_cp_and_mv_dash_semantics() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let temp_path = std::env::temp_dir().join("zxsh_test_cp_mv_dir");
    let _ = std::fs::remove_dir_all(&temp_path);
    std::fs::create_dir_all(&temp_path).unwrap();

    let src_path = temp_path.join("src.txt");
    let dst_path = temp_path.join("dst.txt");
    let sub_dir = temp_path.join("subdir");
    std::fs::create_dir(&sub_dir).unwrap();
    std::fs::write(&src_path, b"test data").unwrap();

    let src_bstr = to_bstr(&src_path);
    let dst_bstr = to_bstr(&dst_path);
    let sub_bstr = to_bstr(&sub_dir);

    // Test cp missing args -> usage error
    let res = run_builtin("cp", &[src_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // Test cp file to file
    let res =
        run_builtin("cp", &[src_bstr.clone(), dst_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(std::fs::read(&dst_path).unwrap(), b"test data");

    // Test cp file to directory
    let res =
        run_builtin("cp", &[src_bstr.clone(), sub_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert_eq!(std::fs::read(sub_dir.join("src.txt")).unwrap(), b"test data");

    // Test mv file to directory
    let res =
        run_builtin("mv", &[dst_bstr.clone(), sub_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(!dst_path.exists());
    assert_eq!(std::fs::read(sub_dir.join("dst.txt")).unwrap(), b"test data");

    // Test cp directory -> error (Recursive copy not supported)
    let res =
        run_builtin("cp", &[sub_bstr.clone(), dst_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // Test cp multiple sources to non-directory -> error
    let res = run_builtin(
        "cp",
        &[src_bstr.clone(), src_bstr.clone(), dst_bstr.clone()],
        &mut state,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    let _ = std::fs::remove_dir_all(&temp_path);
}

#[test]
fn test_builtin_ls_dash_semantics() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let temp_path = std::env::temp_dir().join("zxsh_test_ls_dir");
    let _ = std::fs::remove_dir_all(&temp_path);
    std::fs::create_dir_all(&temp_path).unwrap();

    let file1_path = temp_path.join("file1.txt");
    std::fs::write(&file1_path, b"hello").unwrap();

    let dir_bstr = to_bstr(&temp_path);
    let file_bstr = to_bstr(&file1_path);

    // Test ls on directory
    let res = run_builtin("ls", &[dir_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // Test ls -l on directory
    let res =
        run_builtin("ls", &[BString::from("-l"), dir_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // Test ls on single file
    let res = run_builtin("ls", &[file_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // Test ls with > 1 path argument -> usage error
    let res =
        run_builtin("ls", &[file_bstr.clone(), dir_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // Test ls on non-existent path
    let res =
        run_builtin("ls", &[BString::from("/nonexistent_path_xyz_123")], &mut state, &mut ctx)
            .unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    let _ = std::fs::remove_dir_all(&temp_path);
}

#[test]
fn test_builtin_mkdir_dash_semantics() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let temp_path = std::env::temp_dir().join("zxsh_test_mkdir_dir");
    let _ = std::fs::remove_dir_all(&temp_path);
    std::fs::create_dir_all(&temp_path).unwrap();

    let dir1_path = temp_path.join("dir1");
    let dir1_bstr = to_bstr(&dir1_path);

    // Test mkdir missing args -> usage error
    let res = run_builtin("mkdir", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // Test mkdir dir1
    let res = run_builtin("mkdir", &[dir1_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(dir1_path.is_dir());

    // Test mkdir -p dir2/sub1/sub2
    let nested_path = temp_path.join("dir2").join("sub1").join("sub2");
    let nested_bstr = to_bstr(&nested_path);
    let res =
        run_builtin("mkdir", &[BString::from("-p"), nested_bstr], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(nested_path.is_dir());

    // Test mkdir -p on existing dir
    let res =
        run_builtin("mkdir", &[BString::from("-p"), dir1_bstr], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    let _ = std::fs::remove_dir_all(&temp_path);
}

#[test]
fn test_builtin_rm_dash_semantics() {
    let mut state = ShellState::new();
    let mut ctx = ExecutionContext::initial().unwrap();

    let temp_path = std::env::temp_dir().join("zxsh_test_rm_dir");
    let _ = std::fs::remove_dir_all(&temp_path);
    std::fs::create_dir_all(&temp_path).unwrap();

    let file_path = temp_path.join("rm_test.txt");
    let dir_path = temp_path.join("rm_dir");
    let subfile_path = dir_path.join("inside.txt");

    std::fs::write(&file_path, b"data").unwrap();
    std::fs::create_dir(&dir_path).unwrap();
    std::fs::write(&subfile_path, b"nested").unwrap();

    let file_bstr = to_bstr(&file_path);
    let dir_bstr = to_bstr(&dir_path);

    // Test rm missing args -> usage error
    let res = run_builtin("rm", &[], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // Test rm invalid flag -> usage error
    let res = run_builtin("rm", &[BString::from("-z")], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // Test rm file
    let res = run_builtin("rm", &[file_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(!file_path.exists());

    // Test rm non-existent file without -f -> error
    let res = run_builtin("rm", &[file_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // Test rm non-existent file with -f -> success
    let res = run_builtin("rm", &[BString::from("-f"), file_bstr], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));

    // Test rm dir without -r -> error
    let res = run_builtin("rm", &[dir_bstr.clone()], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(1));

    // Test rm -r dir
    let res = run_builtin("rm", &[BString::from("-r"), dir_bstr], &mut state, &mut ctx).unwrap();
    assert_eq!(res, EvalOutcome::Code(0));
    assert!(!dir_path.exists());

    let _ = std::fs::remove_dir_all(&temp_path);
}

#[test]
fn test_ls_default_and_options_and_errors() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin = std::io::empty();

    // 0 args default to current directory "."
    let code = builtin_ls(&[], &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(code, 0);

    // invalid flag
    stderr.clear();
    let code = builtin_ls(&[BString::from("-z")], &mut state, &mut stdin, &mut stdout, &mut stderr);
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&stderr).contains("usage: ls"));

    // non-existent file
    stderr.clear();
    let code = builtin_ls(
        &[BString::from("/nonexistent_path_file_utils_test")],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);
    assert!(String::from_utf8_lossy(&stderr).contains("cannot stat"));
}

#[test]
fn test_cp_and_mv_advanced_options_and_errors() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin = std::io::empty();

    let temp_dir = std::env::temp_dir().join("zxsh_file_utils_adv_test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();

    let file1 = temp_dir.join("f1.txt");
    let file2 = temp_dir.join("f2.txt");
    let target_dir = temp_dir.join("target_dir");
    std::fs::write(&file1, b"hello").unwrap();
    std::fs::write(&file2, b"world").unwrap();
    std::fs::create_dir_all(&target_dir).unwrap();

    let f1_bstr = to_bstr(&file1);
    let f2_bstr = to_bstr(&file2);
    let target_bstr = to_bstr(&target_dir);

    // Multiple source copy to directory
    let code = builtin_cp(
        &[f1_bstr.clone(), f2_bstr.clone(), target_bstr.clone()],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert!(target_dir.join("f1.txt").exists());
    assert!(target_dir.join("f2.txt").exists());

    // Multiple source move to non-directory -> error
    let code = builtin_mv(
        &[
            to_bstr(&target_dir.join("f1.txt")),
            to_bstr(&target_dir.join("f2.txt")),
            f1_bstr.clone(),
        ],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);

    // cp with -f flag
    let code = builtin_cp(
        &[BString::from("-f"), f1_bstr.clone(), f2_bstr.clone()],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);

    // mv with -f flag
    let code = builtin_mv(
        &[BString::from("-f"), f1_bstr.clone(), f2_bstr.clone()],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);

    // invalid flag to cp
    let code = builtin_cp(
        &[BString::from("-z"), f1_bstr.clone(), f2_bstr.clone()],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);

    // non-existent file source
    let code = builtin_cp(
        &[BString::from("/nonexistent_src"), f2_bstr.clone()],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_mkdir_and_rm_flags_and_errors() {
    let mut state = ShellState::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin = std::io::empty();

    let temp_dir = std::env::temp_dir().join("zxsh_mkdir_rm_adv_test");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();

    let sub1 = temp_dir.join("sub1");
    let sub2 = temp_dir.join("sub2");
    let sub1_bstr = to_bstr(&sub1);
    let sub2_bstr = to_bstr(&sub2);

    // mkdir multiple folders
    let code = builtin_mkdir(
        &[sub1_bstr.clone(), sub2_bstr.clone()],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert!(sub1.is_dir());
    assert!(sub2.is_dir());

    // mkdir invalid flag
    let code = builtin_mkdir(
        &[BString::from("-z"), sub1_bstr.clone()],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 1);

    // rm with capital -R flag (recursive)
    let code = builtin_rm(
        &[BString::from("-R"), to_bstr(&temp_dir)],
        &mut state,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert!(!temp_dir.exists());
}
