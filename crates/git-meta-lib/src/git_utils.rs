//! Git repository helpers for resolving objects and manipulating metadata refs.

use std::path::PathBuf;

use crate::error::{Error, Result};
use gix::error::{ErrorExt, ResultExt};

// 4000 SHA-1 OIDs plus newlines is roughly 164 KiB, safely below the
// smart-HTTP request size that made GitHub reject one-shot tip hydration.
const FETCH_OID_BATCH_SIZE: usize = 4000;

/// Check if a tree entry name looks like a list entry (timestamp-hash format).
pub(crate) fn is_list_entry_name(name: &str) -> bool {
    // Format: {ms_epoch}-{first_5_sha256}
    if let Some((ts_part, hash_part)) = name.split_once('-') {
        ts_part.chars().all(|c| c.is_ascii_digit())
            && !ts_part.is_empty()
            && hash_part.len() == 5
            && hash_part.chars().all(|c| c.is_ascii_hexdigit())
    } else {
        false
    }
}

/// Count reachable metadata commits, honoring shallow boundaries.
///
/// # Errors
/// Returns an error if the revision cannot be resolved or history cannot be read.
pub fn count_commits(repo: &gix::Repository, revision: &str) -> Result<usize> {
    let tip = repo
        .rev_parse_single(revision)?
        .object()?
        .peel_to_commit()?
        .id;
    let mut count = 0;
    for commit in repo.rev_walk([tip]).all()? {
        commit?;
        count += 1;
    }
    Ok(count)
}

/// Fetch explicit metadata refs or object IDs without invoking Git.
///
/// No tags, `FETCH_HEAD`, submodules, or maintenance are requested. Metadata remotes
/// use their configured blobless filter; callers may override shallow history policy.
///
/// # Errors
/// Returns an error if configuration, transfer, or reference updates fail.
pub fn fetch(
    repo: &gix::Repository,
    remote_name: &str,
    refspecs: impl IntoIterator<Item = impl AsRef<str>>,
    shallow: gix::remote::fetch::Shallow,
) -> Result<()> {
    let mut remote = repo
        .find_remote(remote_name)?
        .with_fetch_tags(gix::remote::fetch::Tags::None);
    remote.replace_refspecs(
        refspecs
            .into_iter()
            .map(|spec| gix::bstr::BString::from(spec.as_ref())),
        gix::remote::Direction::Fetch,
    )?;
    let filter = repo
        .config_snapshot()
        .string(&format!("remote.{remote_name}.partialclonefilter"));
    let filter = match filter.as_deref() {
        None => None,
        Some(value) if value.as_slice() == b"blob:none" => {
            Some(gix::remote::fetch::Filter::BlobNone)
        }
        Some(_) => {
            return Err(Error::Other(
                "unsupported metadata partial-clone filter".into(),
            ))
        }
    };
    let outcome = remote.fetch(
        gix::remote::fetch::Options {
            shallow,
            filter,
            ..remote.fetch_options()?
        },
        gix::progress::Discard,
        &std::sync::atomic::AtomicBool::default(),
    )?;
    let updates = match &outcome.status {
        gix::remote::fetch::Status::Change { update_refs, .. }
        | gix::remote::fetch::Status::NoPackReceived { update_refs, .. } => update_refs,
    };
    if updates.updates.iter().any(|update| {
        matches!(
            update.mode,
            gix::remote::fetch::refs::update::Mode::RejectedSourceObjectNotFound { .. }
                | gix::remote::fetch::refs::update::Mode::RejectedTagUpdate
                | gix::remote::fetch::refs::update::Mode::RejectedNonFastForward
                | gix::remote::fetch::refs::update::Mode::RejectedToReplaceWithUnborn
                | gix::remote::fetch::refs::update::Mode::RejectedCurrentlyCheckedOut { .. }
        )
    }) {
        return Err(Error::Other(
            "metadata fetch reference update was rejected".into(),
        ));
    }
    Ok(())
}

