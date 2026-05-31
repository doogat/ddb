use crate::error::{DoogatError, Result};
use crate::parser;
use crate::types::{
    BrokenSequence, LinkDensityEntry, OrphanDoogat, RecentDoogat, SequenceInfo, SequenceNode,
    StaleDoogat, Suggestion, UnlinkedMention,
};

use crate::traits::{GitBackend, IndexPort};

use super::DoogatService;

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    // ── Discovery / Sequences ───────────────────────────────────────────

    pub fn unlinked_mentions(&self, id: &str) -> Result<Vec<UnlinkedMention>> {
        self.ensure_fresh()?;
        self.index.unlinked_mentions(id)
    }

    pub fn suggest_links(&self, id: &str, limit: usize) -> Result<Vec<Suggestion>> {
        self.ensure_fresh()?;
        self.index.suggest_links(id, limit)
    }

    pub fn stale_doogats(&self, type_filter: Option<&str>) -> Result<Vec<StaleDoogat>> {
        self.ensure_fresh()?;
        self.index.stale_doogats(&self.repo, type_filter)
    }

    pub fn orphan_doogats(&self, type_filter: Option<&str>) -> Result<Vec<OrphanDoogat>> {
        self.ensure_fresh()?;
        self.index.orphan_doogats(type_filter)
    }

    pub fn recent_doogats(
        &self,
        days: u32,
        type_filter: Option<&str>,
    ) -> Result<Vec<RecentDoogat>> {
        self.ensure_fresh()?;
        self.index.recent_doogats(days, type_filter)
    }

    pub fn link_density(&self, type_filter: Option<&str>) -> Result<Vec<LinkDensityEntry>> {
        self.ensure_fresh()?;
        self.index.link_density(type_filter)
    }

    pub fn sequence_tree(&self, id: &str, max_depth: usize) -> Result<Vec<(SequenceNode, usize)>> {
        self.ensure_fresh()?;
        self.index.sequence_tree(id, max_depth)
    }

    pub fn sequence_breadcrumb(&self, id: &str) -> Result<Vec<SequenceNode>> {
        self.ensure_fresh()?;
        self.index.sequence_breadcrumb(id)
    }

    pub fn broken_sequences(&self) -> Result<Vec<BrokenSequence>> {
        self.ensure_fresh()?;
        self.index.broken_sequences()
    }

    pub fn sequence_info(&self, id: &str) -> Result<SequenceInfo> {
        self.ensure_fresh()?;
        self.index.sequence_info(id)
    }

    pub fn sequence_children(&self, id: &str) -> Result<Vec<SequenceNode>> {
        self.ensure_fresh()?;
        self.index.sequence_children(id)
    }

    /// Return backlink source IDs for a given doogat path/ID.
    pub fn backlink_ids(&self, id: &str) -> Result<Vec<String>> {
        self.ensure_fresh()?;
        self.index.backlinks(id)
    }

    /// Install a bundled type definition, returning the new doogat ID.
    pub fn install_bundled_type(&self, name: &str) -> Result<String> {
        let content = crate::bundled_types::get_bundled_type(name).ok_or_else(|| {
            DoogatError::BadRequest(format!(
                "unknown bundled type \"{name}\". available: {:?}",
                crate::bundled_types::list_bundled_types()
            ))
        })?;

        let id = parser::generate_id();
        let full_content = content.replacen("---\n", &format!("---\nid: {}\n", id), 1);
        let path = format!("ddb/_typedef/{}.md", id);
        self.repo
            .commit_file(&path, &full_content, &format!("install type {name}"))?;
        let parsed = parser::parse(&full_content, &path)?;
        self.index.index_doogat(&parsed)?;

        Ok(id.to_string())
    }

    /// List all non-typedef doogat IDs.
    pub fn all_doogat_ids(&self) -> Result<Vec<String>> {
        self.ensure_fresh()?;
        let rows = self
            .index
            .query_raw("SELECT id FROM doogats WHERE path NOT LIKE 'ddb/_typedef/%'")?;
        Ok(rows
            .into_iter()
            .filter_map(|r| r.into_iter().next())
            .collect())
    }
}
