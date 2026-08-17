use std::path::Path;
use std::process::{Command, Output};

fn vrac(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vrac"))
        .args(arguments)
        .output()
        .expect("run vrac CLI")
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("temporary path is UTF-8")
}

#[test]
fn help_presents_the_product_workspace_and_database_boundaries() {
    let help = vrac(&["--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("help is UTF-8");
    assert!(help.contains("vrac --workspace <provider-folder>"));
    assert!(help.contains("vrac workspace select"));
    assert!(help.contains("vrac db <command>"));
    assert!(!help.contains("vrac tui"));

    let database_help = vrac(&["db", "--help"]);
    assert!(database_help.status.success());
    let database_help = String::from_utf8(database_help.stdout).expect("help is UTF-8");
    assert!(database_help.contains("vrac db add <file>"));
    assert!(database_help.contains("vrac db check <file>"));
    assert!(!database_help.contains("generate"));

    let workspace_help = vrac(&["workspace", "--help"]);
    assert!(workspace_help.status.success());
    assert_eq!(
        String::from_utf8_lossy(&workspace_help.stdout),
        "Usage: vrac workspace select\n"
    );
}

#[test]
fn version_matches_the_workspace_package() {
    let version = vrac(&["--version"]);
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        format!("vrac {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(version.stderr.is_empty());

    let extra_argument = vrac(&["--version", "unexpected"]);
    assert_eq!(extra_argument.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&extra_argument.stderr).contains("--version accepts no arguments")
    );
}

#[test]
fn the_cli_initializes_edits_moves_and_checks_a_database() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let database = directory.path().join("cli.vrac");
    let database = path_text(&database);

    let init = vrac(&["db", "init", database]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );

    let add_root = vrac(&["db", "add", database, "root"]);
    assert!(
        add_root.status.success(),
        "{}",
        String::from_utf8_lossy(&add_root.stderr)
    );
    let root_id = String::from_utf8(add_root.stdout)
        .expect("root id is UTF-8")
        .trim()
        .to_owned();

    let add_child = vrac(&["db", "add", database, "--parent", &root_id, "child"]);
    assert!(
        add_child.status.success(),
        "{}",
        String::from_utf8_lossy(&add_child.stderr)
    );
    let child_id = String::from_utf8(add_child.stdout)
        .expect("child id is UTF-8")
        .trim()
        .to_owned();

    let edit = vrac(&["db", "set-text", database, &child_id, "updated child"]);
    assert!(
        edit.status.success(),
        "{}",
        String::from_utf8_lossy(&edit.stderr)
    );

    let children = vrac(&["db", "children", database, "--parent", &root_id]);
    assert!(children.status.success());
    let children = String::from_utf8(children.stdout).expect("children output is UTF-8");
    assert!(children.contains(&child_id));
    assert!(children.ends_with("\tupdated child\n"));

    let move_to_root = vrac(&["db", "move", database, &child_id, "--first"]);
    assert!(move_to_root.status.success());

    let roots = vrac(&["db", "children", database]);
    let roots = String::from_utf8(roots.stdout).expect("roots output is UTF-8");
    assert!(roots.starts_with(&child_id));
    assert!(roots.contains(&root_id));
    assert!(roots.contains(&child_id));

    let check = vrac(&["db", "check", database]);
    assert!(check.status.success());
    assert_eq!(String::from_utf8_lossy(&check.stdout), "ok\t3 nodes\n");
}

#[test]
fn invalid_cli_input_has_a_distinct_exit_code() {
    let output = vrac(&["db", "children"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("children expects a file"));
}

#[test]
fn documented_engine_and_integrity_exit_codes_are_stable() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let database = directory.path().join("exit-codes.vrac");
    let database = path_text(&database);
    assert!(vrac(&["db", "init", database]).status.success());

    let missing = vrac(&["db", "node", database, "00000000000000000000000000000000"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("node not found"));

    std::fs::write(
        database,
        include_bytes!("fixtures/rootless-cycle.vrac").as_slice(),
    )
    .expect("copy invalid database fixture");

    let check = vrac(&["db", "check", database]);
    assert_eq!(check.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&check.stdout).contains("unreachable\t2"));
}

#[test]
fn children_can_stream_every_page_without_exposing_cursors() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let database = directory.path().join("large.vrac");
    let database = database.to_str().expect("database path is UTF-8");

    let mut engine = vrac_engine::Engine::open(database).expect("open test database");
    engine
        .generate_nodes(1001, vrac_engine::GenerateShape::Wide)
        .expect("generate paginated test data");
    drop(engine);

    let limited = vrac(&["db", "children", database, "--limit", "2"]);
    assert!(limited.status.success());
    assert_eq!(limited.stdout.split(|byte| *byte == b'\n').count(), 3);
    assert!(
        String::from_utf8_lossy(&limited.stderr)
            .contains("more children are available; use --all to print them")
    );

    let all = vrac(&["db", "children", database, "--all"]);
    assert!(all.status.success());
    assert_eq!(all.stdout.split(|byte| *byte == b'\n').count(), 1003);
    assert!(all.stderr.is_empty());

    let conflicting = vrac(&["db", "children", database, "--all", "--limit", "10"]);
    assert_eq!(conflicting.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&conflicting.stderr)
            .contains("--all cannot be combined with --limit")
    );
}

#[test]
fn the_cli_accepts_relative_placement_and_rejects_conflicting_options() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let database = directory.path().join("placement.vrac");
    let database = path_text(&database);
    assert!(vrac(&["db", "init", database]).status.success());

    let last = vrac(&["db", "add", database, "last"]);
    assert!(last.status.success());
    let last_id = String::from_utf8(last.stdout)
        .expect("last id is UTF-8")
        .trim()
        .to_owned();
    let first = vrac(&["db", "add", database, "--before", &last_id, "first"]);
    assert!(first.status.success());
    let first_id = String::from_utf8(first.stdout)
        .expect("first id is UTF-8")
        .trim()
        .to_owned();

    let roots = vrac(&["db", "children", database]);
    assert!(roots.status.success());
    let roots = String::from_utf8_lossy(&roots.stdout);
    assert!(roots.find(&first_id) < roots.find(&last_id));

    let conflicting = vrac(&["db", "add", database, "--first", "--last", "invalid"]);
    assert_eq!(conflicting.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&conflicting.stderr).contains("conflicts"));
}

#[test]
fn removed_experimental_commands_are_not_accepted_as_aliases() {
    for arguments in [
        &["tui"][..],
        &["workspace"][..],
        &["init", "old.vrac"][..],
        &["generate", "old.vrac", "--nodes", "1"][..],
    ] {
        let output = vrac(arguments);
        assert_eq!(
            output.status.code(),
            Some(2),
            "unexpected status for {arguments:?}"
        );
    }
}

#[test]
fn product_options_and_workspace_selection_have_strict_shapes() {
    let missing_folder = vrac(&["--workspace"]);
    assert_eq!(missing_folder.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&missing_folder.stderr)
            .contains("--workspace expects exactly one provider folder")
    );

    let extra_selection_argument = vrac(&["workspace", "select", "unexpected"]);
    assert_eq!(extra_selection_argument.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&extra_selection_argument.stderr)
            .contains("workspace select accepts no arguments")
    );

    let selection = vrac(&["workspace", "select"]);
    assert_eq!(selection.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&selection.stderr)
            .contains("workspace selection needs an interactive terminal")
    );
}
