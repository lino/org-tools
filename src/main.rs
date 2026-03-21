use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};
use ignore::WalkBuilder;

use orgfmt::config::Config;
use orgfmt::output::{render_diagnostics, OutputFormat};
use orgfmt::runner::Runner;
use orgfmt::source::SourceFile;

#[derive(Parser)]
#[command(name = "orgfmt", about = "Opinionated org-mode linter and formatter")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Path to config file (default: search for .orgfmt.toml in current and parent dirs).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Lint org files and report diagnostics.
    Check {
        /// Files or directories to check.
        paths: Vec<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value = "human")]
        format: OutputFormat,

        /// Auto-fix fixable issues (applies format rule fixes in-place).
        #[arg(long)]
        fix: bool,
    },
    /// Format org files.
    Format {
        /// Files or directories to format.
        paths: Vec<PathBuf>,

        /// Check mode: exit 1 if changes needed, don't modify files.
        #[arg(long)]
        check: bool,

        /// Write to stdout instead of modifying files.
        #[arg(long)]
        stdout: bool,
    },
}

fn collect_org_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();

    for path in paths {
        if path.is_file() {
            files.push(path.clone());
        } else if path.is_dir() {
            for entry in WalkBuilder::new(path).build().flatten() {
                let p = entry.path();
                if p.is_file() && p.extension().is_some_and(|ext| ext == "org") {
                    files.push(p.to_path_buf());
                }
            }
        } else {
            eprintln!("orgfmt: path not found: {}", path.display());
        }
    }

    files.sort();
    files
}

fn load_config(cli_config: &Option<PathBuf>) -> Config {
    if let Some(path) = cli_config {
        if path.is_file() {
            match std::fs::read_to_string(path) {
                Ok(contents) => match toml::from_str::<Config>(&contents) {
                    Ok(config) => return config,
                    Err(e) => {
                        eprintln!("orgfmt: error parsing {}: {}", path.display(), e);
                        process::exit(2);
                    }
                },
                Err(e) => {
                    eprintln!("orgfmt: error reading {}: {}", path.display(), e);
                    process::exit(2);
                }
            }
        } else {
            eprintln!("orgfmt: config file not found: {}", path.display());
            process::exit(2);
        }
    }

    // Search for .orgfmt.toml from current directory upward.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Config::load(&cwd)
}

fn main() {
    let cli = Cli::parse();
    let config = load_config(&cli.config);
    let runner = Runner::new(config);

    let exit_code = match cli.command {
        Command::Check { paths, format, fix } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("orgfmt: no .org files found");
                process::exit(2);
            }

            if fix {
                // --fix mode: apply format fixes, then report remaining lint issues.
                let mut has_issues = false;
                for file in &files {
                    match SourceFile::from_path(file) {
                        Ok(source) => {
                            let (formatted, lint_diags) = runner.format(&source);
                            let changed = formatted != source.content;

                            if changed {
                                if let Err(e) = std::fs::write(file, &formatted) {
                                    eprintln!(
                                        "orgfmt: error writing {}: {}",
                                        file.display(),
                                        e
                                    );
                                } else {
                                    println!("Fixed: {}", file.display());
                                }
                            }

                            if !lint_diags.is_empty() {
                                has_issues = true;
                                print!(
                                    "{}",
                                    render_diagnostics(&lint_diags, format)
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!("orgfmt: error reading {}: {}", file.display(), e);
                        }
                    }
                }
                if has_issues { 1 } else { 0 }
            } else {
                // Normal check mode: report all diagnostics.
                let mut all_diagnostics = Vec::new();
                for file in &files {
                    match SourceFile::from_path(file) {
                        Ok(source) => {
                            all_diagnostics.extend(runner.check(&source));
                        }
                        Err(e) => {
                            eprintln!("orgfmt: error reading {}: {}", file.display(), e);
                        }
                    }
                }

                if !all_diagnostics.is_empty() {
                    print!("{}", render_diagnostics(&all_diagnostics, format));
                    1
                } else {
                    0
                }
            }
        }
        Command::Format {
            paths,
            check,
            stdout,
        } => {
            let files = collect_org_files(&paths);
            if files.is_empty() {
                eprintln!("orgfmt: no .org files found");
                process::exit(2);
            }

            let mut has_changes = false;
            let mut has_lint_issues = false;

            for file in &files {
                match SourceFile::from_path(file) {
                    Ok(source) => {
                        let (formatted, lint_diags) = runner.format(&source);
                        let changed = formatted != source.content;

                        if changed {
                            has_changes = true;
                        }

                        if !lint_diags.is_empty() {
                            has_lint_issues = true;
                            print!("{}", render_diagnostics(&lint_diags, OutputFormat::Human));
                        }

                        if check {
                            if changed {
                                println!("Would reformat: {}", file.display());
                            }
                        } else if stdout {
                            print!("{}", formatted);
                        } else if changed {
                            if let Err(e) = std::fs::write(file, &formatted) {
                                eprintln!("orgfmt: error writing {}: {}", file.display(), e);
                            } else {
                                println!("Formatted: {}", file.display());
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("orgfmt: error reading {}: {}", file.display(), e);
                    }
                }
            }

            if (check && has_changes) || has_lint_issues {
                1
            } else {
                0
            }
        }
    };

    process::exit(exit_code);
}
