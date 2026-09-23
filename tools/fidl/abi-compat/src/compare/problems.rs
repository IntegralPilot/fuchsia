// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use std::fmt::Display;

use super::Path;

// Helper to find the common prefix of two strings and return the common prefix and the differing suffixes.
fn common_prefix<'a, 'b>(a: &'a str, b: &'b str) -> (&'a str, &'a str, &'b str) {
    let n = a.chars().zip(b.chars()).take_while(|(a, b)| a == b).count();
    (&a[..n], &a[n..], &b[n..])
}

#[derive(Clone, Debug, Eq)]
struct ProblemPath {
    version: u32,
    for_display: String,
    for_comparison: String,
}

impl ProblemPath {
    fn new(paths: [&Path; 2]) -> Self {
        let version =
            paths.iter().filter_map(|p| p.api_level().parse::<u32>().ok()).min().unwrap_or(0);

        let [a, b] = paths.map(|p| p.string());

        let for_display: String = if a == b {
            a.to_owned()
        } else {
            let (c, a_suffix, b_suffix) = common_prefix(&a, &b);
            if a_suffix.is_empty() {
                b.to_string()
            } else if b_suffix.is_empty() {
                a.to_string()
            } else {
                format!("{c}({a_suffix}|{b_suffix})")
            }
        };

        let for_comparison = std::cmp::min(a, b).to_owned();

        Self { version, for_display, for_comparison }
    }
}

impl Display for ProblemPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.for_display.fmt(f)
    }
}

impl PartialEq for ProblemPath {
    fn eq(&self, other: &Self) -> bool {
        self.for_comparison.eq(&other.for_comparison)
    }
}

impl PartialOrd for ProblemPath {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(cmp_prefixes_later(&self.for_comparison, &other.for_comparison))
    }
}

impl Ord for ProblemPath {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        cmp_prefixes_later(&self.for_comparison, &other.for_comparison)
    }
}

fn cmp_prefixes_later(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    // Doing a string comparison, but sorting longer strings before their prefixes.
    // This way more specific errors show up before more general ones.
    match a.cmp(b) {
        Equal => Equal,
        Less => {
            if b.starts_with(a) {
                Greater
            } else {
                Less
            }
        }
        Greater => {
            if a.starts_with(b) {
                Less
            } else {
                Greater
            }
        }
    }
}

#[test]
fn test_cmp_prefixes_later() {
    use std::cmp::Ordering::*;
    assert_eq!(Equal, cmp_prefixes_later("foo", "foo"));
    assert_eq!(Greater, cmp_prefixes_later("foo", "bar"));
    assert_eq!(Less, cmp_prefixes_later("bar", "foo"));
    assert_eq!(Greater, cmp_prefixes_later("foo", "foo.bar"));
    assert_eq!(Less, cmp_prefixes_later("foo.bar", "foo"));
}

static ALLOW_LIST: &'static [(&'static [u32], &'static str)] = &[];

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompatibilityProblem {
    warning: bool,
    paths: ProblemPath,
    message: String,
}

impl CompatibilityProblem {
    fn allowed(&self) -> bool {
        ALLOW_LIST.iter().any(|(versions, path)| {
            &self.paths.for_display == path && versions.iter().any(|v| v == &self.paths.version)
        })
    }
}

impl Display for CompatibilityProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.warning {
            writeln!(f, "WRN: {}", self.message)?;
        } else {
            writeln!(f, "ERR: {}", self.message)?;
        }
        writeln!(f, " at: {}", self.paths)
    }
}

impl std::fmt::Debug for CompatibilityProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("CompatibilityProblem");

        d.field("warning", &self.warning);
        d.field("message", &self.message);
        d.field("path", &self.paths.for_display);
        d.finish()
    }
}

#[test]
fn test_compatibility_problem_comparison() {
    let warning = CompatibilityProblem {
        paths: ProblemPath::new([&Path::empty(), &Path::empty()]),
        warning: true,
        message: "beware".to_owned(),
    };
    let error = CompatibilityProblem {
        paths: ProblemPath::new([&Path::empty(), &Path::empty()]),
        warning: false,
        message: "to err is human".to_owned(),
    };

    assert!(error < warning);
}

#[derive(Default)]
pub struct CompatibilityProblems(Vec<CompatibilityProblem>);

impl CompatibilityProblems {
    pub fn error(&mut self, paths: [&Path; 2], message: String) {
        self.push(CompatibilityProblem { paths: ProblemPath::new(paths), warning: false, message });
    }
    pub fn warning(&mut self, paths: [&Path; 2], message: String) {
        self.push(CompatibilityProblem { paths: ProblemPath::new(paths), warning: true, message });
    }

