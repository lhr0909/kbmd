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
