use std::fs;
use std::path::Path;
use std::process::{Command as ProcessCommand, Stdio};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn kbmd(root: &Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("kbmd"));
    command.arg("--project").arg(root);
    command
}

fn initialized() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    kbmd(directory.path())
        .args([
            "init",
            "Demo",
            "--prefix",
            "DEMO",
            "--statuses",
            "Inbox,Doing,Done",
        ])
        .assert()
        .success();
    directory
}

#[test]
fn complete_card_cli_workflow() {
    let directory = initialized();
    kbmd(directory.path())
        .args([
            "add",
            "Ship it",
            "--label",
            "mvp",
            "--section",
            "Plan=Build the thing",
            "--check",
            "Release=macOS",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created DEMO-1"));

    kbmd(directory.path())
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("DEMO-1"))
        .stdout(predicate::str::contains("[0/1]"));

    kbmd(directory.path())
        .args(["move", "demo-1", "doing"])
        .assert()
        .success();
    kbmd(directory.path())
        .args(["show", "DEMO-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Status: Doing"))
        .stdout(predicate::str::contains("## Plan"))
        .stdout(predicate::str::contains("## Release"));
}

#[test]
fn arbitrary_sections_and_checklists_are_editable() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Flexible"])
        .assert()
        .success();
    kbmd(directory.path())
        .args([
            "section",
            "set",
            "DEMO-1",
            "Launch ritual",
            "Watch the metrics.",
        ])
        .assert()
        .success();
    kbmd(directory.path())
        .args(["check", "add", "DEMO-1", "Launch ritual", "Notify support"])
        .assert()
        .success();
    kbmd(directory.path())
        .args(["check", "check", "DEMO-1", "Launch ritual", "1"])
        .assert()
        .success();

    kbmd(directory.path())
        .args(["section", "show", "DEMO-1", "Launch ritual"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Watch the metrics."))
        .stdout(predicate::str::contains("- [x] Notify support"));
}

#[test]
fn custom_fields_keep_string_and_typed_yaml_semantics() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Fields", "--field", "team=007"])
        .assert()
        .success();
    kbmd(directory.path())
        .args([
            "field",
            "set",
            "DEMO-1",
            "estimate",
            "{points: 3, risky: false}",
            "--yaml",
        ])
        .assert()
        .success();

    let output = kbmd(directory.path())
        .args(["show", "DEMO-1", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["data"]["metadata"]["team"], "007");
    assert_eq!(json["data"]["metadata"]["estimate"]["points"], 3);
    assert_eq!(json["data"]["metadata"]["estimate"]["risky"], false);
}

#[test]
fn body_can_be_read_from_stdin() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Piped", "--body-file", "-"])
        .write_stdin("## Any heading\n\n- [ ] Any checklist\n")
        .assert()
        .success();

    kbmd(directory.path())
        .args(["show", "DEMO-1", "--raw"])
        .assert()
        .success()
        .stdout(predicate::str::contains("## Any heading"));
}

#[test]
fn invalid_status_does_not_create_a_card() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Nope", "--status", "Unknown"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown status"));

    assert_eq!(
        fs::read_dir(directory.path().join(".kbmd/cards"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn validation_reports_malformed_files() {
    let directory = initialized();
    fs::write(
        directory.path().join(".kbmd/cards/broken.md"),
        "not frontmatter\n",
    )
    .unwrap();

    kbmd(directory.path())
        .args(["validate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("broken.md"))
        .stderr(predicate::str::contains("1 issue"));
}

#[test]
fn read_only_show_survives_an_unrelated_malformed_card() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Still readable"])
        .assert()
        .success();
    fs::write(
        directory.path().join(".kbmd/cards/broken.md"),
        "not frontmatter\n",
    )
    .unwrap();

    kbmd(directory.path())
        .args(["show", "DEMO-1", "--raw"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Still readable"));
    kbmd(directory.path())
        .args(["show", "DEMO-1", "--path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("DEMO-1.md"));
}

#[test]
fn board_json_has_a_versioned_envelope() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "JSON card"])
        .assert()
        .success();
    let output = kbmd(directory.path())
        .args(["board", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["data"]["name"], "Demo");
    assert_eq!(
        json["data"]["columns"][0]["cards"][0]["metadata"]["id"],
        "DEMO-1"
    );
}

#[test]
fn concurrent_cli_creates_do_not_collide() {
    let directory = initialized();
    let binary = assert_cmd::cargo::cargo_bin!("kbmd");
    let children = (0..8)
        .map(|index| {
            ProcessCommand::new(binary)
                .arg("--project")
                .arg(directory.path())
                .arg("add")
                .arg(format!("Concurrent {index}"))
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();

    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "concurrent create failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = kbmd(directory.path())
        .args(["list", "--json"])
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["data"].as_array().unwrap().len(), 8);
}

#[test]
fn failed_checklist_mutation_leaves_file_unchanged() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Safe", "--check", "Release=Existing"])
        .assert()
        .success();
    let path = directory.path().join(".kbmd/cards/DEMO-1.md");
    let before = fs::read(&path).unwrap();

    kbmd(directory.path())
        .args(["check", "toggle", "DEMO-1", "Release", "99"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("was not found"));

    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn valid_noncanonical_status_casing_still_appears_everywhere() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Manual casing", "--status", "Doing"])
        .assert()
        .success();
    let path = directory.path().join(".kbmd/cards/DEMO-1.md");
    let source = fs::read_to_string(&path).unwrap();
    fs::write(&path, source.replace("status: Doing", "status: doing")).unwrap();

    kbmd(directory.path()).args(["validate"]).assert().success();
    kbmd(directory.path())
        .args(["list", "--status", "Doing"])
        .assert()
        .success()
        .stdout(predicate::str::contains("DEMO-1"));
    kbmd(directory.path())
        .args(["board"])
        .assert()
        .success()
        .stdout(predicate::str::contains("DEMO-1"));

    let output = kbmd(directory.path())
        .args(["board", "--json"])
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        json["data"]["columns"][1]["cards"][0]["metadata"]["id"],
        "DEMO-1"
    );
}

#[test]
fn init_defaults_to_current_directory_name_and_normalizes_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let expected_name = directory.path().file_name().unwrap().to_str().unwrap();
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("kbmd"));
    command
        .current_dir(directory.path())
        .args(["init", "--prefix", " KB "])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Initialized {expected_name}"
        )));

    kbmd(directory.path())
        .args(["add", "Normalized"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created KB-1"));
    assert!(directory.path().join(".kbmd/cards/KB-1.md").is_file());
}

#[test]
fn help_advertises_the_tui_and_display_controls_are_rejected() {
    Command::new(assert_cmd::cargo::cargo_bin!("kbmd"))
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("tui"));

    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "first line\nsecond line"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("control characters"));
    assert!(
        fs::read_dir(directory.path().join(".kbmd/cards"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn global_checklist_commands_can_mutate_preamble_items() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Preamble", "--body", "- [ ] first\n- [ ] second\n"])
        .assert()
        .success();

    kbmd(directory.path())
        .args(["check", "check-global", "DEMO-1", "1"])
        .assert()
        .success();
    kbmd(directory.path())
        .args(["check", "toggle-global", "DEMO-1", "2"])
        .assert()
        .success();
    kbmd(directory.path())
        .args(["check", "uncheck-global", "DEMO-1", "1"])
        .assert()
        .success();
    kbmd(directory.path())
        .args(["check", "remove-global", "DEMO-1", "1"])
        .assert()
        .success();

    kbmd(directory.path())
        .args(["show", "DEMO-1", "--raw"])
        .assert()
        .success()
        .stdout(predicate::str::contains("- [x] second"))
        .stdout(predicate::str::contains("first").not());
}

#[test]
fn markdown_and_field_values_may_begin_with_hyphens() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Hyphen values"])
        .assert()
        .success();

    kbmd(directory.path())
        .args(["section", "set", "DEMO-1", "Plan", "- [ ] first"])
        .assert()
        .success();
    kbmd(directory.path())
        .args(["check", "add", "DEMO-1", "Plan", "- follow-up"])
        .assert()
        .success();
    kbmd(directory.path())
        .args(["field", "set", "DEMO-1", "offset", "-1", "--yaml"])
        .assert()
        .success();

    let output = kbmd(directory.path())
        .args(["show", "DEMO-1", "--json"])
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["data"]["metadata"]["offset"], -1);
    assert!(
        json["data"]["body"]
            .as_str()
            .unwrap()
            .contains("- [ ] - follow-up")
    );
}

#[test]
fn flat_comments_support_argument_file_stdin_and_json_workflows() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Discuss", "--body", "## Notes\n\nKeep this\n"])
        .assert()
        .success();

    let first_output = kbmd(directory.path())
        .args([
            "comment",
            "add",
            "DEMO-1",
            "First observation",
            "--author",
            "Alice",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(first_output.status.success());
    let first: Value = serde_json::from_slice(&first_output.stdout).unwrap();
    assert_eq!(first["schema_version"], 1);
    assert_eq!(first["data"]["version"], 1);
    assert_eq!(first["data"]["author"], "Alice");
    assert_eq!(first["data"]["body"], "First observation");
    assert!(first["data"].get("parent").is_none());
    assert!(first["data"].get("reply_to").is_none());
    let first_id = first["data"]["id"].as_str().unwrap().to_owned();

    kbmd(directory.path())
        .args([
            "comments", "add", "DEMO-1", "--file", "-", "--author", "Bob",
        ])
        .write_stdin("### A heading inside discussion\n\n- [ ] not actionable\n")
        .assert()
        .success();

    let comment_file = directory.path().join("third-comment.md");
    fs::write(&comment_file, "A **file-backed** comment.\n").unwrap();
    kbmd(directory.path())
        .arg("comment")
        .arg("add")
        .arg("DEMO-1")
        .arg("--file")
        .arg(&comment_file)
        .args(["--author", "Carol"])
        .assert()
        .success();

    let list_output = kbmd(directory.path())
        .args(["comment", "list", "DEMO-1", "--json"])
        .output()
        .unwrap();
    assert!(list_output.status.success());
    let list: Value = serde_json::from_slice(&list_output.stdout).unwrap();
    let comments = list["data"].as_array().unwrap();
    assert_eq!(comments.len(), 3);
    assert_eq!(comments[0]["id"], first_id);
    assert_eq!(comments[0]["author"], "Alice");
    assert_eq!(comments[1]["author"], "Bob");
    assert_eq!(
        comments[1]["body"],
        "### A heading inside discussion\n\n- [ ] not actionable"
    );
    assert_eq!(comments[2]["author"], "Carol");
    assert_eq!(comments[2]["body"], "A **file-backed** comment.");

    let show_output = kbmd(directory.path())
        .args(["comment", "show", "DEMO-1", &first_id, "--json"])
        .output()
        .unwrap();
    assert!(show_output.status.success());
    let shown: Value = serde_json::from_slice(&show_output.stdout).unwrap();
    assert_eq!(shown["data"], comments[0]);

    let checks_output = kbmd(directory.path())
        .args(["check", "list", "DEMO-1", "--json"])
        .output()
        .unwrap();
    let checks: Value = serde_json::from_slice(&checks_output.stdout).unwrap();
    assert!(checks["data"].as_array().unwrap().is_empty());

    let sections_output = kbmd(directory.path())
        .args(["section", "list", "DEMO-1", "--json"])
        .output()
        .unwrap();
    let sections: Value = serde_json::from_slice(&sections_output.stdout).unwrap();
    assert_eq!(sections["data"].as_array().unwrap().len(), 2);
    assert!(
        sections["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|section| { section["title"] != "A heading inside discussion" })
    );

    let raw = fs::read_to_string(directory.path().join(".kbmd/cards/DEMO-1.md")).unwrap();
    assert_eq!(raw.matches("<!-- kbmd:comments:v1 -->").count(), 1);
    assert_eq!(raw.matches("<!-- kbmd:comment:v1").count(), 3);
}

#[test]
fn comments_have_no_reply_command_or_thread_fields() {
    Command::new(assert_cmd::cargo::cargo_bin!("kbmd"))
        .args(["comment", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("add"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("show"))
        .stdout(predicate::str::contains("reply").not());

    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Flat"])
        .assert()
        .success();
    kbmd(directory.path())
        .args(["comment", "reply", "DEMO-1", "anything"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

#[test]
fn comment_author_resolution_prefers_explicit_then_environment() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Authors"])
        .assert()
        .success();

    let environment = kbmd(directory.path())
        .env("KBMD_AUTHOR", "Environment Author")
        .args(["comment", "add", "DEMO-1", "from env", "--json"])
        .output()
        .unwrap();
    assert!(environment.status.success());
    let environment: Value = serde_json::from_slice(&environment.stdout).unwrap();
    assert_eq!(environment["data"]["author"], "Environment Author");

    let explicit = kbmd(directory.path())
        .env("KBMD_AUTHOR", "Environment Author")
        .args([
            "comment",
            "add",
            "DEMO-1",
            "explicit wins",
            "--author",
            "Explicit Author",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(explicit.status.success());
    let explicit: Value = serde_json::from_slice(&explicit.stdout).unwrap();
    assert_eq!(explicit["data"]["author"], "Explicit Author");

    kbmd(directory.path())
        .env("KBMD_AUTHOR", "bad\nname")
        .args(["comment", "add", "DEMO-1", "must fail"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid KBMD_AUTHOR"));
}

#[test]
fn concurrent_comment_additions_all_survive_with_unique_ids() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Concurrent discussion"])
        .assert()
        .success();
    let binary = assert_cmd::cargo::cargo_bin!("kbmd");
    let children = (0..8)
        .map(|index| {
            ProcessCommand::new(binary)
                .arg("--project")
                .arg(directory.path())
                .args(["comment", "add", "DEMO-1"])
                .arg(format!("Concurrent comment {index}"))
                .args(["--author", "Load test"])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();

    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "concurrent comment failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = kbmd(directory.path())
        .args(["comment", "list", "DEMO-1", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let comments = json["data"].as_array().unwrap();
    assert_eq!(comments.len(), 8);
    let ids = comments
        .iter()
        .map(|comment| comment["id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 8);
}

#[test]
fn generic_markdown_commands_cannot_damage_comments() {
    let directory = initialized();
    kbmd(directory.path())
        .args([
            "add",
            "Protected",
            "--body",
            "# Project\n\n- [ ] real task\n\n## Comments\n\nLegacy context\n",
        ])
        .assert()
        .success();
    kbmd(directory.path())
        .args([
            "comment",
            "add",
            "DEMO-1",
            "- [ ] discussion only",
            "--author",
            "Alice",
        ])
        .assert()
        .success();

    kbmd(directory.path())
        .args(["check", "toggle", "DEMO-1", "Project", "1"])
        .assert()
        .success();
    kbmd(directory.path())
        .args(["check", "toggle-global", "DEMO-1", "1"])
        .assert()
        .success();
    kbmd(directory.path())
        .args(["check", "add", "DEMO-1", "Project", "second task"])
        .assert()
        .success();

    let path = directory.path().join(".kbmd/cards/DEMO-1.md");
    let before = fs::read(&path).unwrap();
    for arguments in [
        vec!["section", "set", "DEMO-1", "Project", "replacement"],
        vec!["section", "remove", "DEMO-1", "Comments"],
        vec!["check", "add", "DEMO-1", "Comments", "unsafe"],
        vec![
            "section",
            "set",
            "DEMO-1",
            "Notes",
            "<!-- kbmd:comments:v1 -->",
        ],
    ] {
        kbmd(directory.path()).args(arguments).assert().failure();
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    let output = kbmd(directory.path())
        .args(["check", "list", "DEMO-1", "--json"])
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let checks = json["data"].as_array().unwrap();
    assert_eq!(checks.len(), 2);
    assert!(checks.iter().all(|item| item["text"] != "discussion only"));
}

#[test]
fn malformed_requested_comments_are_reported_by_path() {
    let directory = initialized();
    kbmd(directory.path())
        .args(["add", "Broken discussion"])
        .assert()
        .success();
    kbmd(directory.path())
        .args([
            "comment",
            "add",
            "DEMO-1",
            "Will be malformed",
            "--author",
            "Alice",
        ])
        .assert()
        .success();

    let path = directory.path().join(".kbmd/cards/DEMO-1.md");
    let source = fs::read_to_string(&path).unwrap();
    let malformed = source
        .lines()
        .filter(|line| !line.starts_with("<!-- kbmd:comment:end "))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&path, malformed).unwrap();

    kbmd(directory.path())
        .args(["comment", "list", "DEMO-1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("DEMO-1.md"))
        .stderr(predicate::str::contains("no matching end marker"));
    kbmd(directory.path())
        .args(["validate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("DEMO-1.md"))
        .stderr(predicate::str::contains("no matching end marker"));
}
