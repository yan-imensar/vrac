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
fn the_cli_initializes_edits_moves_and_checks_a_workspace() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let database = directory.path().join("cli.vrac");
    let database = path_text(&database);

    let init = vrac(&["init", database]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );

    let add_root = vrac(&["add", database, "root"]);
    assert!(
        add_root.status.success(),
        "{}",
        String::from_utf8_lossy(&add_root.stderr)
    );
    let root_id = String::from_utf8(add_root.stdout)
        .expect("root id is UTF-8")
        .trim()
        .to_owned();

    let add_child = vrac(&["add", database, "--parent", &root_id, "child"]);
    assert!(
        add_child.status.success(),
        "{}",
        String::from_utf8_lossy(&add_child.stderr)
    );
    let child_id = String::from_utf8(add_child.stdout)
        .expect("child id is UTF-8")
        .trim()
        .to_owned();

    let edit = vrac(&["set-text", database, &child_id, "updated child"]);
    assert!(
        edit.status.success(),
        "{}",
        String::from_utf8_lossy(&edit.stderr)
    );

    let children = vrac(&["children", database, "--parent", &root_id]);
    assert!(children.status.success());
    let children = String::from_utf8(children.stdout).expect("children output is UTF-8");
    assert!(children.contains(&child_id));
    assert!(children.ends_with("\tupdated child\n"));

    let move_to_root = vrac(&["move", database, &child_id, "--first"]);
    assert!(move_to_root.status.success());

    let roots = vrac(&["children", database]);
    let roots = String::from_utf8(roots.stdout).expect("roots output is UTF-8");
    assert!(roots.starts_with(&child_id));
    assert!(roots.contains(&root_id));
    assert!(roots.contains(&child_id));

    let check = vrac(&["check", database]);
    assert!(check.status.success());
    assert_eq!(String::from_utf8_lossy(&check.stdout), "ok\t2 nodes\n");
}

#[test]
fn invalid_cli_input_has_a_distinct_exit_code() {
    let output = vrac(&["children"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("children expects a file"));
}

#[test]
fn documented_engine_and_integrity_exit_codes_are_stable() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let database = directory.path().join("exit-codes.vrac");
    let database = path_text(&database);
    assert!(vrac(&["init", database]).status.success());

    let missing = vrac(&["node", database, "00000000000000000000000000000000"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("node not found"));

    std::fs::write(
        database,
        include_bytes!("fixtures/rootless-cycle.vrac").as_slice(),
    )
    .expect("copy invalid workspace fixture");

    let check = vrac(&["check", database]);
    assert_eq!(check.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&check.stdout).contains("unreachable\t2"));
}

#[test]
fn children_can_stream_every_page_without_exposing_cursors() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let database = directory.path().join("large.vrac");
    let database = database.to_str().expect("database path is UTF-8");

    let generated = vrac(&["generate", database, "--nodes", "1001", "--shape", "wide"]);
    assert!(generated.status.success());

    let limited = vrac(&["children", database, "--limit", "2"]);
    assert!(limited.status.success());
    assert_eq!(limited.stdout.split(|byte| *byte == b'\n').count(), 3);
    assert!(
        String::from_utf8_lossy(&limited.stderr)
            .contains("more children are available; use --all to print them")
    );

    let all = vrac(&["children", database, "--all"]);
    assert!(all.status.success());
    assert_eq!(all.stdout.split(|byte| *byte == b'\n').count(), 1002);
    assert!(all.stderr.is_empty());

    let conflicting = vrac(&["children", database, "--all", "--limit", "10"]);
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
    assert!(vrac(&["init", database]).status.success());

    let last = vrac(&["add", database, "last"]);
    assert!(last.status.success());
    let last_id = String::from_utf8(last.stdout)
        .expect("last id is UTF-8")
        .trim()
        .to_owned();
    let first = vrac(&["add", database, "--before", &last_id, "first"]);
    assert!(first.status.success());
    let first_id = String::from_utf8(first.stdout)
        .expect("first id is UTF-8")
        .trim()
        .to_owned();

    let roots = vrac(&["children", database]);
    assert!(roots.status.success());
    assert!(String::from_utf8_lossy(&roots.stdout).starts_with(&first_id));

    let conflicting = vrac(&["add", database, "--first", "--last", "invalid"]);
    assert_eq!(conflicting.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&conflicting.stderr).contains("conflicts"));
}
