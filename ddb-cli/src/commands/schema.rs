use ddb_core::app_contract::ApplySchemaCommand;
use ddb_core::service::DoogatService;

use crate::SchemaAction;

pub(crate) fn schema(repo: &std::path::Path, action: SchemaAction) -> ddb_core::error::Result<()> {
    let (file, dry_run, allow_destructive) = match action {
        // `diff` is exactly `apply --dry-run` (no destructive override): route
        // through the same code path so their stdout is byte-identical.
        SchemaAction::Diff { file } => (file, true, false),
        SchemaAction::Apply {
            file,
            dry_run,
            allow_destructive,
        } => (file, dry_run, allow_destructive),
    };

    let schema_doc = std::fs::read_to_string(&file)?;
    let mut svc = DoogatService::open(repo)?;
    let cmd = ApplySchemaCommand {
        schema_doc,
        dry_run,
        allow_destructive,
    };
    let output = svc.apply_schema(cmd)?;

    crate::warnings::write_warnings(&output.warnings, &mut std::io::stderr())
        .unwrap_or_else(|e| eprintln!("warning write failed: {e}"));

    let report = output.value;
    if report.ops.is_empty() {
        outln!("no changes")?;
    } else {
        for op in &report.ops {
            outln!(
                "{} {} destructive={} -- {} -- {}",
                op.kind,
                op.table,
                op.destructive,
                op.detail,
                op.sql
            )?;
        }
    }
    Ok(())
}
