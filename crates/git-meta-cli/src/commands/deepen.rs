//! `git meta deepen` — fetch more of a shallow metadata history.
//!
//! A project can ask new clones to fetch only a recent slice of the metadata
//! history (`depth:` in `.git-meta`). That slice is enough to read current
//! metadata, but keys published before it are neither present nor known: the
//! commits carrying them were never fetched, so history indexing never saw
//! them.
//!
//! Deepening fetches further back and indexes what that adds, so those older
//! keys become fetchable in turn.

use anyhow::{bail, Result};

use crate::context::CommandContext;
use crate::style::Style;

/// Fetch `depth` more metadata commits, or the whole remaining history.
pub(crate) fn run(depth: Option<u32>) -> Result<()> {
    let ctx = CommandContext::open(None)?;
    let repo = ctx.session.repo();
    let s_err = Style::detect_stderr();
    let ns = ctx.session.namespace();

    let remote = git_meta_lib::git_utils::resolve_meta_remote(repo, None)?;
    let tracking_ref = format!("refs/{ns}/remotes/main");
    if repo.find_reference(&tracking_ref).is_err() {
        bail!("no metadata fetched yet. Run `git meta pull` or `git meta setup` first.");
    }

    // The commits at the edge of what we have. After the fetch they gain
    // parents, and walking from them covers exactly the newly reachable
    // history rather than everything again.
    let frontier = shallow_boundary(repo);
    if frontier.is_empty() {
        eprintln!("The metadata history is already complete — nothing to deepen.");
        return Ok(());
    }

    let before = count_commits(&ctx, &tracking_ref);
    let refspec = format!("+refs/{ns}/main:{tracking_ref}");
    match depth {
        Some(depth) => eprint!("{} by {depth} commits...", s_err.step("Deepening")),
        None => eprint!("{} to the full history...", s_err.step("Deepening")),
    }
    let started = std::time::Instant::now();
    let shallow = depth.map_or_else(
        gix::remote::fetch::Shallow::undo,
        gix::remote::fetch::Shallow::Deepen,
    );
    git_meta_lib::git_utils::fetch(repo, &remote, [&refspec], shallow)?;
    let after = count_commits(&ctx, &tracking_ref);
    eprintln!(
        " {}",
        s_err.ok(&format!(
            "{} commits added in {:.1}s ({after} total).",
            after.saturating_sub(before),
            started.elapsed().as_secs_f64()
        ))
    );

    if after == before {
        eprintln!("Nothing further to fetch.");
        return Ok(());
    }

    eprint!("{} the commits that added...", s_err.step("Indexing"));
    let started = std::time::Instant::now();
    let mut indexed = 0;
    for commit in frontier {
        indexed +=
            git_meta_lib::sync::insert_promisor_entries(repo, ctx.session.store(), commit, None)?;
    }
    eprintln!(
        " {}",
        s_err.ok(&format!(
            "{indexed} keys indexed in {:.1}s.",
            started.elapsed().as_secs_f64()
        ))
    );

    if shallow_boundary(repo).is_empty() {
        eprintln!("{}", s_err.ok("The metadata history is now complete."));
    } else {
        eprintln!(
            "{}",
            s_err.dim("Still shallow — run `git meta deepen` again to reach further back.")
        );
    }
    Ok(())
}

/// The commits Git records as the edge of a shallow history.
///
/// An empty result means the history is complete.
fn shallow_boundary(repo: &gix::Repository) -> Vec<gix::ObjectId> {
    repo.shallow_commits()
        .ok()
        .flatten()
        .map(|commits| commits.iter().copied().collect())
        .unwrap_or_default()
}

fn count_commits(ctx: &CommandContext, tracking_ref: &str) -> usize {
    git_meta_lib::git_utils::count_commits(ctx.session.repo(), tracking_ref).unwrap_or(0)
}
