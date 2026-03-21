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
