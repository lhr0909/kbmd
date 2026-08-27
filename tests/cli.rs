use std::fs;
use std::path::Path;

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