    pub fn add(&mut self, warning: bool, paths: [&Path; 2], message: String) {
        self.push(CompatibilityProblem { paths: ProblemPath::new(paths), warning, message });
    }

    pub fn append(&mut self, mut other: CompatibilityProblems) {
        self.0.append(&mut other.0);
    }

    fn push(&mut self, problem: CompatibilityProblem) {
        if !problem.allowed() {
            self.0.push(problem);
        }
    }

    pub fn is_incompatible(&self) -> bool {
        self.0.iter().any(|p| !p.warning)
    }

    // TODO: remove this
    #[cfg(test)]
    pub fn is_compatible(&self) -> bool {
        self.has_problems(vec![])
    }

    #[cfg(test)]
    /// Returns true if for every problem there's a exactly one pattern that matches and vice versa.
    pub fn has_problems(&self, patterns: Vec<ProblemPattern<'_>>) -> bool {
        let matching_problems: Vec<Vec<usize>> = patterns
            .iter()
            .map(|pattern| {
                self.0
                    .iter()
                    .enumerate()
                    .filter_map(
                        |(i, problem)| {
                            if pattern.matches(problem) {
                                Some(i)
                            } else {
                                None
                            }
                        },
                    )
                    .collect()
            })
            .collect();
        let matched_problems: std::collections::BTreeSet<usize> =
            matching_problems.iter().flat_map(|ps| ps.iter().cloned()).collect();

        let mut ok = true;

        for (pattern, matching) in patterns.iter().zip(matching_problems) {
            match matching.len() {
                1 => (),
                0 => {
                    println!("Pattern doesn't match any problems: {:?}", pattern);
                    ok = false;
                }
                _ => {
                    println!("Pattern matches {} problems: {:?}", matching.len(), pattern);
                    for i in matching {
                        println!("  {:?}", self.0[i]);
                    }
                    ok = false;
                }
            }
        }
        for (i, problem) in self.0.iter().enumerate() {
            if !matched_problems.contains(&i) {
                println!("Unexpected problem: {:?}", problem);
                ok = false;
            }
        }

        ok
    }

    pub fn sort(&mut self) {
        self.0.sort()
    }

    pub fn into_errors_and_warnings(self) -> (Self, Self) {
        let (warnings, errors) = self.0.into_iter().partition(|p| p.warning);
        (Self(errors), Self(warnings))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl IntoIterator for CompatibilityProblems {
    type Item = CompatibilityProblem;

    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[cfg(test)]
#[derive(Debug)]
pub enum StringPattern<'a> {
    Equals(&'a str),
    Begins(&'a str),
    Ends(&'a str),
    Contains(&'a str),
}

#[cfg(test)]
impl StringPattern<'_> {
    pub fn matches(&self, string: impl AsRef<str>) -> bool {
        let string = string.as_ref();
        match self {
            StringPattern::Equals(pattern) => &string == pattern,
            StringPattern::Begins(pattern) => string.starts_with(pattern),
            StringPattern::Ends(pattern) => string.ends_with(pattern),
            StringPattern::Contains(pattern) => string.contains(pattern),
        }
    }
}

#[cfg(test)]
impl<'a> Into<StringPattern<'a>> for &'a str {
    fn into(self) -> StringPattern<'a> {
        StringPattern::Equals(self)
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct ProblemPattern<'a> {
    warning: Option<bool>,
    message: Option<StringPattern<'a>>,
    path: Option<StringPattern<'a>>,
}

#[cfg(test)]
impl<'a> ProblemPattern<'a> {
    pub fn matches(&self, problem: &CompatibilityProblem) -> bool {
        if let Some(warning) = self.warning {
            if warning != problem.warning {
                return false;
            }
        }
        if let Some(message) = &self.message {
            if !message.matches(&problem.message) {
                return false;
            }
        }
        if let Some(path) = &self.path {
            if !path.matches(&problem.paths.for_display) {
                return false;
            }
        }
        true
    }
    pub fn error() -> Self {
        Self { warning: Some(false), ..Default::default() }
    }

    pub fn warning() -> Self {
        Self { warning: Some(true), ..Default::default() }
    }

    pub fn message(self, message: impl Into<StringPattern<'a>>) -> Self {
        Self { message: Some(message.into()), ..self }
    }

    pub fn path(self, path: StringPattern<'a>) -> Self {
        Self { path: Some(path), ..self }
    }
}

#[cfg(test)]
impl std::fmt::Debug for ProblemPattern<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("ProblemPattern");

        if let Some(warning) = self.warning {
            d.field("warning", &warning);
        };
        if let Some(message) = &self.message {
            d.field("message", message);
        }
        if let Some(path) = &self.path {
            d.field("path", path);
        }

        d.finish()
    }
}
