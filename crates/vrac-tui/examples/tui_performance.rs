//! Five-million-node performance reference for the terminal client.

use std::error::Error as StdError;
use std::fs;
use std::io::{Error as IoError, ErrorKind};
use std::path::PathBuf;
use std::time::Instant;

const MEMORY_BUDGET: u64 = 64 * 1024 * 1024;
const MINIMUM_REFERENCE_NODES: u64 = 5_000_000;

fn main() -> Result<(), Box<dyn StdError>> {
    if cfg!(debug_assertions) {
        return Err(input_error("the TUI performance scenario must run with --release").into());
    }

    let source = parse_path()?;
    let source = if source.is_dir() {
        source.join("checkpoint.vrac")
    } else {
        source
    };
    if source
        .file_name()
        .is_none_or(|name| name != "checkpoint.vrac")
    {
        return Err(input_error(
            "pass a provider workspace folder or its immutable checkpoint.vrac",
        )
        .into());
    }
    if !source.is_file() {
        return Err(input_error(format!(
            "the performance checkpoint does not exist: {}",
            source.display()
        ))
        .into());
    }

    let temporary = tempfile::tempdir()?;
    let working_copy = temporary.path().join("tui-performance.vrac");
    let copy_started = Instant::now();
    fs::copy(&source, &working_copy)?;
    println!(
        "working_copy\t{:.3}\tms",
        copy_started.elapsed().as_secs_f64() * 1_000.0
    );
    println!(
        "database_bytes\t{}\tbytes",
        fs::metadata(&working_copy)?.len()
    );

    let peak_before = peak_resident_bytes()?;
    let engine = vrac_tui::run_reference_scenario(&working_copy)?;
    let interactive_peak = peak_resident_bytes()?;
    match (peak_before, interactive_peak) {
        (Some(before), Some(after)) => {
            println!("interactive_peak_rss\t{after}\tbytes");
            println!(
                "interactive_peak_rss_growth\t{}\tbytes",
                after.saturating_sub(before)
            );
            println!("memory_budget\t{MEMORY_BUDGET}\tbytes");
            if after > MEMORY_BUDGET {
                return Err(IoError::other(format!(
                    "the TUI exceeded the {} MiB memory budget",
                    MEMORY_BUDGET / 1024 / 1024
                ))
                .into());
            }
        }
        _ => println!("interactive_peak_rss\tunavailable\tbytes"),
    }

    let check_started = Instant::now();
    let report = engine.check()?;
    println!(
        "integrity_check\t{:.3}\tms",
        check_started.elapsed().as_secs_f64() * 1_000.0
    );
    if !report.is_ok() {
        return Err(IoError::other(format!(
            "integrity check reported issues: {:?}",
            report.issues
        ))
        .into());
    }
    println!("checked_nodes\t{}\tcount", report.node_count);
    if report.node_count < MINIMUM_REFERENCE_NODES {
        return Err(IoError::other(format!(
            "the TUI reference requires at least {MINIMUM_REFERENCE_NODES} nodes"
        ))
        .into());
    }

    Ok(())
}

fn parse_path() -> Result<PathBuf, IoError> {
    let mut arguments = std::env::args_os().skip(1);
    let path = arguments
        .next()
        .ok_or_else(|| input_error("usage: performance <checkpoint-or-workspace-folder>"))?;
    if arguments.next().is_some() {
        return Err(input_error(
            "usage: performance <checkpoint-or-workspace-folder>",
        ));
    }
    Ok(path.into())
}

#[cfg(unix)]
fn peak_resident_bytes() -> Result<Option<u64>, IoError> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the provided rusage value on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return Err(IoError::last_os_error());
    }
    // SAFETY: the successful getrusage call initialized usage.
    let raw = unsafe { usage.assume_init() }.ru_maxrss;
    let bytes = u64::try_from(raw).map_err(|_| IoError::other("negative peak RSS"))?;

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    return Ok(Some(bytes));

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    Ok(Some(bytes.saturating_mul(1024)))
}

#[cfg(not(unix))]
fn peak_resident_bytes() -> Result<Option<u64>, IoError> {
    Ok(None)
}

fn input_error(message: impl Into<String>) -> IoError {
    IoError::new(ErrorKind::InvalidInput, message.into())
}
