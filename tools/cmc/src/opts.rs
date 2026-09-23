// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use clap::{Parser, Subcommand};
use cml::features::Feature;
use std::path::PathBuf;

#[derive(Parser, Debug)]
/// Tool for assembly, compilation, and validation of component manifests.
pub struct Opt {
    #[arg(short = 's', long = "stamp")]
    /// Stamp this file on success
    pub stamp: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    #[command(name = "validate-references")]
    /// validate a component manifest against package manifest.
    ValidateReferences {
        #[arg(name = "Component Manifest", short = 'c', long = "component-manifest")]
        component_manifest: PathBuf,

        #[arg(name = "Package Manifest", short = 'p', long = "package-manifest")]
        package_manifest: PathBuf,

        #[arg(
            name = "Free text label, for instance as context for errors printed",
            short = 'e',
            long = "context"
        )]
        context: Option<String>,
    },

    #[command(name = "merge")]
    /// merge the listed manifest files. Does NOT validate the resulting manifest.
    ///
    /// The semantics for merging are the same ones used for `include`:
    /// https://fuchsia.dev/reference/cml#include
    Merge {
        #[arg(name = "FILE")]
        /// files to process
        ///
        /// If any file contains an array at its root, every object in the array
        /// will be merged into the final object.
        files: Vec<PathBuf>,

        #[arg(short = 'o', long = "output")]
        /// file to write the merged results to, will print to stdout if not provided
        output: Option<PathBuf>,

        #[arg(short = 'f', long = "fromfile")]
        /// response file for files to process
        ///
        /// If specified, additional files to merge will be read from the path provided.
        /// The input format is delimited by newlines.
        fromfile: Option<PathBuf>,

        #[arg(short = 'd', long = "depfile")]
        /// depfile for includes
        ///
        /// If specified, include paths will be listed here, delimited by newlines.
        depfile: Option<PathBuf>,
    },

    #[command(name = "include")]
    /// recursively process contents from includes, and optionally validate the result
    Include {
        #[arg(name = "FILE")]
        /// file to process
        file: PathBuf,

        #[arg(short = 'o', long = "output")]
        /// file to write the merged results to, will print to stdout if not provided
        output: Option<PathBuf>,

        #[arg(short = 'd', long = "depfile")]
        /// depfile for includes
        ///
        /// If specified, include paths will be listed here, delimited by newlines.
        depfile: Option<PathBuf>,

        #[arg(short = 'p', long = "includepath", value_delimiter = ' ', num_args=1..)]
        /// base paths for resolving includes
        includepath: Vec<PathBuf>,

        #[arg(short = 'r', long = "includeroot", default_value = ".")]
        /// base path for resolving include paths that start with "//"
        includeroot: PathBuf,

        #[arg(long = "validate", action = clap::ArgAction::Set, default_value_t = true)]
        /// validate the result
        validate: bool,

        #[arg(short = 'f', long = "features", value_delimiter = ' ', num_args=1..)]
        /// The set of non-standard features to compile with.
        /// Only applies to CML files.
        features: Vec<Feature>,
    },

    #[command(name = "check-includes")]
    /// check if given includes are present in a given component manifest
    CheckIncludes {
        #[arg(name = "FILE")]
        /// file to process
        file: PathBuf,

        #[arg(name = "expect")]
        expected_includes: Vec<String>,

        #[arg(short = 'f', long = "fromfile")]
        /// response file for includes to expect
        ///
        /// If specified, additional includes to expect will be read from the path provided.
        /// The input format is delimited by newlines.
        fromfile: Option<PathBuf>,

        #[arg(short = 'd', long = "depfile")]
        /// depfile for includes
        ///
        /// If specified, include paths will be listed here, delimited by newlines.
        depfile: Option<PathBuf>,

        #[arg(short = 'p', long = "includepath", value_delimiter = ' ', num_args=1..)]
        /// base paths for resolving includes
        includepath: Vec<PathBuf>,

        #[arg(short = 'r', long = "includeroot", default_value = "")]
        /// base path for resolving include paths that start with "//"
        includeroot: PathBuf,
    },

    #[command(name = "format")]
    /// format a json file
    Format {
        #[arg(name = "FILE")]
        /// file to format. If missing, use stdin
        file: Option<PathBuf>,

        #[arg(short = 'p', long = "pretty")]
        /// deprecated and ignored. Please do not use (https://fxbug.dev/42060365).
        pretty: bool,

        #[arg(long = "cml")]
        /// deprecated and ignored. Please do not use (https://fxbug.dev/42060365).
        cml: bool,

        #[arg(short = 'i', long = "in-place")]
        /// replace the input file with the formatted output (implies `--output <inputfile>`)
        inplace: bool,

        #[arg(short = 'o', long = "output")]
        /// file to write the formatted results to, will print to stdout if not provided
        output: Option<PathBuf>,
    },

    #[command(name = "compile")]
    /// compile a CML file
    Compile {
        #[arg(name = "FILE")]
        /// file to format
        file: PathBuf,

        #[arg(short = 'o', long = "output")]
        /// file to write the formatted results to
        output: PathBuf,

        #[arg(short = 'd', long = "depfile")]
        /// depfile for includes
        ///
        /// If specified, include paths will be listed here, delimited by newlines.
        depfile: Option<PathBuf>,

        #[arg(short = 'p', long = "includepath", value_delimiter = ' ', num_args=1..)]
        /// base paths for resolving includes
        includepath: Vec<PathBuf>,

        #[arg(short = 'r', long = "includeroot", default_value = ".")]
        /// base path for resolving include paths that start with "//"
        includeroot: PathBuf,

        #[arg(long = "config-package-path")]
        /// path within the component's package at which its configuration will be available
        config_package_path: Option<String>,

        #[arg(short = 'f', long = "features", value_delimiter = ' ', num_args=1..)]
        /// The set of non-standard features to compile with.
        /// Only applies to CML files.
        features: Vec<Feature>,

        #[arg(long = "experimental-force-runner")]
        /// override runner to this value in resulting CML
        ///
        /// If specified, the program.runner field will be set to this value. This option is
        /// EXPERIMENTAL and subject to removal without warning.
        experimental_force_runner: Option<String>,

        #[arg(long = "must-offer-protocol")]
        /// protocols to verify that all children and collections are offered
        ///
        /// If specified, for each offer named, cmc will require that all children or collections
        /// in `files` have been offered a capability named for the offer specified.  This can be
        /// used to help find missing offers of important capabilities, like fuchsia.logger.LogSink
        must_offer_protocol: Vec<String>,

        #[arg(long = "must-use-protocol")]
        /// protocols to verify that all children and collections are used
        ///
        /// If specified, for each offer named, cmc will require that the offer is in a use block.
        /// This can be used to help find missing usages of important capabilities, like
        /// fuchsia.logger.LogSink
        must_use_protocol: Vec<String>,
        #[arg(long = "must-offer-dictionary")]
        /// dictionaries to verify that all children and collections are used
        ///
        /// If specified, for each offer named, cmc will require that the offer is in a use block.
        /// This can be used to help find missing usages of important capabilities, like
        /// diagnostics
        must_offer_dictionary: Vec<String>,
    },

    #[command(name = "print-cml-reference")]
    /// print generated .cml reference documentation
    PrintReferenceDocs {
        #[arg(name = "file path", short = 'o', long = "output")]
        /// If provided, will output generated reference documentation to a text
        /// file at the file path provided.
        output: Option<PathBuf>,
    },

    #[command(name = "debug-print-cm")]
    /// print pretty rust-debug-format-string representation of .cm file
    DebugPrintCm {
        #[arg(name = "FILE")]
        /// file to process
        file: PathBuf,
    },
}
