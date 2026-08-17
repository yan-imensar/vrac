use std::error::Error as StdError;
use std::fmt;
use std::process::ExitCode;

use vrac_engine::{
    CheckIssue, CreateNode, Destination, Engine, MAX_PAGE_SIZE, Node, NodeId, Page, Placement,
};

const ROOT_USAGE: &str = "\
Vrac, a local-first outliner

Usage:
  vrac
  vrac --workspace <provider-folder>
  vrac workspace select
  vrac db <command> [arguments]
  vrac --help
  vrac --version

Run `vrac db --help` for the direct database commands.
";

const DB_USAGE: &str = "\
Direct database commands

Usage:
  vrac db init <file>
  vrac db add <file> [--parent <id>] [--first|--last|--before <id>|--after <id>] <text>
  vrac db node <file> <id>
  vrac db children <file> [--parent <id>] [--limit <n>|--all]
  vrac db set-text <file> <id> <text>
  vrac db move <file> <id> [--parent <id>] [--first|--last|--before <id>|--after <id>]
  vrac db check <file>
";

const WORKSPACE_USAGE: &str = "Usage: vrac workspace select\n";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.is_empty() {
        return tui_exit(launch_tui(vrac_tui::WorkspaceSelection::Remembered));
    }

    match arguments[0].as_str() {
        "--help" | "-h" => {
            if arguments.len() != 1 {
                return usage_exit("--help accepts no arguments");
            }
            print!("{ROOT_USAGE}");
            return ExitCode::SUCCESS;
        }
        "--version" | "-V" => {
            if arguments.len() != 1 {
                return usage_exit("--version accepts no arguments");
            }
            println!("vrac {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        "--workspace" => {
            if arguments.len() != 2 {
                return usage_exit("--workspace expects exactly one provider folder");
            }
            return tui_exit(launch_tui(vrac_tui::WorkspaceSelection::Folder(
                arguments[1].as_str().into(),
            )));
        }
        "workspace" => return workspace_exit(&arguments[1..]),
        "db" => {}
        command => return usage_exit(&format!("unknown command: {command}")),
    }

    match run_db(&arguments[1..]) {
        Ok(exit_code) => exit_code,
        Err(CliError::Usage(message)) => {
            eprintln!("error: {message}\n\n{DB_USAGE}");
            ExitCode::from(2)
        }
        Err(CliError::Engine(error)) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_exit(arguments: &[String]) -> ExitCode {
    match arguments {
        [argument] if matches!(argument.as_str(), "--help" | "-h") => {
            print!("{WORKSPACE_USAGE}");
            ExitCode::SUCCESS
        }
        [command] if command == "select" => {
            tui_exit(launch_tui(vrac_tui::WorkspaceSelection::Select))
        }
        [] => usage_exit("workspace expects the `select` command"),
        [command, ..] if command != "select" => {
            usage_exit(&format!("unknown workspace command: {command}"))
        }
        _ => usage_exit("workspace select accepts no arguments"),
    }
}

fn usage_exit(message: &str) -> ExitCode {
    eprintln!("error: {message}\n\n{ROOT_USAGE}");
    ExitCode::from(2)
}

fn launch_tui(workspace: vrac_tui::WorkspaceSelection) -> Result<(), Box<dyn StdError>> {
    let data_directory = dirs::data_local_dir()
        .map(|directory| directory.join("vrac"))
        .ok_or("cannot determine the local application-data directory")?;
    vrac_tui::run(vrac_tui::LaunchOptions {
        data_directory,
        workspace,
    })
}

fn tui_exit(result: Result<(), Box<dyn StdError>>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_db(arguments: &[String]) -> Result<ExitCode, CliError> {
    let command = arguments
        .first()
        .ok_or_else(|| CliError::Usage("db expects a command".into()))?;
    let arguments = &arguments[1..];
    match command.as_str() {
        "--help" | "-h" => {
            if !arguments.is_empty() {
                return Err(CliError::Usage("db --help accepts no arguments".into()));
            }
            print!("{DB_USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        "init" => command_init(arguments),
        "add" => command_add(arguments),
        "node" => command_node(arguments),
        "children" => command_children(arguments),
        "set-text" => command_set_text(arguments),
        "move" => command_move(arguments),
        "check" => command_check(arguments),
        _ => Err(CliError::Usage(format!(
            "unknown database command: {command}"
        ))),
    }
}

fn command_init(arguments: &[String]) -> Result<ExitCode, CliError> {
    expect_argument_count(arguments, 1, "init expects a file")?;
    Engine::open(&arguments[0])?;
    println!("initialized\t{}", arguments[0]);
    Ok(ExitCode::SUCCESS)
}

fn command_add(arguments: &[String]) -> Result<ExitCode, CliError> {
    if arguments.len() < 2 {
        return Err(CliError::Usage("add expects a file and text".into()));
    }

    let path = &arguments[0];
    let mut parent_id = None;
    let mut placement = None;
    let mut text_parts = Vec::new();
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--parent" => {
                if parent_id.is_some() {
                    return Err(CliError::Usage(
                        "--parent was provided more than once".into(),
                    ));
                }
                parent_id = Some(parse_id(option_value(arguments, &mut index, "--parent")?)?);
            }
            "--first" => set_placement(&mut placement, Placement::First, "--first")?,
            "--last" => set_placement(&mut placement, Placement::Last, "--last")?,
            "--before" => {
                let id = parse_id(option_value(arguments, &mut index, "--before")?)?;
                set_placement(&mut placement, Placement::Before(id), "--before")?;
            }
            "--after" => {
                let id = parse_id(option_value(arguments, &mut index, "--after")?)?;
                set_placement(&mut placement, Placement::After(id), "--after")?;
            }
            "--" => {
                text_parts.extend_from_slice(&arguments[index + 1..]);
                break;
            }
            option if option.starts_with("--") => {
                return Err(CliError::Usage(format!("unknown option: {option}")));
            }
            _ => text_parts.push(arguments[index].clone()),
        }
        index += 1;
    }

    if text_parts.is_empty() {
        return Err(CliError::Usage("missing node text".into()));
    }

    let mut engine = Engine::open(path)?;
    let mut input = CreateNode::new(text_parts.join(" "));
    input.parent_id = parent_id;
    input.placement = placement.unwrap_or_default();
    let node = engine.create_node(input)?;
    println!("{}", node.id);
    Ok(ExitCode::SUCCESS)
}

fn command_node(arguments: &[String]) -> Result<ExitCode, CliError> {
    expect_argument_count(arguments, 2, "node expects a file and an identifier")?;
    let id = parse_id(&arguments[1])?;
    let engine = Engine::open(&arguments[0])?;
    let node = engine
        .node(id)?
        .ok_or(vrac_engine::Error::NodeNotFound(id))?;
    print_node(&node);
    Ok(ExitCode::SUCCESS)
}

fn command_children(arguments: &[String]) -> Result<ExitCode, CliError> {
    if arguments.is_empty() {
        return Err(CliError::Usage("children expects a file".into()));
    }

    let path = &arguments[0];
    let mut parent_id = None;
    let mut limit = None;
    let mut all = false;
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--parent" => {
                if parent_id.is_some() {
                    return Err(CliError::Usage(
                        "--parent was provided more than once".into(),
                    ));
                }
                parent_id = Some(parse_id(option_value(arguments, &mut index, "--parent")?)?);
            }
            "--limit" => {
                if limit.is_some() {
                    return Err(CliError::Usage(
                        "--limit was provided more than once".into(),
                    ));
                }
                let value = option_value(arguments, &mut index, "--limit")?;
                limit = Some(
                    value
                        .parse()
                        .map_err(|_| CliError::Usage(format!("invalid limit: {value}")))?,
                );
            }
            "--all" => {
                if all {
                    return Err(CliError::Usage("--all was provided more than once".into()));
                }
                all = true;
            }
            option => return Err(CliError::Usage(format!("unknown option: {option}"))),
        }
        index += 1;
    }

    if all && limit.is_some() {
        return Err(CliError::Usage(
            "--all cannot be combined with --limit".into(),
        ));
    }

    let engine = Engine::open(path)?;
    let page_limit = limit.unwrap_or(if all {
        MAX_PAGE_SIZE
    } else {
        Page::default().limit
    });
    let mut after = None;
    loop {
        let page = engine.children(
            parent_id,
            Page {
                limit: page_limit,
                after,
            },
        )?;
        for node in page.nodes {
            print_node(&node);
        }

        match page.next {
            Some(next) if all => after = Some(next),
            Some(_) => {
                eprintln!("more children are available; use --all to print them");
                break;
            }
            None => break,
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn command_set_text(arguments: &[String]) -> Result<ExitCode, CliError> {
    if arguments.len() < 3 {
        return Err(CliError::Usage(
            "set-text expects a file, an identifier, and text".into(),
        ));
    }

    let id = parse_id(&arguments[1])?;
    let mut engine = Engine::open(&arguments[0])?;
    engine.set_text(id, arguments[2..].join(" "))?;
    Ok(ExitCode::SUCCESS)
}

fn command_move(arguments: &[String]) -> Result<ExitCode, CliError> {
    if arguments.len() < 2 {
        return Err(CliError::Usage(
            "move expects a file and an identifier".into(),
        ));
    }

    let path = &arguments[0];
    let id = parse_id(&arguments[1])?;
    let mut parent_id = None;
    let mut placement = None;
    let mut index = 2;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--parent" => {
                if parent_id.is_some() {
                    return Err(CliError::Usage(
                        "--parent was provided more than once".into(),
                    ));
                }
                parent_id = Some(parse_id(option_value(arguments, &mut index, "--parent")?)?);
            }
            "--first" => set_placement(&mut placement, Placement::First, "--first")?,
            "--last" => set_placement(&mut placement, Placement::Last, "--last")?,
            "--before" => {
                let reference = parse_id(option_value(arguments, &mut index, "--before")?)?;
                set_placement(&mut placement, Placement::Before(reference), "--before")?;
            }
            "--after" => {
                let reference = parse_id(option_value(arguments, &mut index, "--after")?)?;
                set_placement(&mut placement, Placement::After(reference), "--after")?;
            }
            option => return Err(CliError::Usage(format!("unknown option: {option}"))),
        }
        index += 1;
    }

    let mut engine = Engine::open(path)?;
    engine.move_node(
        id,
        Destination {
            parent_id,
            placement: placement.unwrap_or_default(),
        },
    )?;
    Ok(ExitCode::SUCCESS)
}

