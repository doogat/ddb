//! Service methods that still depend on the concrete SQLite `Index`.
//!
//! The collaborators called here (sync, bundle import, rename, consistency,
//! attachments) accept a concrete `&Index`, so these methods stay on
//! `DoogatService<G, Index>`. Generifying them over `IndexPort` would cascade
//! through all those modules — a deliberate stop point per the PRD 00142 split
//! criteria, not an oversight. Production always uses `I = Index`.

use std::path::Path;

use crate::error::{DoogatError, Result};
use crate::git_ops;
use crate::indexer::Index;
use crate::sync_manager::SyncManager;
use crate::traits::GitBackend;
use crate::types::{AttachmentInfo, DoogatId, FixReport, RenameReport, SyncReport};

use super::DoogatService;

impl<G: GitBackend> DoogatService<G, Index> {
    pub fn sync(&self, remote: &str, branch: &str) -> Result<SyncReport> {
        let mut mgr = match SyncManager::open(&self.repo) {
            Ok(m) => m,
            Err(DoogatError::NotFound(msg)) => {
                let node_file = self.repo.repo_path().join(".git/ddb-node");
                if node_file.exists() {
                    return Err(DoogatError::NotFound(msg));
                }
                let name = hostname::get()
                    .map(|h| h.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| "unknown".to_string());
                tracing::info!(node_name = %name, "auto-registering node for first sync");
                self.register_node(&name)?;
                SyncManager::open(&self.repo)?
            }
            Err(e) => return Err(e),
        };
        mgr.sync(remote, branch, &self.index)
    }

    pub fn import_bundle(&self, path: &Path) -> Result<SyncReport> {
        let mut mgr = SyncManager::open(&self.repo)?;
        crate::bundle::import_bundle(&self.repo, &mut mgr, &self.index, path)
    }

    pub fn rename_doogat(&self, id: &str, new_path: &str) -> Result<RenameReport> {
        let old_path = self.index.resolve_path(id)?;
        git_ops::rename_doogat(&self.repo, &self.index, &old_path, new_path)
    }

    pub fn fix_all(&self, dry_run: bool) -> Result<FixReport> {
        crate::consistency::fix_all(&self.repo, &self.index, dry_run)
    }

    pub fn zone_migrate_all(&self, dry_run: bool) -> Result<FixReport> {
        crate::consistency::zone_migrate_all(&self.repo, &self.index, dry_run)
    }

    pub fn attach_file(
        &self,
        doogat_id: &str,
        filename: &str,
        bytes: &[u8],
        mime: &str,
    ) -> Result<AttachmentInfo> {
        let id = DoogatId(doogat_id.to_owned());
        crate::attachments::attach_file(&self.repo, &self.index, &id, filename, bytes, mime)
    }

    pub fn detach_file(&self, doogat_id: &str, filename: &str) -> Result<()> {
        let id = DoogatId(doogat_id.to_owned());
        crate::attachments::detach_file(&self.repo, &self.index, &id, filename)
    }
}
