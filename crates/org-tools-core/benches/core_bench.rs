// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use org_tools_core::config::Config;
use org_tools_core::document::OrgDocument;
use org_tools_core::edna::{is_blocked, EdnaContext};
use org_tools_core::rules::format::table_formatter::TableFormatter;
use org_tools_core::rules::{FormatContext, FormatRule};
use org_tools_core::source::SourceFile;
use org_tools_core::tblfm::calc_file;

fn run_bench<F, R>(name: &str, iterations: usize, mut f: F)
where
    F: FnMut() -> R,
{
    // Warmup
    for _ in 0..(iterations / 10).max(1) {
        black_box(f());
    }

    let start = Instant::now();
    for _ in 0..iterations {
        black_box(f());
    }
    let elapsed = start.elapsed();
    let per_iter = elapsed / (iterations as u32);
    let ns_per_iter = per_iter.as_nanos();
    let ops_per_sec = if ns_per_iter > 0 {
        1_000_000_000.0 / (ns_per_iter as f64)
    } else {
        0.0
    };

    println!(
        "{:<35} {:>8} iters  {:>10.2?} / iter  {:>12.0} ops/s",
        name, iterations, per_iter, ops_per_sec
    );
}

fn generate_sample_org(heading_count: usize) -> String {
    let mut s = String::new();
    s.push_str("#+TITLE: Benchmark Sample Document\n");
    s.push_str("#+AUTHOR: Benchmark\n\n");

    for i in 1..=heading_count {
        let level = (i % 3) + 1;
        let stars = "*".repeat(level);
        let kw = if i % 2 == 0 { "TODO " } else { "DONE " };
        let tag = if i % 4 == 0 { " :work:urgent:" } else { " :notes:" };
        s.push_str(&format!("{stars} {kw}Task {i}{tag}\n"));
        if i % 3 == 0 {
            s.push_str("SCHEDULED: <2026-09-01 Mon 10:00> DEADLINE: <2026-09-10 Wed>\n");
        }
        s.push_str(":PROPERTIES:\n");
        s.push_str(&format!(":ID: id-{i:05}\n"));
        if i % 5 == 0 {
            s.push_str(&format!(":BLOCKER: id-{:05}\n", (i.saturating_sub(1)).max(1)));
        }
        s.push_str(":END:\n");
        s.push_str(&format!("Body paragraph content for task {i}.\n\n"));
    }
    s
}

fn generate_sample_table(rows: usize) -> String {
    let mut s = String::new();
    s.push_str("| Item | Qty | Unit Price | Total |\n");
    s.push_str("|---+---+---+---|\n");
    for i in 1..=rows {
        s.push_str(&format!("| Widget {i} | {} | ${:.2} | |\n", i * 2, (i as f64) * 1.5));
    }
    s
}

fn main() {
    println!("=== org-tools-core benchmarks ===");
    println!("-------------------------------------------------------------------------------");

    let small_text = generate_sample_org(20);
    let large_text = generate_sample_org(500);

    let small_source = SourceFile::new(PathBuf::from("small.org"), small_text);
    let large_source = SourceFile::new(PathBuf::from("large.org"), large_text);

    // 1. Parsing benchmarks
    run_bench("parse_document_20_headings", 2_000, || {
        OrgDocument::from_source(&small_source)
    });

    run_bench("parse_document_500_headings", 100, || {
        OrgDocument::from_source(&large_source)
    });

    // 2. ID index lookup benchmark vs linear scan
    let doc_500 = OrgDocument::from_source(&large_source);
    let target_id_first = "id-00001";
    let target_id_mid = "id-00250";
    let target_id_last = "id-00500";
    let target_id_missing = "id-99999";

    run_bench("id_lookup_indexed_hit_first", 50_000, || {
        doc_500.find_by_id(target_id_first)
    });

    run_bench("id_lookup_indexed_hit_mid", 50_000, || {
        doc_500.find_by_id(target_id_mid)
    });

    run_bench("id_lookup_indexed_hit_last", 50_000, || {
        doc_500.find_by_id(target_id_last)
    });

    run_bench("id_lookup_indexed_miss", 50_000, || {
        doc_500.find_by_id(target_id_missing)
    });

    run_bench("id_lookup_linear_mid (baseline)", 10_000, || {
        doc_500
            .entries
            .iter()
            .position(|e| e.properties.get("ID").map(|s| s.as_str()) == Some(target_id_mid))
    });

    run_bench("id_lookup_linear_miss (baseline)", 10_000, || {
        doc_500
            .entries
            .iter()
            .position(|e| e.properties.get("ID").map(|s| s.as_str()) == Some(target_id_missing))
    });

    // 3. Table formatter benchmark
    let table_raw = generate_sample_table(50);
    let table_source = SourceFile::new(PathBuf::from("sample.org"), table_raw);
    let config = Config::default();
    let ctx = FormatContext::new(&table_source, &config);
    let table_formatter = TableFormatter;

    run_bench("table_formatter_50_rows", 2_000, || {
        table_formatter.format(&ctx)
    });

    // 4. Table formula (tblfm) evaluation benchmark
    let tblfm_doc = "| Item | Qty | Rate | Subtotal |\n|---+---+---+---|\n| A | 10 | 5.5 | |\n| B | 20 | 3.0 | |\n| Total | | | |\n#+TBLFM: $4=$2*$3; :: @>$4=vsum(@2$4..@-1$4)";
    let constants = HashMap::new();

    run_bench("tblfm_calc_file_eval", 2_000, || {
        calc_file(tblfm_doc, &constants)
    });

    // 5. Edna blocker dependency evaluation benchmark
    let all_docs = [&doc_500];
    let blocked_ctx = EdnaContext {
        all_docs: &all_docs,
        doc: &doc_500,
        entry_idx: 9, // task 10 (has blocker id-00009)
    };
    let unblocked_ctx = EdnaContext {
        all_docs: &all_docs,
        doc: &doc_500,
        entry_idx: 0, // task 1 (no blocker)
    };

    run_bench("edna_is_blocked_hit", 10_000, || {
        is_blocked(&blocked_ctx)
    });

    run_bench("edna_is_blocked_none", 50_000, || {
        is_blocked(&unblocked_ctx)
    });

    println!("-------------------------------------------------------------------------------");
    println!("Benchmark run complete.");
}
