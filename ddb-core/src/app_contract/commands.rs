//! Command types for the application contract layer.

use crate::error::{DoogatError, Result};
use crate::types::{ConflictAction, Value};
use std::collections::BTreeMap;

/// Whether `DoogatService::create` rejects an unregistered `doogat_type` or
/// falls back to a base-only create. Makes create strictness an explicit
/// per-caller input (PRD 00155) instead of an accident of shared routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnregisteredTypePolicy {
    /// Reject an unregistered `doogat_type` with `TYPE_NOT_REGISTERED`.
    /// GraphQL `createDoogat` and the REST POST handler use this (default).
    #[default]
    Strict,
    /// Create a base doogat (no typed validation) when `doogat_type` names a
    /// type with no registered `_typedef`, attaching an
    /// `UNREGISTERED_TYPE_BASE_ONLY` warning. The CLI uses this to preserve
    /// the released (≤ v0.2.5) contract.
    BaseOnly,
}

/// Command to create a new doogat. `title` and `body` are optional so
/// transports can omit them and rely on service-level defaults (e.g.
/// `title_template`). `on_conflict` controls behaviour when a doogat with
/// the same identity already exists.
#[derive(Debug, Clone)]
pub struct CreateCommand {
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub doogat_type: Option<String>,
    pub body: Option<String>,
    pub fields: BTreeMap<String, Value>,
    pub on_conflict: ConflictAction,
    pub unregistered_type_policy: UnregisteredTypePolicy,
}

/// Command to fetch a single doogat by its 14-digit ID.
#[derive(Debug, Clone)]
pub struct ReadCommand {
    pub id: String,
}

/// Command to modify an existing doogat identified by `id`. Only `Some`
/// fields are applied; `None` leaves the current value unchanged. An empty
/// `fields` map means no field changes.
#[derive(Debug, Clone)]
pub struct UpdateCommand {
    pub id: String,
    pub title: Option<String>,
    pub tags: Option<Vec<String>>,
    pub doogat_type: Option<String>,
    pub body: Option<String>,
    /// Fields to set or update. An empty map means no field changes.
    pub fields: BTreeMap<String, Value>,
    /// Field names to remove from the doogat's frontmatter. An empty vec
    /// means no removals.
    pub unset_fields: Vec<String>,
}

/// Command to delete a doogat by its 14-digit ID. The service returns a
/// result that includes any broken backlinks created by the deletion, which
/// transports should surface rather than discard.
#[derive(Debug, Clone)]
pub struct DeleteCommand {
    pub id: String,
}

/// Command to apply a declarative desired-schema document. `schema_doc` is the
/// YAML describing the desired typedefs. `dry_run` returns the plan without
/// mutating; `allow_destructive` permits drop/rename ops that would otherwise
/// be blocked (PRD 00161 §3.5).
#[derive(Debug, Clone)]
pub struct ApplySchemaCommand {
    pub schema_doc: String,
    pub dry_run: bool,
    pub allow_destructive: bool,
}

/// A validated page window. Build one from raw transport input with
/// [`PageRequest::decode`], which rejects out-of-range values instead of
/// clamping them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    pub limit: usize,
    pub offset: usize,
}

impl PageRequest {
    pub const DEFAULT_LIMIT: usize = 50;
    pub const MAX_LIMIT: usize = 10_000;

    /// Decode raw `limit`/`offset` input. An absent `limit` defaults to
    /// [`Self::DEFAULT_LIMIT`]; an absent `offset` defaults to 0. A `limit`
    /// outside `1..=MAX_LIMIT` or a negative `offset` is a
    /// `DoogatError::Validation` error.
    pub fn decode(limit: Option<i64>, offset: Option<i64>) -> Result<Self> {
        let limit = match limit {
            None => Self::DEFAULT_LIMIT,
            Some(l) => {
                if l <= 0 || l > Self::MAX_LIMIT as i64 {
                    return Err(DoogatError::Validation(format!(
                        "limit must be between 1 and {}, got {l}",
                        Self::MAX_LIMIT
                    )));
                }
                usize::try_from(l).map_err(|_| {
                    DoogatError::Validation(format!("limit {l} does not fit in usize"))
                })?
            }
        };
        let offset = match offset {
            None => 0,
            Some(o) => {
                if o < 0 {
                    return Err(DoogatError::Validation(format!(
                        "offset must be non-negative, got {o}"
                    )));
                }
                usize::try_from(o).map_err(|_| {
                    DoogatError::Validation(format!("offset {o} does not fit in usize"))
                })?
            }
        };
        Ok(Self { limit, offset })
    }
}

