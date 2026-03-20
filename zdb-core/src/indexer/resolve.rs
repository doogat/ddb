use rusqlite::params;

use crate::error::Result;
use crate::traits::ZettelSource;

use super::Index;

impl Index {
    /// Resolve the git-relative path for a zettel ID using the index.
    pub fn resolve_path(&self, id: &str) -> Result<String> {
        self.conn
            .query_row(
                "SELECT path FROM zettels WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(|_| crate::error::ZettelError::NotFound(format!("zettel {id}")))
    }

    /// Check if a type's typedef has `folder: true`.
    /// Returns false if no typedef exists or folder is not set.
    pub fn type_uses_folder(
        &self,
        type_name: &str,
        repo: &(impl ZettelSource + ?Sized),
    ) -> bool {
        // Find the typedef zettel for this type
        let sql = "SELECT path FROM zettels WHERE type = '_typedef' AND title = ?1 LIMIT 1";
        let path: Option<String> = self
            .conn
            .query_row(sql, params![type_name], |row| row.get(0))
            .ok();
        let Some(path) = path else { return false };
        let Ok(content) = repo.read_file(&path) else {
            return false;
        };
        let Ok(parsed) = crate::parser::parse(&content, &path) else {
            return false;
        };
        parsed
            .meta
            .extra
            .get("folder")
            .map(|v| matches!(v, crate::types::Value::Bool(true)) || v.as_str() == Some("true"))
            .unwrap_or(false)
    }

    /// Resolve a zettel ID from an alias (case-insensitive).
    pub fn resolve_alias(&self, name: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT zettel_id FROM _zdb_aliases WHERE alias = ?1 LIMIT 1",
            params![name],
            |row| row.get(0),
        );
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Resolve a wikilink target to a zettel path.
    /// Resolution chain: path lookup → ID lookup → alias lookup.
    pub fn resolve_wikilink(&self, target: &str) -> Result<Option<String>> {
        // 1. Try as direct path (path-qualified wikilinks)
        let path_exists: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM zettels WHERE path = ?1",
                params![target],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if path_exists {
            return Ok(Some(target.to_string()));
        }
        // 2. Try as zettel ID
        if let Ok(path) = self.resolve_path(target) {
            return Ok(Some(path));
        }
        // 3. Try as alias
        if let Some(id) = self.resolve_alias(target)? {
            return Ok(Some(self.resolve_path(&id)?));
        }
        // 4. Partial path matching — match tail path segments
        let bare = target.strip_suffix(".md").unwrap_or(target);
        // Escape LIKE wildcards so _ and % in zettel names are matched literally
        let escaped = bare.replace('%', "\\%").replace('_', "\\_");
        let partial: Option<String> = self
            .conn
            .query_row(
                "SELECT path FROM zettels WHERE path LIKE '%/' || ?1 || '.md' ESCAPE '\\' ORDER BY length(path) ASC LIMIT 1",
                params![escaped],
                |row| row.get(0),
            )
            .ok();
        if let Some(path) = partial {
            return Ok(Some(path));
        }
        Ok(None)
    }
}
