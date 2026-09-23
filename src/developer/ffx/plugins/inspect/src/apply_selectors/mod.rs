// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::HostArchiveReader;
use crate::apply_selectors::filter::filter_data_to_lines;
use crate::apply_selectors::screen::{Line, Screen};
use crate::apply_selectors::terminal::{Terminal, Termion};
use anyhow::{Context, Result, anyhow};
use diagnostics_data::{ExtendedMoniker, Inspect, InspectData};
use errors::ffx_error;
use fdomain_fuchsia_developer_remotecontrol::RemoteControlProxy;
use fdomain_fuchsia_diagnostics_host::ArchiveAccessorProxy;
use ffx_inspect_args::ApplySelectorsCommand;
use std::fs::read_to_string;
use std::io::{stdin, stdout};
use std::path::Path;
use termion::event::{Event, Key};
use termion::input::TermRead;
use termion::raw::IntoRawMode;

mod filter;
pub(crate) mod screen;
pub(crate) mod terminal;

#[cfg(test)]
mod test_utils;

pub async fn execute(
    rcs_proxy: RemoteControlProxy,
    diagnostics_proxy: ArchiveAccessorProxy,
    cmd: ApplySelectorsCommand,
) -> Result<()> {
    // Get full inspect data
    // If a snapshot file (inspect.json) is provided we use it to get inspect data,
    // else we use DiagnosticsProvider to get snapshot data.
    let inspect_data = if let Some(snapshot_file) = &cmd.snapshot_file {
        serde_json::from_str(
            &read_to_string(snapshot_file)
                .context(format!("Unable to read {}.", snapshot_file.display()))?,
        )
        .context(format!("Unable to deserialize {}.", snapshot_file.display()))?
    } else {
        let realm_query = rcs::root_realm_query(&rcs_proxy, std::time::Duration::from_secs(15))
            .await
            .map_err(|e| anyhow!(ffx_error!("Failed to connect to realm query: {e}")))?;
        let provider = HostArchiveReader::new(diagnostics_proxy, realm_query);
        provider
            .snapshot_diagnostics_data::<Inspect>(cmd.accessor_path.as_deref(), std::iter::empty())
            .await?
    };
    let moniker = match cmd.moniker {
        Some(m) => Some(ExtendedMoniker::parse_str(&m)?),
        None => None,
    };
    interactive_apply(&cmd.selector_file, &inspect_data, moniker)?;

    Ok(())
}

fn interactive_apply(
    selector_file: &Path,
    data: &[InspectData],
    requested_moniker: Option<ExtendedMoniker>,
) -> Result<()> {
    let stdin = stdin();
    let stdout = stdout().into_raw_mode().context("Unable to convert terminal to raw mode.")?;

    let mut screen = Screen::new(
        Termion::new(stdout),
        filter_data_to_lines(selector_file, data, requested_moniker.as_ref())?,
    );

    screen.terminal.switch_interactive();
    screen.refresh_screen_and_flush();

    for c in stdin.events() {
        let evt = c.unwrap();
        let should_update = match evt {
            Event::Key(Key::Char('q') | Key::Ctrl('c') | Key::Ctrl('d')) => break,
            Event::Key(Key::Char('h')) => {
                screen.set_filter_removed(!screen.filter_removed);
                screen.clear_screen();
                true
            }
            Event::Key(Key::Char('r')) => {
                screen.set_lines(vec![Line::new("Refressing filtered hierarchiers...")]);
                screen.clear_screen();
                screen.refresh_screen_and_flush();
                screen.set_lines(filter_data_to_lines(
                    selector_file,
                    data,
                    requested_moniker.as_ref(),
                )?);
                true
            }
            Event::Key(Key::PageUp) => screen.scroll(-screen.max_lines(), 0),
            Event::Key(Key::PageDown) => screen.scroll(screen.max_lines(), 0),
            Event::Key(Key::Up) => screen.scroll(-1, 0),
            Event::Key(Key::Down) => screen.scroll(1, 0),
            Event::Key(Key::Left) => screen.scroll(0, -1),
            Event::Key(Key::Right) => screen.scroll(0, 1),
            _e => false,
        };
        if should_update {
            screen.refresh_screen_and_flush();
        }
    }

    screen.terminal.switch_normal();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply_selectors::test_utils::FakeTerminal;
    use crate::tests::utils::get_v1_json_dump;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[fuchsia::test]
    fn hide_filtered_selectors() {
        let selectors = "realm1/realm2/session5/account_manager:root/accounts:total
realm1/realm2/session5/account_manager:root/listeners:total_opened
realm1/realm2/session5/account_manager:root/listeners:active";

        let mut selector_file =
            NamedTempFile::new().expect("Unable to create tempfile for testing.");
        selector_file
            .write_all(selectors.as_bytes())
            .expect("Unable to write selectors to tempfile.");

        let data: Vec<InspectData> =
            serde_json::from_value(get_v1_json_dump()).expect("Unable to parse Inspect Data.");

        let fake_terminal = FakeTerminal::new(90, 30);
        let mut screen = Screen::new(
            fake_terminal.clone(),
            filter_data_to_lines(&selector_file.path(), &data, None)
                .expect("Unable to filter hierarchy."),
        );

        screen.refresh_screen_and_flush();

        screen.set_filter_removed(true);
        screen.clear_screen();
        screen.refresh_screen_and_flush();

        // The output should not contain these selectors
        // realm1/realm2/session5/account_manager:root/accounts:active
        // realm1/realm2/session5/account_manager:root/auth_providers:types
        // realm1/realm2/session5/account_manager:root/listeners:events

        let screen_output = fake_terminal.screen_without_help_footer();

        assert_eq!(
            screen_output.contains("\"active\": 0"),
            false,
            "{} contains: '\"active\": 0' but it was expected to be filtered.",
            &screen_output
        );

        assert_eq!(
            screen_output.contains("auth_providers"),
            false,
            "{} contains: 'auth_providers' but it was expected to be filtered.",
            &screen_output
        );

        assert_eq!(
            screen_output.contains("\"events\": 0"),
            false,
            "{} contains: '\"events\": 0' but it was expected to be filtered.",
            &screen_output
        );
    }
}
