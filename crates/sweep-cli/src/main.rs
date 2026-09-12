//! sweep: find what's eating your disk and clean it safely.
//!
//! Exit codes: 0 = success (even when nothing was found),
//! 2 = finished with per-item errors, 1 = fatal error.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use sweep_core::cleaner;
use sweep_core::detectors::{self, Ctx};
use sweep_core::model::{CleanOptions, Safety, ScanOptions};
use sweep_core::scanner::{self, format_bytes, parse_size};

#[derive(Parser)]
#[command(
    name = "sweep",
    version,
    about = "Find what's eating your disk and clean it safely"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Rank the largest directories/files under PATH.
    Scan {
        /// Root to scan.
        path: PathBuf,
        /// How many entries to show.
        #[arg(long, default_value_t = 30)]
        top: usize,
        /// Hide entries below this size (e.g. 10MB, 1GiB).
        #[arg(long, default_value = "1MB", value_parser = parse_size)]
        min_size: u64,
        /// List individual files at/above this size.
        #[arg(long, default_value = "10MB", value_parser = parse_size)]
        min_file: u64,
        /// Stay on the same filesystem as PATH.
        #[arg(long)]
        same_fs: bool,
        /// Machine-readable output (stable schema for agents).
        #[arg(long)]
        json: bool,
    },
    /// List known caches / build output and their safety labels.
    Detectors {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
        /// Extra roots walked for project artifacts (repeatable).
        #[arg(long)]
        roots: Vec<PathBuf>,
        /// Skip Docker detection (useful when the daemon is down).
        #[arg(long)]
        no_docker: bool,
    },
    /// Delete detector findings. Dry-run unless --execute is given.
    Clean {
        /// Actually delete. Without it: plan + report only.
        #[arg(long)]
        execute: bool,
        /// Skip the confirmation prompt (required together with --json --execute).
        #[arg(long)]
        yes: bool,
        /// Machine-readable receipt.
        #[arg(long)]
        json: bool,
        /// Delete permanently instead of moving to Recycle Bin/Trash.
        #[arg(long)]
        permanent: bool,
        /// Safety ceiling: safe, caution, or all (danger still needs --force).
        #[arg(long, default_value = "safe")]
        only: String,
        /// Allow `danger` items (e.g. anything holding live data).
        #[arg(long)]
        force: bool,
        /// Only these detector ids (repeatable, e.g. --id pub-cache).
        #[arg(long)]
        id: Vec<String>,
        /// Extra roots walked for project artifacts (repeatable).
        #[arg(long)]
        roots: Vec<PathBuf>,
        /// Skip Docker detection.
        #[arg(long)]
        no_docker: bool,
    },
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("sweep: {e:#}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32> {
    let cli = Cli::parse();
    match cli.command {
        Command::Scan {
            path,
            top,
            min_size,
            min_file,
            same_fs,
            json,
        } => {
            let opts = ScanOptions {
                top,
                min_bytes: min_size,
                min_file_bytes: min_file,
                same_filesystem: same_fs,
            };
            let report = scanner::scan_dir(&path, &opts, None)
                .with_context(|| format!("cannot scan {}", path.display()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_scan_human(&report);
            }
            Ok(0)
        }
        Command::Detectors {
            json,
            roots,
            no_docker,
        } => {
            let ctx = build_ctx(&roots, no_docker)?;
            let findings = detectors::scan_all(&ctx);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "schema_version": sweep_core::model::SCHEMA_VERSION,
                        "findings": findings,
                        "total_bytes": findings.iter().map(|f| f.bytes).sum::<u64>(),
                    }))?
                );
            } else {
                print_findings_human(&findings);
            }
            Ok(0)
        }
        Command::Clean {
            execute,
            yes,
            json,
            permanent,
            only,
            force,
            id,
            roots,
            no_docker,
        } => {
            let include = Safety::parse_level(&only)
                .with_context(|| format!("--only must be safe, caution or all (got {only})"))?;
            if json && execute && !yes {
                bail!("refusing --execute --json without --yes: agents must opt in explicitly");
            }
            validate_ids(&id)?;
            let ctx = build_ctx(&roots, no_docker)?;
            let mut findings = detectors::scan_all(&ctx);
            retain_ids(&mut findings, &id);
            let opts = CleanOptions {
                execute,
                to_trash: !permanent,
                include,
                force_danger: force,
            };
            if json {
                // Single-shot for agents: explicit --yes required to act.
                if execute && !yes {
                    bail!("refusing --execute --json without --yes: agents must opt in explicitly");
                }
                let receipt = cleaner::clean(&findings, &opts);
                println!("{}", serde_json::to_string_pretty(&receipt)?);
                return Ok(if receipt.errors.is_empty() { 0 } else { 2 });
            }
            // Human flow: always show the dry-run plan first, confirm, then act.
            let dry = CleanOptions {
                execute: false,
                ..opts
            };
            let plan = cleaner::plan(&findings, &opts);
            let preview = cleaner::execute(&plan, &dry);
            print_receipt_human(&preview, &dry);
            if !execute || preview.removed.is_empty() {
                return Ok(0);
            }
            if !yes && !confirm_human()? {
                println!("Aborted. Nothing deleted.");
                return Ok(0);
            }
            let receipt = cleaner::execute(&plan, &opts);
            print_receipt_human(&receipt, &opts);
            Ok(if receipt.errors.is_empty() { 0 } else { 2 })
        }
    }
}

