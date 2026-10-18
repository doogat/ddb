//! The `DoogatDriver` UniFFI object and its exported methods.
//!
//! Split out of `ffi.rs` per PRD 00156.

use std::path::Path;
use std::sync::Mutex;

use crate::app_contract::{PageRequest, SearchCommand};
use crate::error::DoogatError;
use crate::service::DoogatService;

use super::records::{
    empty_error_context, fields_to_core, AttachmentInfo, ConflictActionRecord, CreateResultRecord,
    DdbError, FieldEntryValue, PaginatedSearchResult, RebuildReport, RebuildWarningRecord,
    SchemaApplyReportRecord, SearchResult, SqlResultRecord, TypeSchemaRecord, UpdateResultRecord,
    UpdateStructuredInput,
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

    /// Create a doogat from structured input, routed through the app contract.
    /// An unregistered `doogat_type` yields a base-only create with an
    /// `UNREGISTERED_TYPE_BASE_ONLY` warning.
    pub fn create_structured(
        &self,
        title: Option<String>,
        tags: Vec<String>,
        doogat_type: Option<String>,
        body: Option<String>,
        fields: Vec<FieldEntryValue>,
        on_conflict: ConflictActionRecord,
    ) -> Result<CreateResultRecord, DdbError> {
        self.with_service(|svc| {
            let cmd = crate::app_contract::CreateCommand {
                title,
                tags,
                doogat_type,
                body,
                fields: fields_to_core(fields),
                on_conflict: on_conflict.into(),
                unregistered_type_policy: crate::app_contract::FFI_UNREGISTERED_TYPE_POLICY,
            };
            let out = svc.create(cmd).map_err(DdbError::from)?;
            Ok(CreateResultRecord::from_output(out))
        })
    }

    pub fn read_doogat(&self, id: String) -> Result<String, DdbError> {
        self.with_service(|svc| svc.read_doogat(&id).map_err(DdbError::from))
    }

    /// Update a doogat from structured input, routed through the app contract.
    /// A `None` field of `input` leaves the stored value unchanged;
    /// `input.unset_fields` removes frontmatter keys.
    pub fn update_structured(
        &self,
        id: String,
        input: UpdateStructuredInput,
    ) -> Result<UpdateResultRecord, DdbError> {
        self.with_service(|svc| {
            let cmd = crate::app_contract::UpdateCommand {
                id,
                title: input.title,
                tags: input.tags,
                doogat_type: input.doogat_type,
                body: input.body,
                fields: fields_to_core(input.fields),
                unset_fields: input.unset_fields,
            };
            let out = svc.update(cmd).map_err(DdbError::from)?;
            Ok(UpdateResultRecord::from_output(out))
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
            let cmd = SearchCommand {
                query,
                page: PageRequest::decode(Some(i64::from(limit)), Some(i64::from(offset)))?,
                types: None,
                tag: None,
            };
            let output = svc.search_command(cmd)?;
            Ok(PaginatedSearchResult {
                hits: output.value.items.into_iter().map(Into::into).collect(),
                total_count: output.value.total as u64,
                warnings: output
                    .warnings
                    .into_iter()
                    .map(|w| RebuildWarningRecord {
                        code: w.code.to_string(),
                        message: w.message,
                    })
                    .collect(),
            })
        })
    }

    pub fn search_filtered(
        &self,
        query: String,
        limit: Option<u32>,
        offset: Option<u32>,
        types: Option<Vec<String>>,
        tag: Option<String>,
    ) -> Result<PaginatedSearchResult, DdbError> {
        self.with_service(|svc| {
            let cmd = SearchCommand {
                query,
                page: PageRequest::decode(limit.map(i64::from), offset.map(i64::from))?,
                types,
                tag,
            };
            let output = svc.search_command(cmd)?;
            Ok(PaginatedSearchResult {
                hits: output.value.items.into_iter().map(Into::into).collect(),
                total_count: output.value.total as u64,
                warnings: output
                    .warnings
                    .into_iter()
                    .map(|w| RebuildWarningRecord {
                        code: w.code.to_string(),
                        message: w.message,
                    })
                    .collect(),
            })
        })
    }

    pub fn reindex(&self) -> Result<RebuildReport, DdbError> {
        self.with_service(|svc| {
            let report = svc.reindex().map_err(DdbError::from)?;
            Ok(RebuildReport {
                indexed: report.indexed as u64,
                tables_materialized: report.tables_materialized as u64,
                types_inferred: report.types_inferred,
                warnings: report
                    .warnings
                    .into_iter()
                    .map(|w| RebuildWarningRecord {
                        code: crate::app_contract::describe_consistency_warning_code(&w)
                            .to_string(),
                        message: crate::app_contract::describe_consistency_warning(&w),
                    })
                    .collect(),
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
