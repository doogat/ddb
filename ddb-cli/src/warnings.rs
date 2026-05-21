use ddb_core::app_contract::AppWarning;
use std::io::Write;

/// Write each warning to `w` as `warning: <code>: <message>\n`.
pub fn write_warnings(warnings: &[AppWarning], w: &mut impl Write) -> std::io::Result<()> {
    for warning in warnings {
        writeln!(w, "warning: {}: {}", warning.code, warning.message)?;
    }
    Ok(())
}
