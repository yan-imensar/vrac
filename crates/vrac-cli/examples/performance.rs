use std::error::Error as StdError;
use std::io::{Error as IoError, ErrorKind};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use vrac::{
    CreateNode, Destination, Engine, GenerateShape, MAX_PAGE_SIZE, Page, Placement, ReferenceInput,
};

const DEFAULT_NODE_COUNT: u64 = 5_000_000;

fn main() -> Result<(), Box<dyn StdError>> {
    if cfg!(debug_assertions) {
        return Err(input_error("the performance scenario must run with --release").into());
    }

    let (path, node_count, shape, shape_name) = parse_arguments()?;
    if path.exists() {
        return Err(input_error(format!(
            "refusing to overwrite existing path: {}",
            path.display()
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
    let mut engine = Engine::open(&path)?;
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

    let mut page_samples = Vec::with_capacity(100);
    for _ in 0..100 {
        let started = Instant::now();
        let page = engine.children(
            None,
            Page {
                limit: 100,
                after: None,
            },
        )?;
        std::hint::black_box(page);
        page_samples.push(started.elapsed());
    }
    page_samples.sort_unstable();
    print_duration("root_page_100_p95", page_samples[94]);

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

    let started = Instant::now();
    engine.set_text(created.id, "Updated performance probe".into())?;
    print_duration("set_text", started.elapsed());

    let started = Instant::now();
    engine.move_node(
        created.id,
        Destination {
            parent_id: None,
            placement: Placement::First,
        },
    )?;
    print_duration("move_root_first", started.elapsed());

    let source_text = "Performance [[target]]";
    let label_start = source_text.find("target").expect("reference label");
    let mut source_input = CreateNode::new(source_text);
    source_input.parent_id = Some(created.id);
    source_input.tags = vec!["meeting".into(), "performance".into()];
    source_input.references = vec![ReferenceInput {
        label_start,
        label_end: label_start + "target".len(),
        target_id: created.id,
    }];
    let started = Instant::now();
    let source = engine.create_node(source_input)?;
    print_duration("create_with_metadata", started.elapsed());

    let started = Instant::now();
    engine.set_tags(source.id, vec!["decision".into(), "performance".into()])?;
    print_duration("set_tags", started.elapsed());

    let updated_text = "Updated [[target]]";
    let updated_start = updated_text
        .find("target")
        .expect("updated reference label");
    let started = Instant::now();
    engine.set_content(
        source.id,
        updated_text.into(),
        vec![ReferenceInput {
            label_start: updated_start,
            label_end: updated_start + "target".len(),
            target_id: created.id,
        }],
    )?;
    print_duration("set_content", started.elapsed());

    let started = Instant::now();
    let metadata_page = engine.children(Some(created.id), Page::default())?;
    print_duration("metadata_page", started.elapsed());
    println!("metadata_page_nodes\t{}\tcount", metadata_page.nodes.len());

    let started = Instant::now();
    let node_path = engine.path(source.id)?;
    print_duration("path", started.elapsed());
    println!("path_nodes\t{}\tcount", node_path.len());

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

fn input_error(message: impl Into<String>) -> IoError {
    IoError::new(ErrorKind::InvalidInput, message.into())
}
