use std::error::Error as StdError;
use std::io::{Error as IoError, ErrorKind};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use vrac::{
    CreateNode, Destination, Engine, GenerateShape, MAX_PAGE_SIZE, Page, Placement, ReferenceInput,
    SyncDeviceId,
};

const DEFAULT_NODE_COUNT: u64 = 5_000_000;
const SAMPLE_COUNT: usize = 100;
const METADATA_PAGE_SIZE: usize = 100;
const DEEP_PATH_LENGTH: usize = 100;
const INTERACTIVE_BUDGET: Duration = Duration::from_millis(5);

fn main() -> Result<(), Box<dyn StdError>> {
    if cfg!(debug_assertions) {
        return Err(input_error("the performance scenario must run with --release").into());
    }

    let (path, node_count, shape, shape_name) = parse_arguments()?;
    let checkpoint_path = path.with_extension("checkpoint.vrac");
    if path.exists() || checkpoint_path.exists() {
        return Err(input_error(format!(
            "refusing to overwrite an existing performance file near: {}",
            path.display(),
        ))
        .into());
    }

    println!("nodes\t{node_count}\tcount");
    println!("shape\t{shape_name}\tname");

    let mut engine = Engine::open(&path)?;
    let started = Instant::now();
    engine.generate_nodes(node_count, shape)?;
    print_duration("generate", started.elapsed());
    drop(engine);

    let started = Instant::now();
    let mut engine = Engine::open_synced(&path, SyncDeviceId::from_bytes([1; 16]))?;
    print_duration("reopen", started.elapsed());

    let started = Instant::now();
    let first_page = engine.children(
        None,
        Page {
            limit: MAX_PAGE_SIZE,
            after: None,
        },
    )?;
    print_duration("first_root_page", started.elapsed());
    println!("first_root_page_nodes\t{}\tcount", first_page.nodes.len());

    let root_page_p95 = measure_p95(|| {
        let page = engine.children(
            None,
            Page {
                limit: 100,
                after: None,
            },
        )?;
        std::hint::black_box(page);
        Ok(())
    })?;
    record_interactive("root_page_100_p95", root_page_p95)?;

    let started = Instant::now();
    let mut after = None;
    let mut root_count = 0_u64;
    let mut page_count = 0_u64;
    loop {
        let page = engine.children(
            None,
            Page {
                limit: MAX_PAGE_SIZE,
                after,
            },
        )?;
        root_count += u64::try_from(page.nodes.len())?;
        page_count += 1;
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    print_duration("all_root_pages", started.elapsed());
    println!("root_nodes\t{root_count}\tcount");
    println!("root_pages\t{page_count}\tcount");

    let started = Instant::now();
    let created = engine.create_node(CreateNode::new("Performance probe"))?;
    print_duration("create_root", started.elapsed());

    let mut metadata_nodes = Vec::with_capacity(METADATA_PAGE_SIZE);
    let mut create_samples = Vec::with_capacity(METADATA_PAGE_SIZE);
    for index in 0..METADATA_PAGE_SIZE {
        let text = format!("Decision {index} on [[target]]");
        let label_start = text.find("target").expect("reference label");
        let mut input = CreateNode::new(&text);
        input.parent_id = Some(created.id);
        input.tags = vec![
            "meeting".into(),
            "decision".into(),
            format!("group-{}", index % 5),
        ];
        input.references = vec![ReferenceInput {
            label_start,
            label_end: label_start + "target".len(),
            target_id: created.id,
        }];
        let started = Instant::now();
        metadata_nodes.push(engine.create_node(input)?);
        create_samples.push(started.elapsed());
    }
    record_interactive(
        "create_with_metadata_p95",
        percentile_95(&mut create_samples),
    )?;
    let source = metadata_nodes
        .first()
        .expect("metadata page is not empty")
        .clone();

    let mut iteration = 0;
    let set_text_p95 = measure_p95(|| {
        let text = if iteration % 2 == 0 {
            "Updated performance probe A"
        } else {
            "Updated performance probe B"
        };
        iteration += 1;
        engine.set_text(created.id, text.into())
    })?;
    record_interactive("set_text_p95", set_text_p95)?;

    let mut iteration = 0;
    let set_tags_p95 = measure_p95(|| {
        let variant = iteration % 2;
        iteration += 1;
        engine.set_tags(
            source.id,
            vec!["decision".into(), format!("performance-{variant}")],
        )
    })?;
    record_interactive("set_tags_p95", set_tags_p95)?;

    let mut iteration = 0;
    let set_content_p95 = measure_p95(|| {
        let text = if iteration % 2 == 0 {
            "Updated [[target A]]"
        } else {
            "Updated [[target B]]"
        };
        iteration += 1;
        let label_start = text.find("target").expect("updated reference label");
        engine.set_content(
            source.id,
            text.into(),
            vec![ReferenceInput {
                label_start,
                label_end: text.len() - 2,
                target_id: created.id,
            }],
        )
    })?;
    record_interactive("set_content_p95", set_content_p95)?;

    let mut iteration = 0;
    let move_p95 = measure_p95(|| {
        let placement = if iteration % 2 == 0 {
            Placement::First
        } else {
            Placement::Last
        };
        iteration += 1;
        engine.move_node(
            created.id,
            Destination {
                parent_id: None,
                placement,
            },
        )
    })?;
    record_interactive("move_root_p95", move_p95)?;

    let node_p95 = measure_p95(|| {
        let node = engine.node(source.id)?;
        std::hint::black_box(node);
        Ok(())
    })?;
    record_interactive("node_with_metadata_p95", node_p95)?;

    let metadata_page = engine.children(
        Some(created.id),
        Page {
            limit: METADATA_PAGE_SIZE,
            after: None,
        },
    )?;
    let target_text = engine
        .node(created.id)?
        .ok_or_else(|| IoError::other("metadata target disappeared"))?
        .text;
    if metadata_page.nodes.len() != METADATA_PAGE_SIZE
        || metadata_page.nodes.iter().any(|node| {
            node.tags.len() < 2
                || node.references.len() != 1
                || node.references[0].target_text != target_text
        })
    {
        return Err(IoError::other("metadata page does not exercise tags and references").into());
    }
    println!("metadata_page_nodes\t{}\tcount", metadata_page.nodes.len());
    let metadata_page_p95 = measure_p95(|| {
        let page = engine.children(
            Some(created.id),
            Page {
                limit: METADATA_PAGE_SIZE,
                after: None,
            },
        )?;
        std::hint::black_box(page);
        Ok(())
    })?;
    record_interactive("metadata_page_100_p95", metadata_page_p95)?;

    let shallow_path_p95 = measure_p95(|| {
        let path = engine.path(source.id)?;
        std::hint::black_box(path);
        Ok(())
    })?;
    record_interactive("shallow_path_p95", shallow_path_p95)?;

    let mut leaf_id = source.id;
    for depth in 0..DEEP_PATH_LENGTH {
        let text = format!("Depth {depth} [[target]]");
        let label_start = text.find("target").expect("deep reference label");
        let mut input = CreateNode::new(&text);
        input.parent_id = Some(leaf_id);
        input.tags = vec!["path".into(), format!("depth-{}", depth % 10)];
        input.references = vec![ReferenceInput {
            label_start,
            label_end: label_start + "target".len(),
            target_id: created.id,
        }];
        leaf_id = engine.create_node(input)?.id;
    }
    let deep_path = engine.path(leaf_id)?;
    if deep_path.len() != DEEP_PATH_LENGTH + 2
        || deep_path
            .iter()
            .skip(1)
            .any(|node| node.tags.is_empty() || node.references.len() != 1)
    {
        return Err(IoError::other("deep path does not exercise node metadata").into());
    }
    println!("deep_path_nodes\t{}\tcount", deep_path.len());
    let deep_path_p95 = measure_p95(|| {
        let path = engine.path(leaf_id)?;
        std::hint::black_box(path);
        Ok(())
    })?;
    record_interactive("deep_path_p95", deep_path_p95)?;

    let started = Instant::now();
    let mut package_count = 0_u64;
    let mut package_bytes = 0_u64;
    while let Some(package) = engine.next_sync_package()? {
        package_count += 1;
        package_bytes += u64::try_from(package.bytes().len())?;
        engine.confirm_sync_package(&package)?;
    }
    print_duration("sync_package_export", started.elapsed());
    println!("sync_packages\t{package_count}\tcount");
    println!("sync_package_bytes\t{package_bytes}\tbytes");

    let started = Instant::now();
    let report = engine.check()?;
    print_duration("integrity_check", started.elapsed());
    if !report.is_ok() {
        return Err(IoError::other(format!(
            "integrity check reported issues: {:?}",
            report.issues
        ))
        .into());
    }
    println!("checked_nodes\t{}\tcount", report.node_count);

    let started = Instant::now();
    engine.checkpoint(&checkpoint_path)?;
    print_duration("checkpoint", started.elapsed());
    println!(
        "checkpoint_bytes\t{}\tbytes",
        std::fs::metadata(&checkpoint_path)?.len()
    );

    drop(engine);
    println!("database_bytes\t{}\tbytes", std::fs::metadata(path)?.len());
    Ok(())
}

fn parse_arguments() -> Result<(PathBuf, u64, GenerateShape, &'static str), Box<dyn StdError>> {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().ok_or_else(|| {
        input_error("usage: performance <new-file> [--nodes <count>] [--shape wide|deep|mixed]")
    })?;
    let mut node_count = DEFAULT_NODE_COUNT;
    let mut shape = GenerateShape::Wide;
    let mut shape_name = "wide";

    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--nodes" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| input_error("--nodes expects a value"))?;
                node_count = value
                    .parse()
                    .map_err(|_| input_error(format!("invalid node count: {value}")))?;
                if node_count == 0 {
                    return Err(input_error("node count must be greater than zero").into());
                }
            }
            "--shape" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| input_error("--shape expects a value"))?;
                (shape, shape_name) = match value.as_str() {
                    "wide" => (GenerateShape::Wide, "wide"),
                    "deep" => (GenerateShape::Deep, "deep"),
                    "mixed" => (GenerateShape::Mixed, "mixed"),
                    _ => {
                        return Err(input_error(format!(
                            "invalid shape: {value} (expected wide, deep, or mixed)"
                        ))
                        .into());
                    }
                };
            }
            _ => return Err(input_error(format!("unknown option: {option}")).into()),
        }
    }

    Ok((PathBuf::from(path), node_count, shape, shape_name))
}

fn print_duration(name: &str, duration: Duration) {
    println!("{name}\t{:.3}\tms", duration.as_secs_f64() * 1_000.0);
}

fn measure_p95(mut operation: impl FnMut() -> vrac::Result<()>) -> vrac::Result<Duration> {
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let started = Instant::now();
        operation()?;
        samples.push(started.elapsed());
    }
    Ok(percentile_95(&mut samples))
}

fn percentile_95(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[(samples.len() * 95).div_ceil(100) - 1]
}

fn record_interactive(name: &str, duration: Duration) -> Result<(), IoError> {
    print_duration(name, duration);
    if duration > INTERACTIVE_BUDGET {
        return Err(IoError::other(format!(
            "{name} exceeded the {:.3} ms interactive budget",
            INTERACTIVE_BUDGET.as_secs_f64() * 1_000.0
        )));
    }
    Ok(())
}

fn input_error(message: impl Into<String>) -> IoError {
    IoError::new(ErrorKind::InvalidInput, message.into())
}