fn command_check(arguments: &[String]) -> Result<ExitCode, CliError> {
    expect_argument_count(arguments, 1, "check expects a file")?;
    let engine = Engine::open(&arguments[0])?;
    let report = engine.check()?;

    if report.is_ok() {
        println!("ok\t{} nodes", report.node_count);
        return Ok(ExitCode::SUCCESS);
    }

    println!("invalid\t{} nodes", report.node_count);
    for issue in report.issues {
        match issue {
            CheckIssue::SqliteIntegrity(message) => {
                println!("sqlite\t{}", escape_text(&message));
            }
            CheckIssue::ForeignKey {
                table,
                rowid,
                parent,
                foreign_key_index,
            } => println!(
                "foreign-key\t{}\t{}\t{}\t{}",
                escape_text(&table),
                rowid.map_or_else(|| "-".into(), |value| value.to_string()),
                escape_text(&parent),
                foreign_key_index
            ),
            CheckIssue::UnreachableNodes(count) => {
                println!("unreachable\t{count}");
            }
            CheckIssue::NonCanonicalTag { node_id, tag } => {
                println!("invalid-tag\t{node_id}\t{}", escape_text(&tag));
            }
            CheckIssue::InvalidReference {
                source_id,
                start,
                end,
            } => {
                println!("invalid-reference\t{source_id}\t{start}\t{end}");
            }
            CheckIssue::InvalidSystemNode { node_id } => {
                println!("invalid-system-node\t{node_id}");
            }
            CheckIssue::MissingJournal => println!("missing-journal"),
            CheckIssue::InvalidSyncState(reason) => {
                println!("invalid-sync-state\t{}", escape_text(&reason));
            }
            CheckIssue::AdditionalIssuesOmitted => println!("issues-omitted"),
        }
    }
    Ok(ExitCode::from(3))
}

