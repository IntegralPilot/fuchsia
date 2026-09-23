// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use auth::{AuthError, AuthFlowChoice, mint_new_access_token};
use ffx_writer::SimpleWriter;
use fho::Result;
use std::fs::File;
use std::io::{Write, stderr, stdin, stdout};
use std::os::unix::fs::PermissionsExt;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "generate",
    description = "Generates authorization credentials for private Fuchsia artifacts",
    example = "ffx auth generate"
)]
pub struct GenerateCommand {
    /// choice of authentication flow, defaults to pkce which should be acceptable for most users.
    #[argh(option, default = "AuthFlowChoice::Default")]
    pub auth: AuthFlowChoice,
}

const TOKEN_FILENAME: &str = "fuchsia_access_token";

fn get_persistent_file() -> Result<File, AuthError> {
    let persistent_file_path = std::path::Path::new(&std::env::temp_dir()).join(TOKEN_FILENAME);

    let mut options = std::fs::OpenOptions::new();
    options.truncate(true);
    options.create(true);
    options.write(true);
    options.open(persistent_file_path).map_err(|e| AuthError::IoError(e))
}

pub async fn generate(cmd: &GenerateCommand, _writer: SimpleWriter) -> Result<()> {
    let mut stdin = stdin();
    let mut stdout = stdout();
    let mut stderr = stderr();
    let ui = structured_ui::TextUi::new(&mut stdin, &mut stdout, &mut stderr);

    let access_token = mint_new_access_token(&cmd.auth, &ui).await?;

    let mut file = get_persistent_file()?;
    file.write_all(access_token.as_bytes()).map_err(|e| AuthError::IoError(e))?;
    let mut perms = file.metadata().map_err(|e| AuthError::IoError(e))?.permissions();
    perms.set_mode(0o600);
    file.set_permissions(perms).map_err(|e| AuthError::IoError(e))?;

    Ok(())
}
