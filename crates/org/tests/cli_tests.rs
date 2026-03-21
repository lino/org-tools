use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn org() -> Command {
    Command::cargo_bin("org").unwrap()
}

#[test]
fn check_clean_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("clean.org");
    fs::write(&file, "* Heading\n\nSome text.\n").unwrap();

    org()
        .args(["fmt", "check"])
        .arg(file.to_str().unwrap())
        .assert()
        .success();
}

#[test]
fn check_dirty_file_exits_1() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("dirty.org");
    fs::write(&file, "* Heading   \nSome text  \n").unwrap();

    org()
        .args(["fmt", "check"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("trailing-whitespace"));
}

#[test]
fn format_in_place() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("format_me.org");
    fs::write(&file, "* Heading   \ntext  \n").unwrap();

    org()
        .args(["fmt", "format"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Formatted:"));

    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(content, "* Heading\ntext\n");
}

#[test]
fn format_check_mode() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("check_format.org");
    fs::write(&file, "* Heading   \n").unwrap();

    org()
        .args(["fmt", "format", "--check"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("Would reformat"));

    // File should NOT be modified.
    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(content, "* Heading   \n");
}

#[test]
fn format_stdout_mode() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("stdout.org");
    fs::write(&file, "text   \n").unwrap();

    org()
        .args(["fmt", "format", "--stdout"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout("text\n");

    // File should NOT be modified.
    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(content, "text   \n");
}

#[test]
fn check_json_output() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("json.org");
    fs::write(&file, "text   \n").unwrap();

    let output = org()
        .args(["fmt", "check", "--format", "json"])
        .arg(file.to_str().unwrap())
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert!(parsed.is_array());
    assert!(!parsed.as_array().unwrap().is_empty());
}

#[test]
fn check_recursive_directory() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("a.org"), "text   \n").unwrap();
    fs::write(sub.join("b.org"), "clean\n").unwrap();
    fs::write(sub.join("not_org.txt"), "text   \n").unwrap();

    org()
        .args(["fmt", "check"])
        .arg(dir.path().to_str().unwrap())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("a.org"))
        .stdout(predicate::str::contains("trailing-whitespace"));
}

#[test]
fn no_files_found_exits_2() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("not_org.txt"), "text\n").unwrap();

    org()
        .args(["fmt", "check"])
        .arg(dir.path().to_str().unwrap())
        .assert()
        .code(2);
}

#[test]
fn check_sample_fixture() {
    org()
        .args(["fmt", "check", "../../tests/fixtures/sample.org"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("heading-level-gap"))
        .stdout(predicate::str::contains("missing-src-language"))
        .stdout(predicate::str::contains("misplaced-property-drawer"));
}

#[test]
fn format_table_alignment() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("table.org");
    fs::write(
        &file,
        "| Name | Age |\n|---+---|\n| Alice | 30 |\n| Bob | 5 |\n",
    )
    .unwrap();

    org()
        .args(["fmt", "format", "--stdout"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout("| Name  | Age |\n|-------+-----|\n| Alice |  30 |\n| Bob   |   5 |\n");
}

#[test]
fn check_fix_mode() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("fix_me.org");
    fs::write(&file, "* Heading   \ntext  \n").unwrap();

    org()
        .args(["fmt", "check", "--fix"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Fixed:"));

    // File should be modified — trailing whitespace removed.
    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(content, "* Heading\ntext\n");
}

// --- update add-id tests ---

#[test]
fn add_id_to_all_entries() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("ids.org");
    fs::write(&file, "* A\n* B\n* C\n").unwrap();

    org()
        .args(["update", "add-id"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 3 IDs"));

    let content = fs::read_to_string(&file).unwrap();
    // All three headings should have :ID: properties.
    assert_eq!(content.matches(":ID:").count(), 3);
    assert_eq!(content.matches(":PROPERTIES:").count(), 3);
    assert_eq!(content.matches(":END:").count(), 3);
}

#[test]
fn add_id_idempotent() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("idem.org");
    fs::write(&file, "* A\n* B\n").unwrap();

    // First run adds IDs.
    org()
        .args(["update", "add-id"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 2 IDs"));

    // Second run is a no-op.
    org()
        .args(["update", "add-id"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("All entries already have IDs"));
}

#[test]
fn add_id_dry_run() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("dry.org");
    fs::write(&file, "* A\n* B\n").unwrap();

    org()
        .args(["update", "add-id", "--dry-run"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Would add 2 IDs"));

    // File should be unchanged.
    let content = fs::read_to_string(&file).unwrap();
    assert_eq!(content, "* A\n* B\n");
}

#[test]
fn add_id_locator_single_entry() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("single.org");
    fs::write(&file, "* A\n* B\n* C\n").unwrap();

    let locator = format!("{}::*/B", file.display());
    org()
        .args(["update", "add-id", &locator])
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 1 ID"));

    let content = fs::read_to_string(&file).unwrap();
    // Only B should have an ID.
    assert_eq!(content.matches(":ID:").count(), 1);
    assert!(content.contains("* A\n* B\n:PROPERTIES:"));
}

#[test]
fn add_id_locator_recursive() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("tree.org");
    fs::write(&file, "* A\n** B\n*** C\n* D\n").unwrap();

    let locator = format!("{}::*/A", file.display());
    org()
        .args(["update", "add-id", "-r", &locator])
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 3 IDs"));

    let content = fs::read_to_string(&file).unwrap();
    // A, B, C should have IDs; D should not.
    assert_eq!(content.matches(":ID:").count(), 3);
    // D should still be a plain heading.
    assert!(content.contains("* D\n") || content.ends_with("* D"));
}

#[test]
fn add_id_custom_format() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("custom.org");
    fs::write(&file, "* Hello World\n").unwrap();

    org()
        .args([
            "update",
            "add-id",
            "--id-format",
            "{file_stem}-{title_slug}-{level}",
        ])
        .arg(file.to_str().unwrap())
        .assert()
        .success();

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains(":ID: custom-hello-world-1\n"));
}

#[test]
fn add_id_existing_drawer() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("drawer.org");
    fs::write(&file, "* Task\n:PROPERTIES:\n:EFFORT: 1:00\n:END:\nBody\n").unwrap();

    org()
        .args(["update", "add-id"])
        .arg(file.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 1 ID"));

    let content = fs::read_to_string(&file).unwrap();
    // Should have one :PROPERTIES: block (not two).
    assert_eq!(content.matches(":PROPERTIES:").count(), 1);
    assert_eq!(content.matches(":END:").count(), 1);
    // :ID: should be inside the existing drawer, before :EFFORT:.
    let id_pos = content.find(":ID:").unwrap();
    let effort_pos = content.find(":EFFORT:").unwrap();
    assert!(id_pos < effort_pos);
}
