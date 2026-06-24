//! The `DoogatDriver` UniFFI object and its exported methods.
//!
//! Split out of `ffi.rs` per PRD 00156.

use std::path::Path;
use std::sync::Mutex;

use crate::error::DoogatError;
use crate::service::DoogatService;

use super::records::{
    empty_error_context, AttachmentInfo, DdbError, PaginatedSearchResult, RebuildReport,
    SchemaApplyReportRecord, SearchResult, SqlResultRecord, TypeSchemaRecord,
};

/// High-level facade for mobile/desktop FFI consumers.
///
/// Wraps a single `Mutex<DoogatService>` for thread safety.
#[derive(uniffi::Object)]
pub struct DoogatDriver {
    pub(super) svc: Mutex<DoogatService>,
}

impl DoogatDriver {
    fn with_service<T, F>(&self, f: F) -> Result<T, DdbError>
    where
        F: FnOnce(&DoogatService) -> Result<T, DdbError>,
    {
        let svc = self.svc.lock().map_err(|e| DdbError::Io {
            msg: format!("service lock poisoned: {e}"),
        })?;
        f(&svc)
    }

    fn with_service_mut<T, F>(&self, f: F) -> Result<T, DdbError>
    where
        F: FnOnce(&mut DoogatService) -> Result<T, DdbError>,
    {
        let mut svc = self.svc.lock().map_err(|e| DdbError::Io {
            msg: format!("service lock poisoned: {e}"),
        })?;
        f(&mut svc)
    }
}

#[uniffi::export]
impl DoogatDriver {
    /// Open an existing Doogat DB repository.
    #[uniffi::constructor]
    pub fn new(repo_path: String) -> Result<Self, DdbError> {
        let svc = DoogatService::open(Path::new(&repo_path)).map_err(DdbError::from)?;
        Ok(Self {
            svc: Mutex::new(svc),
        })
    }

    /// Initialize a new Doogat DB repository at `repo_path` and open it.
    #[uniffi::constructor]
    pub fn create_repo(repo_path: String) -> Result<Self, DdbError> {
        let svc = DoogatService::init(Path::new(&repo_path)).map_err(DdbError::from)?;
        Ok(Self {
            svc: Mutex::new(svc),
        })
    }

    pub fn create_doogat(&self, content: String, message: String) -> Result<String, DdbError> {
        self.with_service(|svc| {
            svc.create_doogat_raw(&content, &message)
                .map_err(DdbError::from)
        })
    }

    pub fn read_doogat(&self, id: String) -> Result<String, DdbError> {
        self.with_service(|svc| svc.read_doogat(&id).map_err(DdbError::from))
    }

    pub fn update_doogat(
        &self,
        id: String,
        content: String,
        message: String,
    ) -> Result<(), DdbError> {
        self.with_service(|svc| {
            svc.update_doogat_raw(&id, &content, &message)
                .map_err(DdbError::from)
        })
    }

    pub fn delete_doogat(&self, id: String, message: String) -> Result<(), DdbError> {
        self.with_service(|svc| {
            svc.delete_doogat(&id, &message).map_err(DdbError::from)?;
            Ok(())
        })
    }

    pub fn search(&self, query: String) -> Result<Vec<SearchResult>, DdbError> {
        self.with_service(|svc| {
            let results = svc.search(&query).map_err(DdbError::from)?;
            Ok(results.into_iter().map(Into::into).collect())
        })
    }

    pub fn search_paginated(
        &self,
        query: String,
        limit: u32,
        offset: u32,
    ) -> Result<PaginatedSearchResult, DdbError> {
        self.with_service(|svc| {
            let result = svc
                .search_paginated(&query, limit as usize, offset as usize)
                .map_err(DdbError::from)?;
            Ok(result.into())
        })
    }

    pub fn reindex(&self) -> Result<RebuildReport, DdbError> {
        self.with_service(|svc| {
            let report = svc.reindex().map_err(DdbError::from)?;
            Ok(RebuildReport {
                indexed: report.indexed as u64,
                tables_materialized: report.tables_materialized as u64,
                types_inferred: report.types_inferred,
            })
        })
    }

    pub fn register_node(&self, name: String) -> Result<String, DdbError> {
        self.with_service(|svc| {
            let node = svc.register_node(&name).map_err(DdbError::from)?;
            Ok(node.uuid)
        })
    }

