// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Luhmann fleeting note queue and transformation workflows (`org fleeting`).
//!
//! Spec:
//! - Luhmann Fleeting Note Support & Transformation Workflow (`SPEC-orgfmt-roam-workflows.org`).
//! - Org Mode Manual §10.1 (*Capture*): <https://orgmode.org/manual/Capture.html>

mod build;
mod scan;
mod transform;
mod types;

pub use build::{build_fleeting_capture_file, clean_fleeting_title, update_level0_properties};
pub use scan::scan_fleeting_items;
pub use transform::{
    build_gtd_task_for_fleeting, build_permanent_zettel_buffer, resolve_fleeting_target,
    update_source_after_discard, update_source_after_task_convert,
    update_source_after_zettel_reform,
};
pub use types::{
    FleetingCaptureSpec, FleetingItem, FleetingKind, FleetingListEnvelope, FleetingStatus,
    FleetingStatusFilter, FleetingTaskSpec, PermanentZettelSpec,
};