/// Push one explicit reference update, retaining typed rejection reasons.
///
/// # Errors
/// Returns transport, configuration, and hook errors. Reference rejections are
/// returned as a push status so callers can distinguish merge retries from failures.
pub fn push(repo: &gix::Repository, remote_name: &str, refspec: &str) -> Result<gix::push::Status> {
    let remote = repo.find_remote(remote_name)?;
    let spec = gix::refspec::parse(
        refspec.as_bytes().into(),
        gix::refspec::parse::Operation::Push,
    )?;
    let update = gix::push::Update::from_refspec(repo, spec)?;
    let source_name = spec
        .source()
        .map(|source| source.to_string())
        .unwrap_or_else(|| "(delete)".into());
    let hook = |updates: &[gix::push::UpdateOutcome]| -> gix::Result<()> {
        let mut input = String::new();
        for outcome in updates
            .iter()
            .filter(|outcome| matches!(outcome.status, gix::push::Status::WouldUpdate { .. }))
        {
            let null = repo.object_hash().null();
            let new_id = outcome.update.new_id.unwrap_or(null);
            let old_id = outcome.old_id.unwrap_or(null);
            let local_ref = if outcome.update.destination == update.destination {
                source_name.clone()
            } else {
                outcome.update.destination.as_bstr().to_string()
            };
            use std::fmt::Write;
            writeln!(
                input,
                "{local_ref} {new_id} {} {old_id}",
                outcome.update.destination.as_bstr()
            )
            .or_error()?;
        }
        let url = remote
            .url(gix::remote::Direction::Push)
            .ok_or_else(|| gix::error::message("push URL is missing").raise())?;
        let url = url.to_bstring();
        let url = gix::path::from_bstr(&url)?;
        let args = [std::ffi::OsStr::new(remote_name), url.as_os_str()];
        for output in repo.run_hook_event_serially(
            "pre-push",
            gix::hook::Options {
                arguments: &args,
                stdin: Some(input.as_bytes()),
                ..Default::default()
            },
        )? {
            use std::io::Write;
            std::io::stderr().write_all(&output.stderr).or_error()?;
            std::io::stdout().write_all(&output.stdout).or_error()?;
            if !output.status.success() {
                return Err(gix::error::message("pre-push hook rejected metadata push").raise());
            }
        }
        Ok(())
    };
    let outcome = remote.push(
        [update.clone()],
        gix::push::Options {
            pre_push_hook: Some(&hook),
            ..Default::default()
        },
        gix::progress::Discard,
        |_, bytes| {
            use std::io::Write;
            std::io::stderr().write_all(bytes)
        },
        &std::sync::atomic::AtomicBool::default(),
    )?;
    remote.update_tracking_refs(&outcome)?;
    outcome
        .updates
        .into_iter()
        .next()
        .map(|update| update.status)
        .ok_or_else(|| Error::Other("metadata push returned no update status".into()))
}

/// List all git remotes that have `meta = true` in their config.
///
/// # Parameters
///
/// - `repo`: the Git repository to query
///
/// # Returns
///
/// A vec of `(name, url)` pairs for each remote with `remote.<name>.meta = true`.
///
/// # Errors
///
/// Returns an error if reading the git config fails.
pub fn list_meta_remotes(repo: &gix::Repository) -> Result<Vec<(String, String)>> {
    let config = repo.config_snapshot();
    let remote_names = repo.remote_names();
    let mut remotes = Vec::new();

    for name in &remote_names {
        let meta_key = format!("remote.{name}.meta");
        if config.boolean(&meta_key) == Some(true) {
            let url_key = format!("remote.{name}.url");
            if let Some(url) = config.string(&url_key) {
                remotes.push((name.to_string(), url.to_string()));
            }
        }
    }

    Ok(remotes)
}

/// Hydrate tip tree blobs for a blobless-fetched ref.
///
/// This fetches all blob objects referenced by the tip tree so gix can read them.
///
/// # Parameters
///
/// - `repo`: the Git repository to operate on
/// - `remote_name`: the remote to fetch blobs from
/// - `ref_name`: the ref whose tree blobs should be fetched
///
/// # Errors
///
/// Returns an error if tree traversal or the object fetch fails.
pub fn hydrate_tip_blobs(repo: &gix::Repository, remote_name: &str, ref_name: &str) -> Result<()> {
    hydrate_tip_blobs_counted(repo, remote_name, ref_name)?;
    Ok(())
}

/// Like [`hydrate_tip_blobs`] but returns the number of blobs fetched.
///
/// # Parameters
///
/// - `repo`: the Git repository to operate on
/// - `remote_name`: the remote to fetch blobs from
/// - `ref_name`: the ref whose tree blobs should be fetched
///
/// # Returns
///
/// The number of blob OIDs discovered in the tree.
///
/// # Errors
///
/// Returns an error if tree traversal or the object fetch fails.
pub fn hydrate_tip_blobs_counted(
    repo: &gix::Repository,
    remote_name: &str,
    ref_name: &str,
) -> Result<usize> {
    let tree = repo.rev_parse_single(ref_name)?.object()?.peel_to_tree()?;
    let entries = tree.traverse().breadthfirst.files()?;
    let oids: Vec<_> = entries
        .into_iter()
        .filter(|entry| entry.mode.is_blob())
        .map(|entry| entry.oid)
        .collect();
    let count = oids.len();
    fetch_blob_oids(repo, remote_name, &oids)?;
    Ok(count)
}