fn print_node(node: &Node) {
    let parent = node
        .parent_id
        .map_or_else(|| "-".into(), |id| id.to_string());
    println!("{}\t{}\t{}", node.id, parent, escape_text(&node.text));
}

fn escape_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character => escaped.push(character),
        }
    }
    escaped
}

fn expect_argument_count(
    arguments: &[String],
    expected: usize,
    message: &str,
) -> Result<(), CliError> {
    if arguments.len() != expected {
        return Err(CliError::Usage(message.into()));
    }
    Ok(())
}

fn option_value<'a>(
    arguments: &'a [String],
    index: &mut usize,
    option: &str,
) -> Result<&'a str, CliError> {
    *index += 1;
    arguments
        .get(*index)
        .map(String::as_str)
        .ok_or_else(|| CliError::Usage(format!("missing value after {option}")))
}

fn set_placement(
    current: &mut Option<Placement>,
    placement: Placement,
    option: &str,
) -> Result<(), CliError> {
    if current.is_some() {
        return Err(CliError::Usage(format!(
            "{option} conflicts with another placement option"
        )));
    }
    *current = Some(placement);
    Ok(())
}

fn parse_id(value: &str) -> Result<NodeId, CliError> {
    value
        .parse()
        .map_err(|error| CliError::Usage(format!("invalid identifier ({value}): {error}")))
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Engine(vrac_engine::Error),
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => formatter.write_str(message),
            Self::Engine(error) => error.fmt(formatter),
        }
    }
}

impl StdError for CliError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Usage(_) => None,
            Self::Engine(error) => Some(error),
        }
    }
}

impl From<vrac_engine::Error> for CliError {
    fn from(error: vrac_engine::Error) -> Self {
        Self::Engine(error)
    }
}
