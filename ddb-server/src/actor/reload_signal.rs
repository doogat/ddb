use ddb_core::app_contract::AppOutput;
use ddb_core::error::DoogatError;
use ddb_core::schema_diff::plan::SchemaApplyReport;
use ddb_core::service::UpsertOutcome;
use ddb_core::sql_engine::SqlResult;
use ddb_core::types::{
    BrokenSequence, CompactionReport, MaintenanceReport, OrphanDoogat, PaginatedSearchResult,
    ParsedDoogat, SequenceInfo, SequenceNode, StaleDoogat, Suggestion, SyncReport, TableSchema,
    UnlinkedMention,
};

/// Schema reload intent carried alongside the closure: whether `call` triggers
/// (and awaits) a schema reload after the verb completes.
pub enum ReloadIntent {
    /// Nothing can change the schema: no reload check, regardless of result.
    None,
    /// Caller has already decided, from SQL text classified via
    /// `requires_schema_reload` before the write ran. Reload iff
    /// `self.is_ok() && bool`.
    Explicit(bool),
    /// `applySchema` only: reload iff the result is `Ok` AND `!dry_run &&
    /// value.applied`.
    SchemaApply { dry_run: bool },
}

/// Reads whether a verb's result, under the given [`ReloadIntent`], must
/// trigger a schema reload.
pub trait ReloadSignal {
    fn schema_changed(&self, intent: &ReloadIntent) -> bool;
}

/// Implement [`ReloadSignal`] as "never reloads, whatever the intent" for verb
/// return types that cannot change the schema.
macro_rules! no_reload_intent {
    ($($ty:ty),* $(,)?) => {
        $(
            impl ReloadSignal for Result<$ty, DoogatError> {
                fn schema_changed(&self, _intent: &ReloadIntent) -> bool {
                    false
                }
            }
        )*
    };
}

no_reload_intent!(
    (),
    bool,
    i64,
    ParsedDoogat,
    Option<ParsedDoogat>,
    Vec<ParsedDoogat>,
    AppOutput<ParsedDoogat>,
    PaginatedSearchResult,
    Vec<TableSchema>,
    Vec<String>,
    ddb_core::types::AttachmentInfo,
    Vec<ddb_core::types::AttachmentInfo>,
    CompactionReport,
    MaintenanceReport,
    SyncReport,
    Vec<UnlinkedMention>,
    Vec<Suggestion>,
    Vec<StaleDoogat>,
    Vec<OrphanDoogat>,
    SequenceInfo,
    Vec<SequenceNode>,
    Vec<BrokenSequence>,
    UpsertOutcome,
);

impl ReloadSignal for Result<SqlResult, DoogatError> {
    fn schema_changed(&self, intent: &ReloadIntent) -> bool {
        matches!(intent, ReloadIntent::Explicit(true)) && self.is_ok()
    }
}

impl ReloadSignal for Result<Vec<SqlResult>, DoogatError> {
    fn schema_changed(&self, intent: &ReloadIntent) -> bool {
        matches!(intent, ReloadIntent::Explicit(true)) && self.is_ok()
    }
}

impl ReloadSignal for Result<AppOutput<SchemaApplyReport>, DoogatError> {
    fn schema_changed(&self, intent: &ReloadIntent) -> bool {
        matches!(intent, ReloadIntent::SchemaApply { dry_run: false })
            && matches!(self, Ok(o) if o.value.applied)
    }
}