/// Look up a blob OID in a git tree by following a slash-separated path.
///
/// Returns `None` if any path segment is missing. Trees are local (fetched even
/// in blobless clones), so this works without network access.
///
/// # Parameters
///
/// - `repo`: the Git repository containing the tree
/// - `tree_id`: the root tree object ID to start from
/// - `path`: slash-separated path to the blob (e.g. `"a/b/file.txt"`)
///
/// # Returns
///
/// `Some(ObjectId)` of the blob at the path, or `None` if not found.
///
/// # Errors
///
/// Returns an error if reading tree objects from the repository fails.
#[cfg_attr(not(feature = "internal"), allow(dead_code))]
pub fn find_blob_oid_in_tree(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    path: &str,
) -> Result<Option<gix::ObjectId>> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Ok(None);
    }

    let mut current_tree_id = tree_id;

    for (i, segment) in segments.iter().enumerate() {
        let tree = repo
            .find_tree(current_tree_id)
            .map_err(|e| Error::Other(format!("{e}")))?;
        let Some(entry) = tree.find_entry(*segment) else {
            return Ok(None);
        };

        let entry_id = entry.object_id();
        let is_tree = entry.mode().is_tree();

        if i == segments.len() - 1 {
            return Ok(Some(entry_id));
        }

        if !is_tree {
            return Ok(None);
        }
        current_tree_id = entry_id;
    }

    Ok(None)
}

/// Fetch specific blob OIDs from a remote.
///
/// Similar to [`hydrate_tip_blobs`] but takes an explicit list of OIDs
/// instead of discovering them by traversing the tip tree.
///
/// # Parameters
///
/// - `repo`: the Git repository to operate on
/// - `remote_name`: the remote to fetch blobs from
/// - `oids`: the blob OIDs to fetch
///
/// # Errors
///
/// Returns an error if fetching the requested objects fails.
#[cfg_attr(not(feature = "internal"), allow(dead_code))]
pub fn fetch_blob_oids(
    repo: &gix::Repository,
    remote_name: &str,
    oids: &[gix::ObjectId],
) -> Result<()> {
    if oids.is_empty() {
        return Ok(());
    }

    fetch_oid_batches(repo, remote_name, oids)
}

fn fetch_oid_batches(
    repo: &gix::Repository,
    remote_name: &str,
    oids: &[gix::ObjectId],
) -> Result<()> {
    for batch in oid_batches(oids) {
        let missing: Vec<_> = batch
            .iter()
            .filter(|oid| !repo.has_object(**oid))
            .map(ToString::to_string)
            .collect();
        if !missing.is_empty() {
            fetch(
                repo,
                remote_name,
                &missing,
                gix::remote::fetch::Shallow::NoChange,
            )?;
        }
    }
    Ok(())
}

fn oid_batches<T>(oids: &[T]) -> impl Iterator<Item = &[T]> {
    oids.chunks(FETCH_OID_BATCH_SIZE)
}

/// Resolve a meta remote by name, or pick the first one if no name given.
///
/// # Parameters
///
/// - `repo`: the Git repository to query
/// - `remote`: optional remote name; if `None`, returns the first meta remote
///
/// # Returns
///
/// The name of the resolved meta remote.
///
/// # Errors
///
/// Returns [`Error::NoRemotes`] if no meta remotes are configured, or
/// [`Error::RemoteNotFound`] if the specified name is not a meta remote.
pub fn resolve_meta_remote(repo: &gix::Repository, remote: Option<&str>) -> Result<String> {
    let meta_remotes = list_meta_remotes(repo)?;

    if meta_remotes.is_empty() {
        return Err(Error::NoRemotes);
    }

    if let Some(name) = remote {
        if meta_remotes.iter().any(|(n, _)| n == name) {
            Ok(name.to_string())
        } else {
            Err(Error::RemoteNotFound(name.to_string()))
        }
    } else {
        let config = repo.config_snapshot();
        meta_remotes
            .iter()
            .find(|(name, _)| config.boolean(&format!("remote.{name}.metaside")) != Some(true))
            .or_else(|| meta_remotes.first())
            .map(|(name, _)| name.clone())
            .ok_or(Error::NoRemotes)
    }
}

fn gix_config_string(repo: &gix::Repository, key: &str, default: &str) -> String {
    let config = repo.config_snapshot();
    config
        .string(key)
        .map_or_else(|| default.to_string(), |s| s.to_string())
}

/// Discover the Git repository from the current directory.
///
/// # Errors
///
/// Returns [`Error::NotARepository`] if no git repository is found.
pub(crate) fn discover_repo() -> Result<gix::Repository> {
    let repo = gix::discover(".").map_err(|_| Error::NotARepository)?;
    Ok(repo)
}

/// Get the path to the local SQLite metadata database.
///
/// # Parameters
///
/// - `repo`: the Git repository
///
/// # Returns
///
/// The path to `git-meta.sqlite` inside the git directory.
pub(crate) fn db_path(repo: &gix::Repository) -> Result<PathBuf> {
    Ok(repo.git_dir().join("git-meta.sqlite"))
}

