use crate::error::Result;
use crate::hlc::Hlc;
use crate::types::{ConflictFile, ResolvedFile};

/// Resolve conflicts using Last-Writer-Wins via the unified `lww_pick`.
/// Higher HLC wins (numeric `Hlc::Ord`). When an HLC is missing on either side
/// or the two are exactly equal, the higher content key wins — a role-independent
/// fallback (PRD 00166), replacing the earlier "ours" default.
pub fn resolve_lww(conflicts: Vec<ConflictFile>) -> Result<Vec<ResolvedFile>> {
    let mut resolved = Vec::new();
    for conflict in conflicts {
        let content = pick_lww_winner(&conflict);
        resolved.push(ResolvedFile {
            path: conflict.path,
            content,
            fm_crdt_bytes: None,
        });
    }
    Ok(resolved)
}

/// Which side of a conflict wins an LWW decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LwwSide {
    Ours,
    Theirs,
}

/// The one missing/tie HLC rule, applied at every LWW decision site.
///
/// Higher HLC wins. When an HLC is absent on either side OR the two HLCs are
/// equal, the higher content key wins — a role-independent, content-deterministic
/// fallback that converges under an ours/theirs swap.
pub(crate) fn lww_pick(
    ours_hlc: Option<&Hlc>,
    theirs_hlc: Option<&Hlc>,
    ours_key: &str,
    theirs_key: &str,
) -> LwwSide {
    match (ours_hlc, theirs_hlc) {
        (Some(o), Some(t)) if o != t => {
            if t > o {
                LwwSide::Theirs
            } else {
                LwwSide::Ours
            }
        }
        _ => {
            if theirs_key > ours_key {
                LwwSide::Theirs
            } else {
                LwwSide::Ours
            }
        }
    }
}

/// Pick the winner based on HLC comparison.
fn pick_lww_winner(conflict: &ConflictFile) -> String {
    match lww_pick(
        conflict.ours_hlc.as_ref(),
        conflict.theirs_hlc.as_ref(),
        &conflict.ours,
        &conflict.theirs,
    ) {
        LwwSide::Ours => conflict.ours.clone(),
        LwwSide::Theirs => conflict.theirs.clone(),
    }
}
