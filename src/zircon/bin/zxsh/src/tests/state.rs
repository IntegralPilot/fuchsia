// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::collections::FlatMap;
use crate::eval::testing::Frame;
use crate::eval::{ExecutionContext, ShellState};
use crate::fd::Fd;
use bstr::{BStr, BString, ByteSlice};

#[test]
fn test_env_basic_variables() {
    let mut state = ShellState::new();
    assert_eq!(state.get_var(BStr::new("VAR")), None);

    state.set_var(BStr::new("VAR"), BStr::new("value"));
    assert_eq!(state.get_var(BStr::new("VAR")), Some(BString::from("value")));

    state.unset_var(BStr::new("VAR"));
    assert_eq!(state.get_var(BStr::new("VAR")), None);
}

#[test]
fn test_env_readonly_variables() {
    let mut state = ShellState::new();
    state.set_var(BStr::new("VAR"), BStr::new("value"));
    assert!(!state.is_readonly(BStr::new("VAR")));

    state.make_readonly(BStr::new("VAR"));
    assert!(state.is_readonly(BStr::new("VAR")));

    // Try to overwrite readonly variable (should be ignored)
    state.set_var(BStr::new("VAR"), BStr::new("new_value"));
    assert_eq!(state.get_var(BStr::new("VAR")), Some(BString::from("value")));

    // Try to unset readonly variable (should be ignored)
    state.unset_var(BStr::new("VAR"));
    assert_eq!(state.get_var(BStr::new("VAR")), Some(BString::from("value")));
}

#[test]
fn test_env_functions() {
    let mut state = ShellState::new();
    assert_eq!(state.get_function(BStr::new("my_func")), None);

    let body = vec![1, 2, 3]; // Dummy serialized body
    state.add_function(BString::from("my_func"), body.clone());
    assert_eq!(state.get_function(BStr::new("my_func")), Some(&body));

    let removed = state.remove_function(BStr::new("my_func"));
    assert_eq!(removed, Some(body));
    assert_eq!(state.get_function(BStr::new("my_func")), None);
}

#[test]
fn test_env_aliases() {
    let mut state = ShellState::new();
    assert!(!state.aliases.contains_key(BStr::new("ll")));

    state.aliases.insert(BString::from("ll"), BString::from("ls -l"));
    assert_eq!(state.aliases.get(BStr::new("ll")), Some(&BString::from("ls -l")));
}

#[test]
fn test_internal_fd_leak() {
    let _env = ShellState::new();
    let ctx = ExecutionContext::initial().unwrap();
    use std::os::fd::AsRawFd;
    let stdout_phys = ctx.stdout().unwrap().as_raw_fd();
    let res = ctx.dup_fd(Fd(stdout_phys));
    assert!(res.is_err());
}

#[test]
fn test_state_non_utf8_handling() {
    let mut state = ShellState::new();

    // 1. Non-UTF8 global variable name and value
    let var_name = BStr::new(b"VAR_\xFF\xFE");
    let var_val = BStr::new(b"VAL_\x80\x81");
    state.set_var(var_name, var_val);
    assert_eq!(state.get_var(var_name), Some(BString::from(var_val)));

    // 2. Non-UTF8 alias
    let alias_name = BString::from(b"cmd_\xFF");
    let alias_val = BString::from(b"echo \xFE\xFD");
    state.aliases.insert(alias_name.clone(), alias_val.clone());
    assert_eq!(state.aliases.get(alias_name.as_bstr()), Some(&alias_val));

    // 3. Non-UTF8 function name and binary body
    let func_name = BString::from(b"func_\xDE\xAD");
    let func_body = vec![0xFF, 0x00, 0xFE, 0x01];
    state.add_function(func_name.clone(), func_body.clone());
    assert_eq!(state.get_function(func_name.as_bstr()), Some(&func_body));

    // 4. Non-UTF8 positional arguments
    let args = vec![BString::from(b"arg1_\x80"), BString::from(b"arg2_\x90")];
    state.set_args(args.clone());
    assert_eq!(state.get_args(), args);

    // 5. Non-UTF8 local variables within a function frame
    state.frames.push(Frame { local_vars: FlatMap::new(), args: vec![] });
    let local_name = BStr::new(b"LOCAL_\xAA\xBB");
    let local_val = BStr::new(b"LOCAL_VAL_\xCC\xDD");
    state.declare_local(local_name, Some(local_val));
    assert_eq!(state.get_var(local_name), Some(BString::from(local_val)));
}

