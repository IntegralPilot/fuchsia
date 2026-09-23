// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;
pub mod checks;
pub mod utils;

use anyhow::Result;
use args::StaticChecksCommand;
use cm_rust::ComponentDecl;
use fuchsia_archive::Utf8Reader;
use std::io::{Cursor, Write};

/// Entry-point for the command `ffx driver static-checks`.
pub async fn static_checks(cmd: StaticChecksCommand, writer: &mut dyn Write) -> Result<()> {
    let cursor = Cursor::new(std::fs::read(&cmd.driver)?);
    let mut far_reader = Utf8Reader::new(cursor)?;

    let driver_components = utils::find_driver_components(&mut far_reader)?;
    writeln!(
        writer,
        "Found {} in {}",
        utils::pluralize(driver_components.len() as u64, "driver component"),
        cmd.driver
    )?;

    let mut results = Vec::<(String, bool)>::new();
    for (driver_component, cm_path) in driver_components {
        writeln!(writer, "\n\nRunning static checks against '{}'", cm_path)?;

        let mut static_checker = StaticChecker::new(&mut far_reader, driver_component);

        static_checker.run_check(writer, "Bind rules are valid", checks::check_bind_rules)?;
        static_checker.run_check(
            writer,
            "Device categories are valid",
            checks::check_device_categories,
        )?;
        // Additional checks should be added here.

        if static_checker.checks_failed > 0 {
            results.push((cm_path, false));
        } else {
            results.push((cm_path, true));
        }
    }

    let mut total = 0;
    let mut total_failed = 0;
    write!(writer, "\n")?;
    for (cm_path, passed) in results {
        total += 1;
        if passed {
            writeln!(writer, "{}: {}", cm_path, utils::green("PASSED"))?;
        } else {
            total_failed += 1;
            writeln!(writer, "{}: {}", cm_path, utils::red("FAILED"))?;
        }
    }

    let mut failure_string = utils::pluralize(total_failed, "failure");
    if total_failed > 0 {
        failure_string = utils::red(&failure_string);
    } else {
        failure_string = utils::green(&failure_string);
    }

    writeln!(
        writer,
        "\nChecked {} with {}.",
        utils::cyan(&utils::pluralize(total, "driver")),
        failure_string,
    )?;

    Ok(())
}

struct StaticChecker<'a> {
    far_reader: &'a mut utils::FarReader,
    component: ComponentDecl,
    checks_passed: u64,
    checks_failed: u64,
}

impl<'a> StaticChecker<'a> {
    fn new(far_reader: &'a mut utils::FarReader, component: ComponentDecl) -> Self {
        Self { far_reader, component, checks_passed: 0, checks_failed: 0 }
    }

    fn run_check(
        &mut self,
        writer: &mut dyn Write,
        name: &str,
        f: checks::CheckFunction,
    ) -> Result<()> {
        writeln!(writer, "[RUNNING]\t{}", name)?;
        let result = f(self.far_reader, &self.component)?;
        match result {
            Ok(_) => {
                writeln!(writer, "[PASSED]\t{}", name)?;
                self.checks_passed += 1;
            }
            Err(checks::StaticCheckFail { reason }) => {
                writeln!(writer, "{} {}", utils::red("Error:"), reason)?;
                writeln!(writer, "[FAILED]\t{}", name)?;
                self.checks_failed += 1;
            }
        }
        Ok(())
    }
}