/// Get the user's email from Git config.
///
/// # Parameters
///
/// - `repo`: the Git repository
///
/// # Returns
///
/// The configured `user.email`, or `"unknown"` if not set.
pub(crate) fn get_email(repo: &gix::Repository) -> Result<String> {
    Ok(gix_config_string(repo, "user.email", "unknown"))
}

/// Get the user's name from Git config.
///
/// # Parameters
///
/// - `repo`: the Git repository
///
/// # Returns
///
/// The configured `user.name`, or `"unknown"` if not set.
pub(crate) fn get_name(repo: &gix::Repository) -> Result<String> {
    Ok(gix_config_string(repo, "user.name", "unknown"))
}

/// Get the meta namespace from Git config (defaults to "meta").
///
/// # Parameters
///
/// - `repo`: the Git repository
///
/// # Returns
///
/// The configured `meta.namespace`, or `"meta"` if not set.
pub(crate) fn get_namespace(repo: &gix::Repository) -> Result<String> {
    Ok(gix_config_string(repo, "meta.namespace", "meta"))
}

/// Expand a partial commit SHA to the full 40-char hex string.
///
/// # Parameters
///
/// - `repo`: the Git repository
/// - `partial`: a partial (or full) commit SHA or ref name
///
/// # Returns
///
/// The full 40-character hex SHA of the commit.
///
/// # Errors
///
/// Returns [`Error::ResolveError`] if the partial SHA cannot be resolved,
/// or [`Error::Other`] if the resolved object is not a commit.
pub(crate) fn resolve_commit_sha(repo: &gix::Repository, partial: &str) -> Result<String> {
    let obj = repo
        .rev_parse_single(partial.as_bytes())
        .map_err(|_| Error::ResolveError(partial.to_string()))?;
    let id = obj.detach();
    // Verify it's a commit by peeling
    let object = repo
        .find_object(id)
        .map_err(|e| Error::Other(format!("{e}")))?;
    if object.kind != gix::object::Kind::Commit {
        // Try peeling tags etc.
        let peeled = object
            .peel_to_kind(gix::object::Kind::Commit)
            .map_err(|e| Error::Other(format!("{e}")))?;
        Ok(peeled.id.to_string())
    } else {
        Ok(id.to_string())
    }
}

/// Loose-object count above which maintenance is worth doing, when the
/// repository does not set `gc.auto`. The same default Git uses.
const DEFAULT_GC_AUTO: i64 = 6_700;

/// Pack the objects serialization has written, retaining every local object.
///
/// Metadata-only repositories have no other automatic maintenance. Compacting
/// loose objects keeps their storage and lookup costs bounded. Retaining all
/// objects protects concurrent writers and hydrated blobs whose parent trees
/// have not been fetched into a partial store. The native repacker preserves
/// promisor boundaries and object ages for later garbage collection.
///
/// Maintenance is best attempted after normal work to avoid adding latency to
/// individual writes. Failure never invalidates work that already succeeded,
/// so errors are ignored.
pub fn maintain_object_store(repo: &gix::Repository) {
    if has_many_loose_objects(repo) {
        let _ = repo.repack_objects(&std::sync::atomic::AtomicBool::default());
    }
}

/// Count only enough local loose objects to decide whether packing is useful.
/// `gc.auto <= 0` disables automatic maintenance, as it does in Git.
fn has_many_loose_objects(repo: &gix::Repository) -> bool {
    let threshold = repo
        .config_snapshot()
        .integer("gc.auto")
        .unwrap_or(DEFAULT_GC_AUTO);
    let Ok(threshold) = usize::try_from(threshold) else {
        return false;
    };
    if threshold == 0 {
        return false;
    }
    gix::odb::loose::Store::at(repo.objects.store_ref().path(), repo.object_hash())
        .iter()
        .filter_map(std::result::Result::ok)
        .nth(threshold)
        .is_some()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_is_list_entry_name() {
        assert!(is_list_entry_name("1771232450203-23c0f"));
        assert!(is_list_entry_name("1771232450204-0d5f2"));
        assert!(!is_list_entry_name("model"));
        assert!(!is_list_entry_name("agent"));
        assert!(!is_list_entry_name("123-toolong"));
        assert!(!is_list_entry_name("123-abc")); // 3 chars, not 5
        assert!(!is_list_entry_name("-23c0f")); // empty timestamp
    }

    #[test]
    fn oid_batches_splits_large_oid_lists() {
        assert_eq!(FETCH_OID_BATCH_SIZE, 4000);

        let oids = vec!["oid"; 4001];
        let sizes: Vec<usize> = oid_batches(&oids).map(<[_]>::len).collect();

        assert_eq!(sizes, vec![4000, 1]);
    }
}
