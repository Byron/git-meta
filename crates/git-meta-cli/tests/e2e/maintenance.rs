use crate::harness::{self, open_repo, setup_repo};

#[test]
fn native_maintenance_retains_objects_and_ignores_lock_failure_without_git_on_path() {
    for locked in [false, true] {
        let (dir, _) = setup_repo();
        let repo = open_repo(dir.path());
        let object_id = repo
            .write_blob(b"hydrated blob outside the available graph")
            .unwrap()
            .detach();
        if locked {
            std::fs::write(repo.common_dir().join("gc.pid.lock"), b"another collector").unwrap();
        }
        let empty_path = tempfile::TempDir::new().unwrap();
        harness::git_meta_without_git(dir.path(), empty_path.path())
            .args(["set", "project", "maintenance", "native"])
            .assert()
            .success();
        harness::git_meta_without_git(dir.path(), empty_path.path())
            .env("GIT_CONFIG_COUNT", "4")
            .env("GIT_CONFIG_KEY_3", "gc.auto")
            .env("GIT_CONFIG_VALUE_3", "1")
            .args(["serialize"])
            .assert()
            .success();
        let repo = open_repo(dir.path());
        assert!(
            repo.try_find_object(object_id).unwrap().is_some(),
            "maintenance never expires hydrated objects"
        );
        let loose = gix::odb::loose::Store::at(repo.objects.store_ref().path(), repo.object_hash());
        assert_eq!(
            loose.iter().count() > 0,
            locked,
            "only successful maintenance removes redundant loose objects"
        );
    }
}
