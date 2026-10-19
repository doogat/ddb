mod append_log;
mod body;
mod frontmatter;
mod lww;
#[cfg(test)]
mod tests;

pub use append_log::resolve_append_log;
pub use body::{merge_body, merge_reference};
pub use frontmatter::{merge_frontmatter, resolve_conflicts};
pub use lww::resolve_lww;
pub(crate) use lww::{lww_pick, LwwSide};

use crate::error::{DoogatError, Result};
use crate::types::{ConflictFile, ResolvedFile};

impl From<automerge::AutomergeError> for DoogatError {
    fn from(e: automerge::AutomergeError) -> Self {
        Self::Automerge(e.to_string())
    }
}

/// Default CRDT conflict resolver using Automerge.
pub struct DefaultResolver;

impl crate::traits::ConflictResolver for DefaultResolver {
    fn resolve_conflicts(
        &self,
        conflicts: Vec<ConflictFile>,
        strategy: Option<&str>,
    ) -> Result<Vec<ResolvedFile>> {
        resolve_conflicts(conflicts, strategy)
    }
}
