//! sweep: find what's eating your disk and clean it safely.
//!
//! Exit codes: 0 = success (even when nothing was found),
//! 2 = finished with per-item errors, 1 = fatal error.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;
use sweep_core::cleaner;
use sweep_core::detectors::{self, Ctx};
use sweep_core::model::{CleanOptions, Safety, ScanOptions};
use sweep_core::recycle;
use sweep_core::scanner::{self, display_path, format_bytes, parse_size};

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
        /// Also query allocated bytes per file (slower; hard links can count twice).
        #[arg(long)]
        allocated: bool,
        /// Exit with code 2 when any scan entry could not be read.
        #[arg(long)]
        strict: bool,
        /// Machine-readable output (stable schema for agents).
        #[arg(long)]
        json: bool,
    },
    /// Show large paths that no cleanup detector recognizes; review only.
    Review {
        path: PathBuf,
        #[arg(long, default_value_t = 20)]
        top: usize,
        #[arg(long, default_value = "500MB", value_parser = parse_size)]
        min_size: u64,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        strict: bool,
        /// Include global cache detectors (slower; useful for drive-wide review).
        #[arg(long)]
        all_detectors: bool,
    },
    /// List known caches / build output and their safety labels.
    Detectors {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
        /// Extra roots walked for project artifacts (repeatable).
        #[arg(long, num_args = 1..)]
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
        #[arg(long, num_args = 1..)]
        roots: Vec<PathBuf>,
        /// Skip Docker detection.
        #[arg(long)]
        no_docker: bool,
    },
    /// Inspect or explicitly empty the Recycle Bin on selected drives.
    Bin {
        /// Drive letter to inspect or empty (repeatable, e.g. --drive C --drive D).
        #[arg(long, required = true, value_parser = parse_drive)]
        drive: Vec<char>,
        /// Permanently delete the selected drives' Recycle Bin contents.
        #[arg(long)]
        execute: bool,
        /// Skip the confirmation prompt (required with --json --execute).
        #[arg(long)]
        yes: bool,
        /// Machine-readable receipt.
        #[arg(long)]
        json: bool,
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
            allocated,
            strict,
            json,
        } => {
            let opts = ScanOptions {
                top,
                min_bytes: min_size,
                min_file_bytes: min_file,
                same_filesystem: same_fs,
            };
            let report = scanner::scan_dir_with_allocated(&path, &opts, None, allocated)
                .with_context(|| format!("cannot scan {}", path.display()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_scan_human(&report);
            }
            Ok(if strict && !report.complete { 2 } else { 0 })
        }
        Command::Review {
            path,
            top,
            min_size,
            json,
            strict,
            all_detectors,
        } => {
            let opts = ScanOptions {
                // Filter recognized findings after ranking. Keep every
                // eligible entry so recognized paths cannot exhaust a
                // fixed pre-filter cap and hide unknown large paths.
                top: usize::MAX,
                min_bytes: min_size,
                min_file_bytes: min_size,
                same_filesystem: true,
            };
            let report = scanner::scan_dir(&path, &opts, None)
                .with_context(|| format!("cannot review {}", path.display()))?;
            let ctx = build_ctx(&[path], true)?;
            let project_ids = [
                "cargo",
                "pycache",
                "pytest-caches",
                "vite-build",
                "angular-build",
                "dotnet-build",
                "flutter-build",
                "nextjs-build",
            ];
            let findings = if all_detectors {
                detectors::scan_all(&ctx)
            } else {
                detectors::scan_selected(&ctx, &project_ids.map(str::to_string))
            };
            let known: Vec<PathBuf> = findings
                .iter()
                .filter_map(|finding| match &finding.action {
                    sweep_core::CleanAction::RemovePath { path } => Some(display_path(path)),
                    _ => None,
                })
                .collect();
            let review: Vec<_> = report
                .entries
                .iter()
                .filter(|entry| entry.depth > 0)
                .filter(|entry| !known.iter().any(|path| entry.path.starts_with(path)))
                .take(top)
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "schema_version": sweep_core::model::SCHEMA_VERSION,
                        "root": report.root,
                        "complete": report.complete,
                        "issues": report.issues,
                        "retry_paths": report.retry_paths,
                        "warnings": report.warnings,
                        "warnings_suppressed": report.warnings_suppressed,
                        "review_only": true,
                        "entries": review,
                    }))?
                );
            } else {
                println!("Large unclassified paths (review only; no deletion action):");
                for entry in review {
                    println!(
                        "  {:>10}  {}",
                        format_bytes(entry.bytes),
                        entry.path.display()
                    );
                }
                if !report.complete {
                    println!(
                        "Scan was partial: {} permission denied, {} vanished, {} other.",
                        report.issues.permission_denied,
                        report.issues.not_found,
                        report.issues.other
                    );
                }
            }
            Ok(if strict && !report.complete { 2 } else { 0 })
        }
        Command::Detectors {
            json,
            roots,
            no_docker,
        } => {
            let ctx = build_ctx(&roots, no_docker)?;
            let findings = detectors::scan_all(&ctx);
            let recommendations = recommendations(&findings);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "schema_version": sweep_core::model::SCHEMA_VERSION,
                        "findings": findings,
                        "total_bytes": findings.iter().map(|f| f.bytes).sum::<u64>(),
                        "recommendations": recommendations.iter().map(|f| serde_json::json!({
                            "path": match &f.action {
                                sweep_core::CleanAction::RemovePath { path } => Some(path),
                                _ => None,
                            },
                            "label": f.label,
                            "bytes": f.bytes,
                            "safety": f.safety,
                            "rebuild_note": rebuild_note(f),
                        })).collect::<Vec<_>>(),
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
            let findings = detectors::scan_selected(&ctx, &id);
            let opts = CleanOptions {
                execute,
                to_trash: !permanent,
                include,
                force_danger: force,
            };
            if json {
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
            let started = std::time::Instant::now();
            let (tx, rx) = std::sync::mpsc::sync_channel::<cleaner::CleanProgress>(64);
            let printer = std::thread::spawn(move || {
                while let Ok(ev) = rx.recv() {
                    eprintln!(
                        "  [{:>3}/{}] {:>10}  {}{}",
                        ev.done,
                        ev.total,
                        format_bytes(ev.bytes),
                        if ev.ok { "" } else { "FAILED  " },
                        ev.label
                    );
                }
            });
            let receipt = cleaner::execute_with_progress(&plan, &opts, Some(tx));
            let _ = printer.join();
            print_receipt_human(&receipt, &opts);
            eprintln!("Elapsed: {:.1}s", started.elapsed().as_secs_f64());
            Ok(if receipt.errors.is_empty() { 0 } else { 2 })
        }
        Command::Bin {
            drive,
            execute,
            yes,
            json,
        } => run_bin(&drive, execute, yes, json),
    }
}

