use std::error::Error as StdError;
use std::io::{Error as IoError, ErrorKind};
use std::path::PathBuf;

use vrac::{Engine, Page, SystemNode};

const ITERATIONS: usize = 1_000;
const PAGE_SIZE: usize = 100;
const SEARCH_LIMIT: usize = 8;
const INTERACTIVE_MEMORY_BUDGET: u64 = 32 * 1024 * 1024;

fn main() -> Result<(), Box<dyn StdError>> {
    if cfg!(debug_assertions) {
        return Err(input_error("the memory scenario must run with --release").into());
    }

    let path = parse_path()?;
    let peak_before = peak_resident_bytes()?;
    let engine = Engine::open(&path)?;

    let decision = engine
        .search("decision 42", SEARCH_LIMIT)?
        .into_iter()
        .next()
        .ok_or_else(|| input_error("the workspace does not contain the performance probes"))?;
    let metadata_parent = decision
        .references
        .first()
        .ok_or_else(|| input_error("the decision probe has no reference"))?
        .target_id;
    let deep_leaf = engine
        .search("depth 99", SEARCH_LIMIT)?
        .into_iter()
        .next()
        .ok_or_else(|| input_error("the workspace does not contain the deep path probe"))?
        .id;
    let journal_day = engine
        .search("2030 01 01", SEARCH_LIMIT)?
        .into_iter()
        .find(|node| matches!(node.system, Some(SystemNode::JournalDay { .. })))
        .ok_or_else(|| input_error("the workspace does not contain the journal probe"))?
        .id;

    exercise_interactive_reads(&engine, metadata_parent, deep_leaf, journal_day)?;
    let peak_after_warmup = peak_resident_bytes()?;

    for _ in 0..ITERATIONS {
        exercise_interactive_reads(&engine, metadata_parent, deep_leaf, journal_day)?;
    }

    let peak_after = peak_resident_bytes()?;
    println!("iterations\t{ITERATIONS}\tcount");
    match (peak_before, peak_after_warmup, peak_after) {
        (Some(before), Some(after_warmup), Some(after)) => {
            let growth = after.saturating_sub(before);
            println!("peak_rss\t{after}\tbytes");
            println!(
                "warmup_peak_rss_growth\t{}\tbytes",
                after_warmup.saturating_sub(before)
            );
            println!("final_peak_rss_growth\t{growth}\tbytes");
            println!("interactive_memory_budget\t{INTERACTIVE_MEMORY_BUDGET}\tbytes");
            if growth > INTERACTIVE_MEMORY_BUDGET {
                return Err(IoError::other(format!(
                    "interactive reads exceeded the {} MiB resident-memory budget",
                    INTERACTIVE_MEMORY_BUDGET / 1024 / 1024
                ))
                .into());
            }
        }
        _ => println!("peak_rss\tunavailable\tbytes"),
    }

    Ok(())
}

fn exercise_interactive_reads(
    engine: &Engine,
    metadata_parent: vrac::NodeId,
    deep_leaf: vrac::NodeId,
    journal_day: vrac::NodeId,
) -> vrac::Result<()> {
    std::hint::black_box(engine.children(
        None,
        Page {
            limit: PAGE_SIZE,
            after: None,
        },
    )?);
    std::hint::black_box(engine.children(
        Some(metadata_parent),
        Page {
            limit: PAGE_SIZE,
            after: None,
        },
    )?);
    std::hint::black_box(engine.search("decision 42", SEARCH_LIMIT)?);
    std::hint::black_box(engine.tags("dec", SEARCH_LIMIT)?);
    std::hint::black_box(engine.backlinks(
        metadata_parent,
        None,
        Page {
            limit: PAGE_SIZE,
            after: None,
        },
    )?);
    std::hint::black_box(engine.backlinks(
        metadata_parent,
        Some("decision"),
        Page {
            limit: PAGE_SIZE,
            after: None,
        },
    )?);
    std::hint::black_box(engine.backlink_tags(metadata_parent, PAGE_SIZE)?);
    std::hint::black_box(engine.path(deep_leaf)?);
    std::hint::black_box(engine.node(journal_day)?);
    Ok(())
}

fn parse_path() -> Result<PathBuf, IoError> {
    let mut arguments = std::env::args().skip(1);
    let path = arguments
        .next()
        .ok_or_else(|| input_error("usage: memory <performance-file>"))?;
    if arguments.next().is_some() {
        return Err(input_error("usage: memory <performance-file>"));
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
