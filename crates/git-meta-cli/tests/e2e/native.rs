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

#[test]
fn tip_hydration_includes_symlink_blobs() {
    let source_dir = tempfile::TempDir::new().unwrap();
    let source = gix::init_bare(source_dir.path()).unwrap();
    let blob_id = source.write_blob(b"symlink target").unwrap().detach();
    let tree = gix::objs::Tree {
        entries: vec![gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Link.into(),
            filename: "link".into(),
            oid: blob_id,
        }],
    };
    let tree_id = source.write_object(&tree).unwrap().detach();
    source
        .reference(
            "refs/meta/main",
            tree_id,
            gix::refs::transaction::PreviousValue::Any,
            "fixture",
        )
        .unwrap();
    let client_dir = tempfile::TempDir::new().unwrap();
    gix::init_bare(client_dir.path()).unwrap();
    let client = gix::open_opts(
        client_dir.path(),
        gix::open::Options::isolated().config_overrides([
            format!("remote.meta.url={}", source_dir.path().display()),
            "remote.meta.partialclonefilter=blob:none".into(),
        ]),
    )
    .unwrap();
    git_meta_lib::git_utils::fetch(
        &client,
        "meta",
        ["refs/meta/main:refs/meta/remotes/main"],
        gix::remote::fetch::Shallow::NoChange,
    )
    .unwrap();
    assert!(
        !client.has_object(blob_id),
        "blobless fetch omits the symlink contents"
    );
    let count = git_meta_lib::git_utils::hydrate_tip_blobs_counted(
        &client,
        "meta",
        "refs/meta/remotes/main",
    )
    .unwrap();
    assert_eq!(
        count, 1,
        "a symlink stores blob content that must be hydrated"
    );
    assert_eq!(client.find_blob(blob_id).unwrap().data, b"symlink target");
}

#[test]
fn pull_accepts_rewritten_metadata_history_without_git_on_path() {
    for readd_remote in [false, true] {
        let (client, _) = setup_repo();
        let remote = setup_bare_with_history();
        let remote_repo = harness::open_repo(remote.path());
        let initial_id = harness::ref_to_commit_oid(&remote_repo, "refs/meta/main");
        let mut initial = remote_repo
            .find_commit(initial_id)
            .unwrap()
            .decode()
            .unwrap()
            .to_owned()
            .unwrap();
        let mut base = remote_repo
            .find_commit(initial.parents[0])
            .unwrap()
            .decode()
            .unwrap()
            .to_owned()
            .unwrap();
        let older_blob = remote_repo
            .write_blob(b"older value outside the merge base")
            .unwrap()
            .detach();
        let mut older_tree = remote_repo.empty_tree().edit().unwrap();
        older_tree
            .upsert(
                "project/older/__value",
                gix::objs::tree::EntryKind::Blob,
                older_blob,
            )
            .unwrap();
        let mut older = base.clone();
        older.tree = older_tree.write().unwrap().detach();
        older.message = "older history".into();
        base.parents = vec![remote_repo.write_object(&older).unwrap().detach()].into();
        initial.parents = vec![remote_repo.write_object(&base).unwrap().detach()].into();
        let initial_id = remote_repo.write_object(&initial).unwrap().detach();
        remote_repo
            .reference(
                "refs/meta/main",
                initial_id,
                gix::refs::transaction::PreviousValue::Any,
                "extend fixture history",
            )
            .unwrap();
        let empty_path = tempfile::TempDir::new().unwrap();
        harness::git_meta_without_git(client.path(), empty_path.path())
            .args(["remote", "add", remote.path().to_str().unwrap()])
            .assert()
            .success();
        let original = harness::ref_to_commit_oid(&remote_repo, "refs/meta/main");
        let mut rewritten = remote_repo
            .find_commit(original)
            .unwrap()
            .decode()
            .unwrap()
            .to_owned()
            .unwrap();
        rewritten.message = "rewritten metadata tip".into();
        let rewritten_id = remote_repo.write_object(&rewritten).unwrap().detach();
        remote_repo
            .reference(
                "refs/meta/main",
                rewritten_id,
                gix::refs::transaction::PreviousValue::MustExistAndMatch(original.into()),
                "rewrite remote",
            )
            .unwrap();
        let client_repo = harness::open_repo(client.path());
        let base_blob = remote_repo
            .find_tree(base.tree)
            .unwrap()
            .lookup_entry_by_path("project/old_key/__value")
            .unwrap()
            .unwrap()
            .object_id();
        assert!(
            !client_repo.has_object(base_blob),
            "the future merge base starts blobless"
        );
        if readd_remote {
            harness::git_meta_without_git(client.path(), empty_path.path())
                .args(["remote", "remove", "meta"])
                .assert()
                .success();
            client_repo
                .reference(
                    "refs/meta/local/main",
                    original,
                    gix::refs::transaction::PreviousValue::Any,
                    "preserve existing local metadata before setup",
                )
                .unwrap();
            harness::git_meta_without_git(client.path(), empty_path.path())
                .args(["remote", "add", remote.path().to_str().unwrap()])
                .assert()
                .success();
        } else {
            harness::git_meta_without_git(client.path(), empty_path.path())
                .arg("pull")
                .assert()
                .success();
        }
        assert_eq!(
            harness::ref_to_commit_oid(
                &harness::open_repo(client.path()),
                "refs/meta/remotes/main"
            ),
            rewritten_id,
            "metadata refs outside heads/tags accept rewritten history like Git fetch"
        );
        assert!(
            client_repo.has_object(base_blob),
            "three-way materialization hydrates its common ancestor"
        );
        assert!(
            !client_repo.has_object(older_blob),
            "older history outside the merge base remains promised"
        );
        harness::git_meta_without_git(client.path(), empty_path.path())
            .args(["get", "project", "testing"])
            .assert()
            .success()
            .stdout(predicate::str::contains("hello"));
    }
}