#[test]
fn test_shell_env() {
    let mut state = ShellState::new();
    state.set_and_export_var(BStr::new("PATH"), BStr::new("/custom/bin:/another/bin"));
    state.set_and_export_var(BStr::new("FOO"), BStr::new("bar"));

    let env = state.vars();
    assert_eq!(env.path().entries().count(), 2);

    let cstrings = env.to_spawn_env().expect("to_spawn_env failed");
    let cstr_strs: Vec<_> = cstrings.iter().map(|s| s.to_str().unwrap()).collect();
    assert!(cstr_strs.contains(&"FOO=bar"));
    assert!(cstr_strs.contains(&"PATH=/custom/bin:/another/bin"));
}

#[test]
fn test_variable_values_with_equals() {
    let mut state = ShellState::new();
    state.set_var(BStr::new("FOO"), BStr::new("bar=baz=qux"));
    assert_eq!(state.get_var(BStr::new("FOO")), Some(BString::from("bar=baz=qux")));

    state.export_var(BStr::new("FOO"));
    let env = state.vars();
    let cstrings = env.to_spawn_env().expect("to_spawn_env failed");
    let cstr_strs: Vec<_> = cstrings.iter().map(|s| s.to_str().unwrap()).collect();
    assert!(cstr_strs.contains(&"FOO=bar=baz=qux"));

    // Test local variable values with equals
    state.frames.push(Frame { local_vars: FlatMap::new(), args: vec![] });
    state.declare_local(BStr::new("LOCAL_VAR"), Some(BStr::new("a=b=c=d")));
    assert_eq!(state.get_var(BStr::new("LOCAL_VAR")), Some(BString::from("a=b=c=d")));
}

#[test]
#[should_panic(expected = "name cannot contain '='")]
fn test_set_var_with_equals_panics() {
    let mut state = ShellState::new();
    state.set_var(BStr::new("A=B"), BStr::new("val"));
}

#[test]
#[should_panic(expected = "name cannot contain '='")]
fn test_export_var_with_equals_panics() {
    let mut state = ShellState::new();
    state.export_var(BStr::new("A=B"));
}

#[test]
#[should_panic(expected = "name cannot contain '='")]
fn test_unset_var_with_equals_panics() {
    let mut state = ShellState::new();
    state.unset_var(BStr::new("A=B"));
}

#[test]
#[should_panic(expected = "name cannot contain '='")]
fn test_readonly_var_with_equals_panics() {
    let mut state = ShellState::new();
    state.make_readonly(BStr::new("A=B"));
}

#[test]
#[should_panic(expected = "name cannot contain '='")]
fn test_declare_local_with_equals_panics() {
    let mut state = ShellState::new();
    state.frames.push(Frame { local_vars: FlatMap::new(), args: vec![] });
    state.declare_local(BStr::new("A=B"), Some(BStr::new("val")));
}

#[test]
#[should_panic(expected = "name cannot contain '='")]
fn test_add_function_with_equals_panics() {
    let mut state = ShellState::new();
    state.add_function(BString::from("A=B"), vec![]);
}

#[test]
#[should_panic(expected = "name cannot contain '='")]
fn test_shell_env_new_with_equals_panics() {
    use crate::eval::ShellEnv;
    let _ = ShellEnv::new(vec![(BString::from("A=B"), BString::from("val"))]);
}

#[test]
fn test_default_shell_variables_align_with_dash() {
    let state = ShellState::new();
    assert_eq!(state.get_var(BStr::new("PATH")), Some(BString::from("/bin:/boot/bin:/boot-bin")));
    assert!(!state.exported().contains(BStr::new("PATH")));

    assert!(state.get_var(BStr::new("PWD")).is_some());
    assert!(state.exported().contains(BStr::new("PWD")));

    assert_eq!(state.get_var(BStr::new("IFS")), Some(BString::from(" \t\n")));
    assert!(!state.exported().contains(BStr::new("IFS")));

    assert_eq!(state.get_var(BStr::new("OPTIND")), Some(BString::from("1")));
    assert!(!state.exported().contains(BStr::new("OPTIND")));

    assert!(state.get_var(BStr::new("PPID")).is_some());
    assert!(state.is_readonly(BStr::new("PPID")));
    assert!(!state.exported().contains(BStr::new("PPID")));

    let expected_ps1 = if unsafe { libc::geteuid() } == 0 { "# " } else { "$ " };
    assert_eq!(state.get_var(BStr::new("PS1")), Some(BString::from(expected_ps1)));
    assert_eq!(state.get_var(BStr::new("PS2")), Some(BString::from("> ")));
    assert_eq!(state.get_var(BStr::new("PS4")), Some(BString::from("+ ")));
}
