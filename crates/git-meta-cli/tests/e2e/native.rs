use crate::harness::{self, setup_bare_with_history, setup_repo};
use predicates::prelude::*;

#[test]
fn synchronize_and_hydrate_without_git_on_path() {
    let (a, _) = setup_repo();
    let (b, _) = setup_repo();
    let remote = setup_bare_with_history();
    let empty_path = tempfile::TempDir::new().unwrap();
    for repo in [&a, &b] {
        harness::git_meta_without_git(repo.path(), empty_path.path())
            .args(["remote", "add", remote.path().to_str().unwrap()])
            .assert()
            .success();
        harness::git_meta_without_git(repo.path(), empty_path.path())
            .args(["index-history"])
            .assert()
            .success();
        harness::git_meta_without_git(repo.path(), empty_path.path())
            .args(["get", "project", "old_key"])
            .assert()
            .success()
            .stdout(predicate::str::contains("old_value"));
    }
    harness::git_meta_without_git(a.path(), empty_path.path())
        .args(["set", "project", "native", "roundtrip"])
        .assert()
        .success();
    harness::git_meta_without_git(a.path(), empty_path.path())
        .args(["push"])
        .assert()
        .success();
    harness::git_meta_without_git(b.path(), empty_path.path())
        .args(["pull"])
        .assert()
        .success();
    harness::git_meta_without_git(b.path(), empty_path.path())
        .args(["get", "project", "native"])
        .assert()
        .success()
        .stdout(predicate::str::contains("roundtrip"));
    harness::git_meta_without_git(b.path(), empty_path.path())
        .args(["remote", "remove", "meta"])
        .assert()
        .success();
}
