// Copyright (C) 2026 orgfmt contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Core library for the `org` CLI suite.
//!
//! Provides configuration, source file handling, format/lint rule traits,
//! the runner pipeline, diagnostic output, file collection, and the
//! [`OrgDocument`](document::OrgDocument) heading tree model.

pub mod config;
pub mod diagnostic;
pub mod document;
pub mod files;
pub mod formatter;
pub mod output;
pub mod rules;
pub mod runner;
pub mod source;
