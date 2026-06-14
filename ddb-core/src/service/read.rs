use crate::error::Result;
use crate::parser;
use crate::types::ParsedDoogat;

use crate::traits::{GitBackend, IndexPort};

use super::DoogatService;

impl<G: GitBackend, I: IndexPort> DoogatService<G, I> {
    /// Read a doogat's raw content by ID.
    pub fn read_doogat(&self, id: &str) -> Result<String> {
        self.ensure_fresh()?;
        let path = self.index.resolve_path(id)?;
        self.repo.read_file(&path)
    }

    /// Read raw content by git-relative path, skipping index freshness check.
    pub fn read_doogat_raw(&self, path: &str) -> Result<String> {
        self.repo.read_file(path)
    }

    /// Read and parse a doogat by ID, returning a fully parsed doogat.
    pub fn get_doogat_parsed(&self, id: &str) -> Result<ParsedDoogat> {
        self.ensure_fresh()?;
        let path = self.index.resolve_path(id)?;
        let content = self.repo.read_file(&path)?;
        let mut parsed = parser::parse(&content, &path)?;
        parsed.updated_at = self.index.lookup_updated_at(id).unwrap_or(None);
        Ok(parsed)
    }

    /// Batch-fetch multiple doogats by ID, skipping any that fail to resolve or parse.
    pub fn get_doogats_batch(&self, ids: &[String]) -> Result<Vec<ParsedDoogat>> {
        self.ensure_fresh()?;
        let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        let updated_map = self
            .index
            .lookup_updated_at_batch(&id_refs)
            .unwrap_or_default();
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            if let Ok(path) = self.index.resolve_path(id) {
                if let Ok(content) = self.repo.read_file(&path) {
                    if let Ok(mut parsed) = parser::parse(&content, &path) {
                        parsed.updated_at = updated_map.get(id.as_str()).cloned();
                        results.push(parsed);
                    }
                }
            }
        }
        Ok(results)
    }
}
