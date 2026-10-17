use std::sync::{Arc, Mutex};

use chrono::Utc;
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

use crate::events::{DoogatEvent, EventBus, EventKind};

/// Explicit event intent passed to [`super::ActorHandle::call`] for event-bearing verbs.
///
/// Non-mutating verbs use [`EventIntent::None`]. The intent declares *what kind* of
/// event a verb emits, while the closure's actual result decides *whether* to emit
/// (only `Ok` emits).
pub enum EventIntent {
    /// Reads and non-mutating statements: emit nothing.
    None,
    /// `create`: one `Created` for the returned doogat.
    Created,
    /// `update`: one `Updated` for the returned doogat.
    Updated,
    /// `delete`: one `Deleted`. The closure resolves the type into the shared cell
    /// BEFORE deleting (the row is gone afterward); emission reads it back after.
    Deleted {
        id: String,
        doogat_type: Arc<Mutex<Option<String>>>,
    },
    /// `batch_create`: one `Created` per returned doogat.
    CreateMany,
    /// `batch_update`: one `Updated` per returned doogat.
    BatchUpdate,
    /// `upsert_singleton`: `Created` if the outcome created a doogat, else `Updated`.
    Upsert { type_name: String },
}

/// Emit the `DoogatEvent`s a just-run verb produced, derived from its explicit
/// [`EventIntent`] and its actual result.
///
/// Emission happens at the closure boundary: the **intent** decides the event
/// *kind*, the **result** (only `Ok` emits) supplies the id/type. Because `call` is
/// generic over the verb's return type `R`, the per-type knowledge of how to read
/// the doogat(s) out of an `Ok` result lives in this trait. Verb return types that
/// never emit (`Deleted` aside) can rely on the [`no_emit_intent!`] macro; the
/// mutation payloads carry a hand-written impl.
pub trait IntentEmit {
    /// Emit any events this result warrants under `intent`. `Err` results emit
    /// nothing.
    fn emit(&self, bus: &EventBus, intent: &EventIntent);
}

/// Emit a `Deleted` event when the intent is `Deleted` and the result is `Ok`.
///
/// `Deleted` carries its id/type in the intent (resolved before the delete ran), so
/// it is the one intent whose emission does not read the Ok payload. Every
/// [`IntentEmit`] impl calls this first, then handles its own payload-bearing kinds.
fn emit_deleted_if<T>(bus: &EventBus, intent: &EventIntent, result: &Result<T, DoogatError>) {
    if let EventIntent::Deleted { id, doogat_type } = intent {
        if result.is_ok() {
            bus.send(DoogatEvent {
                kind: EventKind::Deleted,
                doogat_id: id.clone(),
                doogat_type: doogat_type
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
                timestamp: Utc::now(),
            });
        }
    }
}

/// Implement [`IntentEmit`] for verb return types that emit only under `Deleted`
/// (reads, counts, SQL results, attachment info, maintenance reports, ...). A
/// mutating verb that returns one of these payload types (e.g. `delete` returns
/// `ActorResult<()>`) still emits via the `Deleted` intent.
macro_rules! no_emit_intent {
    ($($ty:ty),* $(,)?) => {
        $(
            impl IntentEmit for Result<$ty, DoogatError> {
                fn emit(&self, bus: &EventBus, intent: &EventIntent) {
                    emit_deleted_if(bus, intent, self);
                }
            }
        )*
    };
}

no_emit_intent!(
    (),
    bool,
    i64,
    ParsedDoogat,
    Option<ParsedDoogat>,
    PaginatedSearchResult,
    SqlResult,
    Vec<SqlResult>,
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
);

impl IntentEmit for Result<AppOutput<SchemaApplyReport>, DoogatError> {
    fn emit(&self, bus: &EventBus, intent: &EventIntent) {
        emit_deleted_if(bus, intent, self);
    }
}

/// `create`/`update`: emit one `Created`/`Updated` for the returned doogat.
impl IntentEmit for Result<AppOutput<ParsedDoogat>, DoogatError> {
    fn emit(&self, bus: &EventBus, intent: &EventIntent) {
        emit_deleted_if(bus, intent, self);
        if let Ok(output) = self {
            match intent {
                EventIntent::Created => {
                    bus.send(doogat_event(&EventKind::Created, &output.value, Utc::now()))
                }
                EventIntent::Updated => {
                    bus.send(doogat_event(&EventKind::Updated, &output.value, Utc::now()))
                }
                _ => {}
            }
        }
    }
}

/// `create_many`/`batch_update`: emit one `Created`/`Updated` per returned doogat.
impl IntentEmit for Result<Vec<ParsedDoogat>, DoogatError> {
    fn emit(&self, bus: &EventBus, intent: &EventIntent) {
        emit_deleted_if(bus, intent, self);
        if let Ok(doogats) = self {
            let kind = match intent {
                EventIntent::CreateMany => EventKind::Created,
                EventIntent::BatchUpdate => EventKind::Updated,
                _ => return,
            };
            let now = Utc::now();
            for z in doogats {
                bus.send(doogat_event(&kind, z, now));
            }
        }
    }
}

/// `upsert_singleton`: emit `Created` iff the outcome created a doogat, else `Updated`.
impl IntentEmit for Result<UpsertOutcome, DoogatError> {
    fn emit(&self, bus: &EventBus, intent: &EventIntent) {
        emit_deleted_if(bus, intent, self);
        if let (EventIntent::Upsert { type_name }, Ok(outcome)) = (intent, self) {
            bus.send(DoogatEvent {
                kind: if outcome.created {
                    EventKind::Created
                } else {
                    EventKind::Updated
                },
                doogat_id: outcome.id.clone(),
                doogat_type: Some(type_name.clone()),
                timestamp: Utc::now(),
            });
        }
    }
}

fn doogat_event(
    kind: &EventKind,
    z: &ParsedDoogat,
    timestamp: chrono::DateTime<Utc>,
) -> DoogatEvent {
    DoogatEvent {
        kind: kind.clone(),
        doogat_id: z
            .meta
            .id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        doogat_type: z.meta.doogat_type.clone(),
        timestamp,
    }
}
