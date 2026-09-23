// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `cmc` is the Component Manifest Compiler.

use anyhow::{Error, ensure};
use cml::types::document::Document;
use cml::{OfferToAllCapability, error, features};
use reference_doc::MarkdownReferenceDocGenerator;
use std::path::{Path, PathBuf};
use std::{fs, io};

mod compile;
mod debug_print_cm;
mod format;
mod include;
mod merge;
pub mod opts;
mod reference;
mod util;

pub fn run_cmc(opt: opts::Opt) -> Result<(), Error> {
    match opt.cmd {
        opts::Commands::ValidateReferences { component_manifest, package_manifest, context } => {
            reference::validate(&component_manifest, &package_manifest, context.as_ref())?
        }
        opts::Commands::Merge { files, output, fromfile, depfile } => {
            merge::merge(files, output, fromfile, depfile)?
        }
        opts::Commands::Include {
            file,
            output,
            depfile,
            includepath,
            includeroot,
            validate,
            features,
        } => {
            path_exists(&file)?;
            include::merge_includes(
                &file,
                output.as_ref(),
                depfile.as_ref(),
                &includepath,
                &includeroot,
                validate,
                &features.into(),
            )?
        }
        opts::Commands::CheckIncludes {
            file,
            expected_includes,
            fromfile,
            depfile,
            includepath,
            includeroot,
        } => {
            path_exists(&file)?;
            optional_path_exists(fromfile.as_ref())?;
            include::check_includes(
                &file,
                expected_includes,
                fromfile.as_ref(),
                depfile.as_ref(),
                opt.stamp.as_ref(),
                &includepath,
                &includeroot,
            )?
        }
        opts::Commands::Format { file, pretty, cml, inplace, mut output } => {
            // TODO(https://fxbug.dev/42060365): stop accepting these flags.
            let _pretty = pretty;
            let _cml = cml;

            let input = if let Some(file) = &file {
                path_exists(&file)?;
                if inplace {
                    output = Some(file.clone());
                }
                format::Input::File(file)
            } else {
                if inplace {
                    return Err(error::Error::invalid_args("--inplace (-i) requires a file").into());
                }
                format::Input::Stdin(io::stdin().lock())
            };
            format::format(input, output)?;
        }
        opts::Commands::Compile {
            file,
            output,
            depfile,
            includepath,
            includeroot,
            config_package_path,
            features,
            experimental_force_runner,
            must_offer_protocol,
            must_use_protocol,
            must_offer_dictionary,
        } => {
            path_exists(&file)?;
            compile::compile(
                &file,
                &output,
                depfile,
                &includepath,
                &includeroot,
                config_package_path.as_ref().map(String::as_str),
                &features.into(),
                &experimental_force_runner,
                cml::CapabilityRequirements {
                    must_offer: &must_offer_protocol
                        .iter()
                        .map(|value| cml::OfferToAllCapability::Protocol(value))
                        .chain(
                            must_offer_dictionary
                                .iter()
                                .map(|value| OfferToAllCapability::Dictionary(value)),
                        )
                        .collect::<Vec<_>>(),
                    must_use: &must_use_protocol
                        .iter()
                        .map(|value| cml::MustUseRequirement::Protocol(value))
                        .collect::<Vec<_>>(),
                },
            )?
        }
        opts::Commands::PrintReferenceDocs { output } => {
            let docs = Document::get_reference_doc_markdown();
            match &output {
                None => println!("{}", docs),
                Some(path) => fs::write(path, docs)?,
            }
        }
        opts::Commands::DebugPrintCm { file } => {
            path_exists(&file)?;
            debug_print_cm::debug_print_cm(&file)?
        }
    }
    if let Some(stamp_path) = opt.stamp {
        stamp(stamp_path)?;
    }
    Ok(())
}

fn path_exists(path: &Path) -> Result<(), Error> {
    ensure!(path.exists(), "{:?} does not exist", path);
    Ok(())
}

fn optional_path_exists(optional_path: Option<&PathBuf>) -> Result<(), Error> {
    if let Some(path) = optional_path.as_ref() {
        ensure!(path.exists(), "{:?} does not exist", path);
    }
    Ok(())
}

fn stamp(stamp_path: PathBuf) -> Result<(), Error> {
    fs::File::create(stamp_path)?;
    Ok(())
}
