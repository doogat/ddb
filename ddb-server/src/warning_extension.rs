use std::sync::Mutex;

/// A warning entry with a code and message.
#[derive(Debug, Clone, PartialEq)]
pub struct WarningEntry {
    pub code: &'static str,
    pub message: String,
}

/// Collects warnings for a single request.
#[derive(Debug, Default)]
pub struct WarningCollector {
    warnings: Mutex<Vec<WarningEntry>>,
}

impl WarningCollector {
    /// Push a warning to the collector.
    pub fn push_warning(&self, code: &'static str, message: String) {
        self.warnings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(WarningEntry { code, message });
    }

    /// Drain all warnings from the collector.
    pub fn drain_warnings(&self) -> Vec<WarningEntry> {
        self.warnings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_collector_starts_empty() {
        let collector = WarningCollector::default();
        let warnings = collector.drain_warnings();
        assert!(warnings.is_empty());
    }

    #[test]
    fn push_warning_adds_entry_with_correct_code_and_message() {
        let collector = WarningCollector::default();
        collector.push_warning("DEPRECATED", "field x is deprecated".to_string());
        let warnings = collector.drain_warnings();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "DEPRECATED");
        assert_eq!(warnings[0].message, "field x is deprecated");
    }

    #[test]
    fn push_warning_multiple_entries_preserves_order() {
        let collector = WarningCollector::default();
        collector.push_warning("FIRST", "first warning".to_string());
        collector.push_warning("SECOND", "second warning".to_string());
        collector.push_warning("THIRD", "third warning".to_string());
        let warnings = collector.drain_warnings();
        assert_eq!(warnings.len(), 3);
        assert_eq!(warnings[0].code, "FIRST");
        assert_eq!(warnings[1].code, "SECOND");
        assert_eq!(warnings[2].code, "THIRD");
    }

    #[test]
    fn drain_warnings_returns_all_entries_then_leaves_collector_empty() {
        let collector = WarningCollector::default();
        collector.push_warning("CODE_A", "msg a".to_string());
        collector.push_warning("CODE_B", "msg b".to_string());

        let first_drain = collector.drain_warnings();
        assert_eq!(first_drain.len(), 2);

        // collector must now be empty
        let second_drain = collector.drain_warnings();
        assert!(second_drain.is_empty());
    }

    #[test]
    fn drain_warnings_entry_fields_are_accessible() {
        let collector = WarningCollector::default();
        collector.push_warning("SLOW_QUERY", "query took 500ms".to_string());
        let warnings = collector.drain_warnings();
        // Both fields must be public and carry the values we set.
        let entry = &warnings[0];
        assert_eq!(entry.code, "SLOW_QUERY");
        assert_eq!(entry.message, "query took 500ms");
    }

    #[test]
    fn two_instances_do_not_share_state() {
        let collector_a = WarningCollector::default();
        let collector_b = WarningCollector::default();
        collector_a.push_warning("ONLY_IN_A", "pushed to a".to_string());
        // b must be unaffected by a's push
        let b_warnings = collector_b.drain_warnings();
        assert!(b_warnings.is_empty(), "collector_b should not see collector_a's warnings");
        // a must still hold its own warning
        let a_warnings = collector_a.drain_warnings();
        assert_eq!(a_warnings.len(), 1);
        assert_eq!(a_warnings[0].code, "ONLY_IN_A");
    }
}
