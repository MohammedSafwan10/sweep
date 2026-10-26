//! Read-only stage timings. Never executes a cleanup plan.
use std::path::PathBuf;
use std::time::Instant;
use sweep_core::detectors::{self, Ctx};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() {
        return Err("usage: profile_cleanup PATH... (or --discover ROOT)".into());
    }
    if args[0] == "--discover" {
        let mut ctx = Ctx::from_env();
        ctx.project_roots = args.iter().skip(1).map(PathBuf::from).collect();
        ctx.docker_enabled = false;
        for id in ["pub-cache", "cargo"] {
            let start = Instant::now();
            let findings = detectors::scan_selected(&ctx, &[id.to_owned()]);
            println!(
                "{}",
                serde_json::json!({"stage": "discovery", "id": id,
                "seconds": start.elapsed().as_secs_f64(), "findings": findings.len(),
                "bytes": findings.iter().map(|f| f.bytes).sum::<u64>()})
            );
        }
    } else {
        for arg in args {
            let path = PathBuf::from(arg);
            let start = Instant::now();
            let counters = sweep_core::snapshot::SnapshotProgress::default();
            let (stop, rx) = std::sync::mpsc::channel::<()>();
            let snapshot = std::thread::scope(|scope| {
                let progress = &counters;
                let display_path = &path;
                let printer = std::thread::Builder::new().spawn_scoped(scope, move || {
                    while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                        rx.recv_timeout(std::time::Duration::from_secs(5))
                    {
                        eprintln!(
                            "{:.1}s: {:?}, {:?}",
                            start.elapsed().as_secs_f64(),
                            display_path,
                            progress.counts()
                        );
                    }
                })?;
                let result = sweep_core::snapshot::capture_with_progress(&path, Some(&counters));
                drop(stop);
                printer
                    .join()
                    .map_err(|_| std::io::Error::other("progress thread panicked"))?;
                result
            })?;
            println!(
                "{}",
                serde_json::json!({"stage": "snapshot", "path": path,
                "seconds": start.elapsed().as_secs_f64(), "snapshot": snapshot})
            );
        }
    }
    Ok(())
}