/// Command to run a full-text search against the SQLite FTS5 index.
///
/// `query` is passed through the search-query normalizer. `page` is the
/// validated page window. `types` keeps only hits whose `doogat_type` is
/// listed; `tag` keeps only hits carrying that tag. `None` means no filter.
#[derive(Debug, Clone)]
pub struct SearchCommand {
    pub query: String,
    pub page: PageRequest,
    pub types: Option<Vec<String>>,
    pub tag: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::PageRequest;
    use crate::error::{DoogatError, Result};

    /// Every out-of-range input is a hard `Validation` error, never a
    /// silently clamped page.
    fn assert_validation(result: Result<PageRequest>, what: &str) {
        match result {
            Err(DoogatError::Validation(_)) => {}
            other => panic!("{what}: expected DoogatError::Validation, got {other:?}"),
        }
    }

    #[test]
    fn default_and_max_limit_constants_are_pinned() {
        assert_eq!(PageRequest::DEFAULT_LIMIT, 50);
        assert_eq!(PageRequest::MAX_LIMIT, 10_000);
    }

    #[test]
    fn defaults_to_limit_50_and_offset_0_when_both_absent() {
        let page = PageRequest::decode(None, None).expect("absent limit/offset must decode");
        assert_eq!(page, PageRequest { limit: 50, offset: 0 });
    }

    #[test]
    fn absent_offset_defaults_to_zero_while_keeping_explicit_limit() {
        let page = PageRequest::decode(Some(25), None).expect("valid limit must decode");
        assert_eq!(page, PageRequest { limit: 25, offset: 0 });
    }

    #[test]
    fn absent_limit_defaults_to_50_while_keeping_explicit_offset() {
        let page = PageRequest::decode(None, Some(7)).expect("valid offset must decode");
        assert_eq!(page, PageRequest { limit: 50, offset: 7 });
    }

    #[test]
    fn accepts_limit_of_one_as_the_lower_bound() {
        let page = PageRequest::decode(Some(1), Some(0)).expect("limit 1 is in range");
        assert_eq!(page, PageRequest { limit: 1, offset: 0 });
    }

    #[test]
    fn accepts_limit_of_exactly_max() {
        let page = PageRequest::decode(Some(10_000), Some(0)).expect("limit 10,000 is in range");
        assert_eq!(page, PageRequest { limit: 10_000, offset: 0 });
    }

    #[test]
    fn accepts_offset_of_zero() {
        let page = PageRequest::decode(Some(10), Some(0)).expect("offset 0 is in range");
        assert_eq!(page.offset, 0);
    }

    #[test]
    fn round_trips_explicit_limit_and_offset_exactly() {
        let page = PageRequest::decode(Some(37), Some(1234)).expect("in-range values decode");
        assert_eq!(page.limit, 37);
        assert_eq!(page.offset, 1234);
    }

    #[test]
    fn rejects_zero_limit() {
        assert_validation(PageRequest::decode(Some(0), None), "limit 0");
    }

    #[test]
    fn rejects_negative_limit() {
        assert_validation(PageRequest::decode(Some(-1), None), "limit -1");
        assert_validation(PageRequest::decode(Some(i64::MIN), None), "limit i64::MIN");
    }

    #[test]
    fn rejects_limit_one_over_max() {
        assert_validation(PageRequest::decode(Some(10_001), None), "limit 10,001");
    }

    #[test]
    fn rejects_limit_far_over_max_instead_of_clamping() {
        assert_validation(PageRequest::decode(Some(i64::MAX), None), "limit i64::MAX");
    }

    #[test]
    fn rejects_negative_offset() {
        assert_validation(PageRequest::decode(None, Some(-1)), "offset -1");
        assert_validation(PageRequest::decode(Some(10), Some(i64::MIN)), "offset i64::MIN");
    }

    #[test]
    fn rejects_invalid_offset_even_when_limit_is_valid() {
        assert_validation(PageRequest::decode(Some(10), Some(-5)), "valid limit, offset -5");
    }

    #[test]
    fn rejects_invalid_limit_even_when_offset_is_valid() {
        assert_validation(PageRequest::decode(Some(0), Some(5)), "limit 0, valid offset");
    }

    /// On 64-bit targets every non-negative i64 fits in usize, so the largest
    /// offset must decode unchanged (no clamping toward some smaller cap).
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn accepts_largest_i64_offset_without_clamping_on_64_bit() {
        let page = PageRequest::decode(Some(10), Some(i64::MAX)).expect("i64::MAX fits usize");
        assert_eq!(page.offset, i64::MAX as usize);
    }

    /// On 32-bit targets an offset above `u32::MAX` cannot convert to usize
    /// and must be rejected, not truncated.
    #[cfg(target_pointer_width = "32")]
    #[test]
    fn rejects_offset_that_overflows_usize_on_32_bit() {
        assert_validation(
            PageRequest::decode(Some(10), Some(i64::from(u32::MAX) + 1)),
            "offset u32::MAX + 1",
        );
    }
}