    pub fn compact(&self) -> Result<(), DdbError> {
        self.with_service(|svc| {
            let opts = crate::types::CompactOptions {
                force: true,
                skip_backup: true,
                ..Default::default()
            };
            svc.compact(&opts).map_err(DdbError::from)?;
            Ok(())
        })
    }

    pub fn run_maintenance(&self) -> Result<bool, DdbError> {
        self.with_service(|svc| {
            let report = svc.run_maintenance(None).map_err(DdbError::from)?;
            Ok(report.success)
        })
    }

    pub fn list_doogats(&self) -> Result<Vec<String>, DdbError> {
        self.with_service(|svc| svc.list_doogats().map_err(DdbError::from))
    }

    pub fn execute_sql(&self, sql: String) -> Result<SqlResultRecord, DdbError> {
        self.with_service_mut(|svc| {
            let result = svc.execute_sql(&sql).map_err(DdbError::from)?;
            Ok(result.into())
        })
    }

    pub fn apply_schema(
        &self,
        schema_doc: String,
        dry_run: bool,
        allow_destructive: bool,
    ) -> Result<SchemaApplyReportRecord, DdbError> {
        self.with_service_mut(|svc| {
            let out = svc
                .apply_schema(crate::app_contract::ApplySchemaCommand {
                    schema_doc,
                    dry_run,
                    allow_destructive,
                })
                .map_err(DdbError::from)?;
            Ok(SchemaApplyReportRecord::from_output(out))
        })
    }

    pub fn begin_transaction(&self) -> Result<(), DdbError> {
        self.with_service_mut(|svc| svc.begin_transaction().map_err(DdbError::from))
    }

    pub fn commit_transaction(&self) -> Result<(), DdbError> {
        self.with_service_mut(|svc| svc.commit_transaction().map_err(DdbError::from))
    }

    pub fn rollback_transaction(&self) -> Result<(), DdbError> {
        self.with_service_mut(|svc| svc.rollback_transaction().map_err(DdbError::from))
    }

    pub fn list_type_schemas(&self) -> Result<Vec<TypeSchemaRecord>, DdbError> {
        self.with_service(|svc| {
            let schemas = svc.list_type_schemas().map_err(DdbError::from)?;
            Ok(schemas.into_iter().map(Into::into).collect())
        })
    }

    pub fn attach_file(
        &self,
        doogat_id: String,
        file_path: String,
    ) -> Result<AttachmentInfo, DdbError> {
        let bytes = std::fs::read(&file_path).map_err(|e| DdbError::from(DoogatError::Io(e)))?;
        let filename = Path::new(&file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| DdbError::Validation {
                msg: "invalid filename".into(),
                code: None,
                context: empty_error_context(),
            })?
            .to_owned();
        let mime = crate::types::AttachmentInfo::mime_from_filename(&filename).to_owned();
        self.with_service(|svc| {
            let info = svc
                .attach_file(&doogat_id, &filename, &bytes, &mime)
                .map_err(DdbError::from)?;
            Ok(info.into())
        })
    }

    pub fn detach_file(&self, doogat_id: String, filename: String) -> Result<(), DdbError> {
        self.with_service(|svc| {
            svc.detach_file(&doogat_id, &filename)
                .map_err(DdbError::from)
        })
    }

    pub fn list_attachments(&self, doogat_id: String) -> Result<Vec<AttachmentInfo>, DdbError> {
        self.with_service(|svc| {
            let list = svc.list_attachments(&doogat_id).map_err(DdbError::from)?;
            Ok(list.into_iter().map(AttachmentInfo::from).collect())
        })
    }

    pub fn export_full_bundle(&self, output_path: String) -> Result<String, DdbError> {
        self.with_service(|svc| {
            let path = svc
                .export_full_bundle(Path::new(&output_path))
                .map_err(DdbError::from)?;
            Ok(path.to_string_lossy().into_owned())
        })
    }

    pub fn export_delta_bundle(
        &self,
        target_node_uuid: String,
        output_path: String,
    ) -> Result<String, DdbError> {
        self.with_service(|svc| {
            let path = svc
                .export_delta_bundle(&target_node_uuid, Path::new(&output_path))
                .map_err(DdbError::from)?;
            Ok(path.to_string_lossy().into_owned())
        })
    }

    pub fn import_bundle(&self, bundle_path: String) -> Result<(), DdbError> {
        self.with_service(|svc| {
            svc.import_bundle(Path::new(&bundle_path))
                .map_err(DdbError::from)?;
            Ok(())
        })
    }
}
