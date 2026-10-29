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

#[cfg(unix)]
#[test]
fn sender_hook_rejection_leaves_remote_unchanged_without_git_on_path() {
    use std::os::unix::fs::PermissionsExt;
    let (client, _) = setup_repo();
    let remote = setup_bare_with_history();
    let empty_path = tempfile::TempDir::new().unwrap();
    harness::git_meta_without_git(client.path(), empty_path.path())
        .args(["remote", "add", remote.path().to_str().unwrap()])
        .assert()
        .success();
    let remote_repo = harness::open_repo(remote.path());
    let before = harness::ref_to_commit_oid(&remote_repo, "refs/meta/main");
    harness::git_meta_without_git(client.path(), empty_path.path())
        .args(["set", "project", "native", "rejected"])
        .assert()
        .success();
    let hook = client.path().join(".git/hooks/pre-push");
    std::fs::write(&hook, "#!/bin/sh\nread local_ref local_oid remote_ref remote_oid\n[ \"$local_ref\" = refs/meta/local/main ] || exit 2\n[ \"$remote_ref\" = refs/meta/main ] || exit 3\nprintf 'native sender hook rejected\\n' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    harness::git_meta_without_git(client.path(), empty_path.path())
        .args(["push"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("native sender hook rejected"));
    assert_eq!(
        harness::ref_to_commit_oid(&remote_repo, "refs/meta/main"),
        before,
        "a sender hook must reject before publishing metadata"
    );
}

#[test]
fn readme_publication_does_not_replace_an_existing_branch_without_git_on_path() {
    let (client, _) = setup_repo();
    let remote = tempfile::TempDir::new().unwrap();
    gix::init_bare(remote.path()).unwrap();
    let empty_path = tempfile::TempDir::new().unwrap();
    harness::git_meta_without_git(client.path(), empty_path.path())
        .args(["remote", "add", remote.path().to_str().unwrap(), "--init"])
        .assert()
        .success();
    harness::git_meta_without_git(client.path(), empty_path.path())
        .args(["push", "--readme"])
        .assert()
        .success();
    let remote_repo = harness::open_repo(remote.path());
    let original = harness::ref_to_commit_oid(&remote_repo, "refs/heads/main");
    let mut commit = remote_repo
        .find_commit(original)
        .unwrap()
        .decode()
        .unwrap()
        .to_owned()
        .unwrap();
    commit.message = "An existing project branch".into();
    let before = remote_repo.write_object(&commit).unwrap().detach();
    remote_repo
        .reference(
            "refs/heads/main",
            before,
            gix::refs::transaction::PreviousValue::MustExistAndMatch(original.into()),
            "existing branch",
        )
        .unwrap();
    harness::git_meta_without_git(client.path(), empty_path.path())
        .args(["push", "--readme"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
    assert_eq!(
        harness::ref_to_commit_oid(&remote_repo, "refs/heads/main"),
        before
    );
}

#[cfg(unix)]
#[test]
fn every_push_url_is_attempted_and_hook_failures_are_reported_without_git_on_path() {
    use std::os::unix::fs::PermissionsExt;
    for reject_first in [false, true] {
        let (client, _) = setup_repo();
        let first = tempfile::TempDir::new().unwrap();
        let second = tempfile::TempDir::new().unwrap();
        for remote in [&first, &second] {
            gix::init_bare(remote.path()).unwrap();
        }
        let empty_path = tempfile::TempDir::new().unwrap();
        harness::git_meta_without_git(client.path(), empty_path.path())
            .args(["remote", "add", first.path().to_str().unwrap(), "--init"])
            .assert()
            .success();
        let before =
            harness::ref_to_commit_oid(&harness::open_repo(first.path()), "refs/meta/main");
        harness::git_meta_without_git(client.path(), empty_path.path())
            .args(["set", "project", "mirrors", "both"])
            .assert()
            .success();
        if reject_first {
            let hook = client.path().join(".git/hooks/pre-push");
            std::fs::write(
                &hook,
                format!(
                    "#!/bin/sh\n[ \"$1\" = meta ] || exit 2\n[ \"$2\" != '{}' ]\n",
                    first.path().display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut command = harness::git_meta_without_git(client.path(), empty_path.path());
        command
            .env("GIT_CONFIG_COUNT", "5")
            .env("GIT_CONFIG_KEY_3", "remote.meta.pushurl")
            .env("GIT_CONFIG_VALUE_3", first.path())
            .env("GIT_CONFIG_KEY_4", "remote.meta.pushurl")
            .env("GIT_CONFIG_VALUE_4", second.path())
            .arg("push");
        if reject_first {
            command
                .assert()
                .failure()
                .stderr(predicate::str::contains("pre-push hook rejected"));
        } else {
            command.assert().success();
        }
        let published =
            harness::ref_to_commit_oid(&harness::open_repo(second.path()), "refs/meta/main");
        assert_ne!(
            published, before,
            "later destinations must be attempted even if an earlier hook fails"
        );
        assert_eq!(
            harness::ref_to_commit_oid(&harness::open_repo(first.path()), "refs/meta/main"),
            if reject_first { before } else { published },
            "each sender hook receives its own destination URL"
        );
    }
}
