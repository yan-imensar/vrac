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

    let move_to_root = vrac(&["move", database, &child_id, "--position", "5"]);
    assert!(move_to_root.status.success());

    let roots = vrac(&["children", database]);
    let roots = String::from_utf8(roots.stdout).expect("roots output is UTF-8");
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
