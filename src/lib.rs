// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! orgfmt — opinionated org-mode linter and formatter.
//!
//! This crate provides the core library for orgfmt: configuration loading,
//! source file handling, format and lint rule traits, the runner pipeline,
//! and diagnostic output rendering.

pub mod config;
pub mod diagnostic;
pub mod formatter;
pub mod output;
pub mod rules;
pub mod runner;
pub mod source;