fn parse_drive(value: &str) -> Result<char, String> {
    let letter = value.strip_suffix(':').unwrap_or(value);
    if letter.len() != 1 || !letter.as_bytes()[0].is_ascii_alphabetic() {
        return Err("drive must be a single letter such as C or D".to_string());
    }
    Ok(char::from(letter.as_bytes()[0]).to_ascii_uppercase())
}

fn bin_available(drive: char) -> Option<u64> {
    fs4::statvfs(format!("{drive}:\\"))
        .ok()
        .map(|stats| stats.available_space())
}

fn run_bin(drives: &[char], execute: bool, yes: bool, json: bool) -> Result<i32> {
    if json && execute && !yes {
        bail!("refusing --execute --json without --yes");
    }
    let drives: BTreeSet<char> = drives.iter().copied().collect();
    let mut before = Vec::new();
    for drive in drives {
        let stats = recycle::query(drive)
            .with_context(|| format!("cannot inspect {drive}: Recycle Bin"))?;
        before.push((drive, stats, bin_available(drive)));
    }
    if !json {
        println!("Recycle Bin (permanent deletion requires --execute):");
        for (drive, stats, _) in &before {
            println!(
                "  {drive}:  {} in {} items",
                format_bytes(stats.bytes),
                stats.items
            );
        }
    }
    if execute && !yes && !confirm_human()? {
        println!("Aborted. Recycle Bin unchanged.");
        return Ok(0);
    }
    let mut errors = Vec::new();
    let mut results = Vec::new();
    for (drive, stats, available_before) in before {
        let mut error = None;
        let mut after = None;
        let mut available_after = None;
        if execute {
            if stats.items > 0 {
                if let Err(err) = recycle::empty(drive) {
                    error = Some(err.to_string());
                    errors.push(format!("{drive}: {err}"));
                }
            }
            match recycle::query(drive) {
                Ok(value) => {
                    if error.is_none() && value.items > 0 {
                        let message = format!("{drive}: {} Recycle Bin items remain", value.items);
                        errors.push(message.clone());
                        error = Some(message);
                    }
                    after = Some(value);
                }
                Err(err) => {
                    errors.push(format!("{drive}: cannot verify emptying: {err}"));
                    error = Some(err.to_string());
                }
            }
            available_after = bin_available(drive);
        }
        if !json && execute {
            if let Some(err) = &error {
                println!("  {drive}: FAILED: {err}");
            } else if let Some(remaining) = after {
                println!(
                    "  {drive}: {} items remain; available space {} -> {}",
                    remaining.items,
                    available_before
                        .map(format_bytes)
                        .unwrap_or_else(|| "unknown".to_string()),
                    available_after
                        .map(format_bytes)
                        .unwrap_or_else(|| "unknown".to_string())
                );
            }
        }
        results.push(serde_json::json!({
            "drive": drive,
            "before": stats,
            "after": after,
            "available_before": available_before,
            "available_after": available_after,
            "error": error,
        }));
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": sweep_core::model::SCHEMA_VERSION,
                "dry_run": !execute,
                "drives": results,
                "errors": errors,
            }))?
        );
    } else if !execute {
        println!("DRY RUN — nothing emptied. Re-run with --execute to act.");
    }
    Ok(if errors.is_empty() { 0 } else { 2 })
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
        "{}  {} {} logical bytes in {} files / {} dirs",
        report.root.display(),
        if report.complete {
            "complete:"
        } else {
            "PARTIAL:"
        },
        format_bytes(report.total_bytes),
        report.total_files,
        report.total_dirs
    );
    if !report.complete {
        println!("Some entries could not be read; the measured total is incomplete.");
    }
    if let Some(volume) = &report.volume {
        println!(
            "Volume: {} used / {} total; {} available (filesystem accounting)",
            format_bytes(volume.total_bytes.saturating_sub(volume.free_bytes)),
            format_bytes(volume.total_bytes),
            format_bytes(volume.available_bytes)
        );
    }
    if let Some(allocated) = report.allocated_bytes {
        println!(
            "Allocated file entries: {} (hard links may count more than once)",
            format_bytes(allocated)
        );
    }
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
    if !report.complete {
        println!(
            "Issues: {} permission denied, {} vanished, {} other",
            report.issues.permission_denied, report.issues.not_found, report.issues.other
        );
        for path in report.retry_paths.iter().take(5) {
            println!("  Retry after resolving access: {}", path.display());
        }
    }
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
        "Detected: {} logical bytes across {} items\n",
        format_bytes(total),
        findings.len()
    );
    let recommended = recommendations(findings);
    if !recommended.is_empty() {
        println!("Review first (regeneration can take time or bandwidth):");
        for finding in recommended.iter().take(5) {
            println!(
                "  {:>10}  [{}]  {} — {}",
                format_bytes(finding.bytes),
                safety_tag(finding.safety),
                finding.label,
                rebuild_note(finding)
            );
        }
        println!();
    }
    for f in findings {
        println!(
            "{:>10}  [{}]  {}",
            format_bytes(f.bytes),
            safety_tag(f.safety),
            f.label
        );
        if let sweep_core::CleanAction::RemovePath { path } = &f.action {
            println!("             {}", display_path(path).display());
        }
        println!(
            "             {} ({})",
            f.detail.lines().next().unwrap_or(""),
            f.detector_id
        );
    }
    println!("\nRun `sweep clean` for a dry-run plan, `sweep clean --execute` to act.");
}

