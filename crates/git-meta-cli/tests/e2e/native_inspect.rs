use crate::harness::{git_meta_without_git, writable_fixture};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct BlameGroup {
    commit: String,
    author: Option<String>,
    author_time: Option<String>,
    summary: Option<String>,
    previous: Option<String>,
    lines: Vec<Line>,
}

#[derive(Debug, Deserialize)]
struct Line {
    line: u32,
    original_line: u32,
    text: String,
}

fn oracle(dir: &Path, args: &[&str]) -> String {
    let output = gix_testtools::git_command(dir).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "Git oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn assert_blame(dir: &Path, path: &str, revision: Option<&str>) {
    let mut args = vec!["blame", "--line-porcelain"];
    if let Some(revision) = revision {
        args.push(revision);
    }
    args.extend(["--", path]);
    let expected = oracle(dir, &args);
    let empty_path = tempfile::TempDir::new().unwrap();
    let mut command = git_meta_without_git(dir, empty_path.path());
    command.args(["blame", "--porcelain"]);
    if let Some(revision) = revision {
        command.args(["--rev", revision]);
    }
    let output = command
        .arg(path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let groups: Vec<BlameGroup> = serde_json::from_slice(&output).unwrap();
    let mut expected_lines = expected.lines();
    for group in groups {
        for line in &group.lines {
            let header = expected_lines
                .next()
                .unwrap()
                .split_whitespace()
                .collect::<Vec<_>>();
            assert_eq!(header[0], group.commit);
            assert_eq!(header[1].parse::<u32>().unwrap(), line.original_line);
            assert_eq!(header[2].parse::<u32>().unwrap(), line.line);
            let mut author = None;
            let mut author_time = None;
            let mut summary = None;
            let mut previous = None;
            for metadata in expected_lines.by_ref() {
                if let Some(text) = metadata.strip_prefix('\t') {
                    assert_eq!(text, line.text);
                    break;
                }
                if let Some(value) = metadata.strip_prefix("author ") {
                    author = Some(value);
                }
                if let Some(value) = metadata.strip_prefix("author-time ") {
                    author_time = Some(value);
                }
                if let Some(value) = metadata.strip_prefix("summary ") {
                    summary = Some(value);
                }
                if let Some(value) = metadata.strip_prefix("previous ") {
                    previous = Some(value);
                }
            }
            assert_eq!(group.author.as_deref(), author);
            if !group.commit.chars().all(|c| c == '0') {
                assert_eq!(group.author_time.as_deref(), author_time);
            }
            assert_eq!(group.summary.as_deref(), summary);
            assert_eq!(
                group.previous.as_deref(),
                previous,
                "{path} {revision:?} {group:?}"
            );
        }
    }
    assert!(
        expected_lines.next().is_none(),
        "all Git blame lines were rendered"
    );
}

#[test]
fn blame_matches_git_without_git_on_path() {
    let (repo, _) = writable_fixture("native-inspect.sh");
    assert_blame(repo.path(), "dir/renamed.txt", Some("HEAD"));
    assert_blame(repo.path(), "dir/renamed.txt", None);
    assert_blame(&repo.path().join("dir"), "./renamed.txt", None);
    assert_blame(
        repo.path(),
        repo.path().join("dir/renamed.txt").to_str().unwrap(),
        Some("HEAD"),
    );
    assert_blame(repo.path(), "typechange.txt", Some("HEAD"));
    assert_blame(repo.path(), "typechange.txt", None);
    let author_email = oracle(repo.path(), &["show", "-s", "--format=%ae", "HEAD"]);
    std::fs::write(
        repo.path().join(".mailmap"),
        format!(
            "Mapped Author <mapped@example.com> <{}>\n",
            author_email.trim()
        ),
    )
    .unwrap();
    assert_blame(repo.path(), "dir/renamed.txt", Some("HEAD"));
    std::fs::write(repo.path().join(".gitattributes"), "*.txt text\n").unwrap();
    std::fs::write(
        repo.path().join("dir/renamed.txt"),
        "first\r\nSECOND\r\nTHIRD\r\nfourth\r\nuncommitted\r\n",
    )
    .unwrap();
    assert_blame(repo.path(), "dir/renamed.txt", None);
}

#[test]
fn blame_rejects_missing_untracked_and_outside_paths_without_git() {
    let (repo, _) = writable_fixture("native-inspect.sh");
    let empty_path = tempfile::TempDir::new().unwrap();
    std::fs::write(repo.path().join("untracked.txt"), "untracked\n").unwrap();
    for path in ["missing.txt", "untracked.txt", "../outside.txt", "dir"] {
        git_meta_without_git(repo.path(), empty_path.path())
            .args(["blame", path])
            .assert()
            .failure();
    }
    git_meta_without_git(repo.path(), empty_path.path())
        .args(["blame", "--rev", "missing-revision", "dir/renamed.txt"])
        .assert()
        .failure();
    std::fs::write(repo.path().join("dir/renamed.txt"), b"invalid\xff\n").unwrap();
    git_meta_without_git(repo.path(), empty_path.path())
        .args(["blame", "dir/renamed.txt"])
        .assert()
        .failure();
}

fn without_ansi(input: &str) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for c in input.chars() {
        if c == '\x1b' {
            escaped = true;
        } else if escaped {
            if c == 'm' {
                escaped = false;
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn show_matches_diff_tree_without_git_on_path() {
    let (repo, _) = writable_fixture("native-inspect.sh");
    let empty_path = tempfile::TempDir::new().unwrap();
    for quote_path in ["true", "false"] {
        oracle(repo.path(), &["config", "core.quotePath", quote_path]);
        for revision in ["HEAD", "HEAD^", "root", "merge"] {
            let expected = oracle(
                repo.path(),
                &[
                    "diff-tree",
                    "--no-commit-id",
                    "-r",
                    "--name-status",
                    revision,
                ],
            );
            let output = git_meta_without_git(repo.path(), empty_path.path())
                .args(["show", revision])
                .assert()
                .success()
                .get_output()
                .stdout
                .clone();
            let output = without_ansi(std::str::from_utf8(&output).unwrap());
            let actual = output
                .split_once("Files changed:\n")
                .map(|(_, files)| {
                    files
                        .lines()
                        .take_while(|line| line.starts_with("  "))
                        .map(|line| line.trim_start().replacen(' ', "\t", 1) + "\n")
                        .collect::<String>()
                })
                .unwrap_or_default();
            assert_eq!(actual, expected, "{revision}, core.quotePath={quote_path}");
        }
    }
}
