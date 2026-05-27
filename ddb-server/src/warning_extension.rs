use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub struct WarningEntry {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct WarningCollector {
    warnings: Mutex<Vec<WarningEntry>>,
}

impl WarningCollector {
    pub fn push_warning(&self, code: &'static str, message: String) {
        self.warnings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(WarningEntry { code, message });
    }

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
    fn drain_warnings_returns_entries_in_order_then_clears() {
        let collector = WarningCollector::default();
        collector.push_warning("CODE_A", "msg a".to_string());
        collector.push_warning("CODE_B", "msg b".to_string());

        let first_drain = collector.drain_warnings();
        assert_eq!(
            first_drain,
            vec![
                WarningEntry {
                    code: "CODE_A",
                    message: "msg a".to_string(),
                },
                WarningEntry {
                    code: "CODE_B",
                    message: "msg b".to_string(),
                },
            ]
        );
        assert!(collector.drain_warnings().is_empty());
    }

    #[test]
    fn two_instances_do_not_share_state() {
        let collector_a = WarningCollector::default();
        let collector_b = WarningCollector::default();
        collector_a.push_warning("ONLY_IN_A", "pushed to a".to_string());

        let b_warnings = collector_b.drain_warnings();
        assert!(
            b_warnings.is_empty(),
            "collector_b should not see collector_a's warnings"
        );

        let a_warnings = collector_a.drain_warnings();
        assert_eq!(
            a_warnings,
            vec![WarningEntry {
                code: "ONLY_IN_A",
                message: "pushed to a".to_string(),
            }]
        );
    }
}