fn recommendations(findings: &[sweep_core::Finding]) -> Vec<&sweep_core::Finding> {
    let mut candidates: Vec<_> = findings
        .iter()
        .filter(|finding| matches!(finding.action, sweep_core::CleanAction::RemovePath { .. }))
        .filter(|finding| finding.safety != Safety::Danger)
        .collect();
    candidates.sort_by(|a, b| a.safety.cmp(&b.safety).then(b.bytes.cmp(&a.bytes)));
    candidates.truncate(10);
    candidates
}

fn rebuild_note(finding: &sweep_core::Finding) -> &'static str {
    match finding.detector_id.as_str() {
        "cargo" | "flutter-build" | "dotnet-build" | "gradle" => {
            "project builds may need to run again"
        }
        "js-caches" | "pip-cache" | "pub-cache" | "go-cache" | "nextjs-build" => {
            "packages or generated assets may need downloading or rebuilding"
        }
        "browser-cache" => "browser may download or recreate this data",
        "agent-artifacts" => "old task output or history may be useful",
        _ => "regeneration may take time or bandwidth",
    }
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
            "{} {} in logical file sizes ({}):",
            if receipt.dry_run {
                "Would remove"
            } else {
                "Removed"
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
                display_path(&r.path).display()
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
    for change in &receipt.space_changes {
        if let Some(after) = change.available_after {
            let delta = i128::from(after) - i128::from(change.available_before);
            println!(
                "Available on {}: {} → {} ({:+} bytes; other activity may affect this)",
                display_path(&change.volume_root).display(),
                format_bytes(change.available_before),
                format_bytes(after),
                delta
            );
        } else {
            println!(
                "Available-space check failed after cleanup on {}",
                display_path(&change.volume_root).display()
            );
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
