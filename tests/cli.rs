use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::{NamedTempFile, TempDir};

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mmap-dump"));
    command.env("NO_COLOR", "1").env_remove("CLICOLOR_FORCE");
    command
}

fn file_with(contents: &[u8]) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("temporary file should be created");
    file.write_all(contents)
        .expect("temporary file should be written");
    file.flush().expect("temporary file should be flushed");
    file
}

fn run(path: &Path, offset: u64, extra_args: &[&str]) -> Output {
    let mut command = command();
    command.arg(path).arg(offset.to_string()).args(extra_args);
    command.output().expect("mmap-dump should run")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "unexpected failure: stderr={:?}",
        output.stderr
    );
    assert!(output.stderr.is_empty(), "stderr={:?}", output.stderr);
}

fn assert_failure(output: &Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "stdout={:?}", output.stdout);
    assert!(!output.stderr.is_empty());
    assert!(
        !output.stderr.contains(&b'\x1b'),
        "captured diagnostics must not contain terminal escapes"
    );
}

#[test]
fn writes_binary_data_from_an_unaligned_offset() {
    let contents = b"\0\xffheader\nbody\0tail";
    let file = file_with(contents);

    let output = run(file.path(), 3, &[]);

    assert_success(&output);
    assert_eq!(output.stdout, &contents[3..]);
}

#[test]
fn accepts_the_whole_file_options() {
    let contents = b"whole file mapping";
    let file = file_with(contents);

    for option in ["--map-whole-file", "-w"] {
        let output = run(file.path(), 0, &[option]);

        assert_success(&output);
        assert_eq!(output.stdout, contents, "{option}");
    }
}

#[test]
fn returns_empty_output_at_or_beyond_end_of_file() {
    let contents = b"content";
    let file = file_with(contents);

    for offset in [contents.len() as u64, u64::MAX] {
        let output = run(file.path(), offset, &[]);

        assert_success(&output);
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn fails_for_a_missing_file() {
    let directory = TempDir::new().expect("temporary directory should be created");
    let missing = directory.path().join("missing");

    let output = run(&missing, 0, &[]);
    assert_failure(&output);
}

#[test]
fn rejects_non_regular_files() {
    let directory = TempDir::new().expect("temporary directory should be created");

    let output = run(directory.path(), 0, &[]);
    assert_failure(&output);
}

#[test]
fn reports_lock_contention() {
    let file = file_with(b"locked");
    let lock_owner = File::open(file.path()).expect("temporary file should reopen");
    lock_owner
        .try_lock()
        .expect("test should acquire an exclusive lock");

    let output = run(file.path(), 0, &[]);
    assert_failure(&output);
}

#[test]
fn treats_an_early_reader_exit_as_success() {
    let file = NamedTempFile::new().expect("temporary file should be created");
    file.as_file()
        .set_len(16 * 1024 * 1024)
        .expect("temporary file should be extended");

    let mut child = command()
        .arg(file.path())
        .arg("0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mmap-dump should start");
    let mut stdout = child.stdout.take().expect("stdout should be piped");
    let mut first_byte = [0_u8; 1];
    stdout
        .read_exact(&mut first_byte)
        .expect("mmap-dump should write at least one byte");
    drop(stdout);

    let output = child
        .wait_with_output()
        .expect("mmap-dump should finish after the pipe closes");

    assert_success(&output);
}
