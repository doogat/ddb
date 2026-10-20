#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_FLAGS: [&str; 6] = [
        "typed_orderby_base_fields",
        "relation_where_filters",
        "inline_zone_ddl",
        "schema_apply",
        "offline_sdl",
        "capabilities_handshake",
    ];

    fn expected_owned() -> Vec<String> {
        EXPECTED_FLAGS.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flags_pin_exact_initial_set_in_stable_order() {
        assert_eq!(FLAGS, &EXPECTED_FLAGS[..]);
    }

    #[test]
    fn flags_first_three_cover_orderby_relation_and_inline_zone_probes() {
        assert_eq!(
            &FLAGS[..3],
            &[
                "typed_orderby_base_fields",
                "relation_where_filters",
                "inline_zone_ddl",
            ][..]
        );
    }

    #[test]
    fn flags_reject_near_miss_names() {
        for near_miss in ["schema", "schema_apply_v2", "offline", "orderby", ""] {
            assert!(
                !FLAGS.contains(&near_miss),
                "unexpected flag {near_miss:?} in {FLAGS:?}"
            );
        }
    }

    #[test]
    fn capabilities_returns_all_flags_as_owned_strings_in_order() {
        let caps: Vec<String> = capabilities();
        assert_eq!(caps, expected_owned());
    }

    #[test]
    fn capabilities_agrees_with_flags_registry() {
        let caps = capabilities();
        assert_eq!(caps.len(), FLAGS.len());
        for (cap, flag) in caps.iter().zip(FLAGS.iter()) {
            assert_eq!(cap.as_str(), *flag);
        }
    }

    #[test]
    fn capabilities_returns_fresh_vec_on_each_call() {
        let mut first = capabilities();
        first.clear();
        first.push("tampered".to_string());
        for call in 0..20 {
            assert_eq!(capabilities(), expected_owned(), "call {call}");
        }
    }

    #[test]
    fn version_reports_crate_package_version() {
        let v = version();
        assert!(!v.is_empty());
        assert_eq!(v, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn version_is_compile_time_constant_not_runtime_lookup() {
        // A shipped binary has no CARGO_PKG_VERSION in its environment;
        // only `cargo test` sets it. Remove it so a runtime lookup degrades.
        std::env::remove_var("CARGO_PKG_VERSION");
        let first = version();
        let second = version();
        assert!(!first.is_empty());
        assert_eq!(first, env!("CARGO_PKG_VERSION"));
        assert!(std::ptr::eq(first, second));
    }
}