/// Human clean flow needs confirmation BEFORE deleting, so it plans first,
/// asks, then executes. The JSON flow stays single-shot for agents.
fn confirm_human() -> Result<bool> {
    print!("Delete as listed above? [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().eq_ignore_ascii_case("y"))
}

fn validate_ids(ids: &[String]) -> Result<()> {
    let detectors = detectors::all_detectors();
    for id in ids {
        if !detectors.iter().any(|d| d.id() == id) {
            bail!("unknown detector --id: {id}");
        }
    }
    Ok(())
}

fn retain_ids(findings: &mut Vec<sweep_core::Finding>, ids: &[String]) {
    if !ids.is_empty() {
        findings.retain(|f| ids.contains(&f.detector_id));
    }
}

fn build_ctx(roots: &[PathBuf], no_docker: bool) -> Result<Ctx> {
    let mut ctx = Ctx::from_env();
    if !roots.is_empty() {
        // --roots REPLACES the default (current directory); invalid entries
        // fail loudly instead of silently scanning somewhere unexpected.
        let mut kept = Vec::new();
        for root in roots {
            if root.is_dir() {
                kept.push(root.clone());
            } else {
                bail!("--roots entry is not a directory: {}", root.display());
            }
        }
        ctx.project_roots = kept;
    }
    if no_docker {
        ctx.docker_enabled = false;
    }
    Ok(ctx)
}

fn print_scan_human(report: &sweep_core::ScanReport) {
    println!(
        "{}  total {} in {} files / {} dirs",
        report.root.display(),
        format_bytes(report.total_bytes),
        report.total_files,
        report.total_dirs
    );
    println!("{:-<64}", "");
    for e in &report.entries {
        let kind = if e.is_dir { "d" } else { "f" };
        println!(
            "{:>10}  [{kind}]  {}{}",
            format_bytes(e.bytes),
            "  ".repeat(e.depth.min(8)),
            e.path.display()
        );
    }
    print_warnings(&report.warnings, report.warnings_suppressed);
}

fn safety_tag(safety: Safety) -> &'static str {
    match safety {
        Safety::Safe => "SAFE  ",
        Safety::Caution => "CAUTION",
        Safety::Danger => "DANGER",
    }
}

fn print_findings_human(findings: &[sweep_core::Finding]) {
    if findings.is_empty() {
        println!("Nothing cleanable found. Your disk is already lean.");
        return;
    }
    let total: u64 = findings.iter().map(|f| f.bytes).sum();
    println!(
        "Reclaimable: {} across {} items\n",
        format_bytes(total),
        findings.len()
    );
    for f in findings {
        println!(
            "{:>10}  [{}]  {}",
            format_bytes(f.bytes),
            safety_tag(f.safety),
            f.label
        );
        if let sweep_core::CleanAction::RemovePath { path } = &f.action {
            println!("             {}", path.display());
        }
        println!(
            "             {} ({})",
            f.detail.lines().next().unwrap_or(""),
            f.detector_id
        );
    }
    println!("\nRun `sweep clean` for a dry-run plan, `sweep clean --execute` to act.");
}

fn print_receipt_human(receipt: &sweep_core::CleanReceipt, opts: &CleanOptions) {
    if receipt.dry_run {
        println!(
            "DRY RUN — nothing deleted. Re-run with --execute to act (default destination: {}).\n",
            if opts.to_trash {
                "Recycle Bin"
            } else {
                "permanent!"
            }
        );
    }
    if !receipt.removed.is_empty() {
        println!(
            "{} {} ({}):",
            if receipt.dry_run {
                "Would free"
            } else {
                "Freed"
            },
            format_bytes(receipt.freed_bytes),
            receipt.removed.len()
        );
        for r in &receipt.removed {
            println!(
                "  {:>10}  [{}]  [{}]  {}",
                format_bytes(r.bytes),
                r.via,
                safety_tag(r.safety),
                r.path.display()
            );
        }
    }
    if !receipt.skipped.is_empty() {
        println!("\nSkipped:");
        for s in &receipt.skipped {
            println!(
                "  - [{}] {}: {}",
                safety_tag(s.safety),
                s.label,
                s.reason.lines().next().unwrap_or("")
            );
        }
    }
    if !receipt.errors.is_empty() {
        println!("\nErrors:");
        for e in &receipt.errors {
            println!("  ! {e}");
        }
    }
    if receipt.removed.is_empty() && receipt.skipped.is_empty() && receipt.errors.is_empty() {
        println!("Nothing to do.");
    }
}

fn print_warnings(warnings: &[String], suppressed: usize) {
    if warnings.is_empty() && suppressed == 0 {
        return;
    }
    println!(
        "\nWarnings ({} shown, {} suppressed):",
        warnings.len(),
        suppressed
    );
    for w in warnings.iter().take(5) {
        println!("  ! {w}");
    }
}
