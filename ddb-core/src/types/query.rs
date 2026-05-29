#[cfg(test)]
mod tests {
    use super::*;

    // The contract pins QueryValue to EXACTLY four variants: Null, Integer(i64),
    // Real(f64), Text(String). There is intentionally NO Bool and NO Blob variant.
    // The set is pinned by `variant_set_is_exactly_four_no_wildcard`: its exhaustive
    // match with no wildcard arm stops compiling if a variant is ADDED, and the
    // positive construction tests stop compiling if a variant is REMOVED. Together
    // they fix the variant set to exactly { Null, Integer, Real, Text }.

    #[test]
    fn variants_compare_equal_by_value() {
        assert_eq!(QueryValue::Null, QueryValue::Null);
        assert_eq!(QueryValue::Integer(3), QueryValue::Integer(3));
        assert_eq!(QueryValue::Real(1.5), QueryValue::Real(1.5));
        assert_eq!(
            QueryValue::Text("rust".to_string()),
            QueryValue::Text("rust".to_string())
        );
    }

    #[test]
    fn distinct_values_within_variant_are_not_equal() {
        assert_ne!(QueryValue::Integer(3), QueryValue::Integer(4));
        assert_ne!(QueryValue::Real(1.0), QueryValue::Real(2.0));
        assert_ne!(
            QueryValue::Text("a".to_string()),
            QueryValue::Text("b".to_string())
        );
    }

    #[test]
    fn distinct_variants_are_not_equal() {
        assert_ne!(QueryValue::Null, QueryValue::Text("x".to_string()));
        assert_ne!(QueryValue::Real(1.0), QueryValue::Integer(1));
        assert_ne!(QueryValue::Null, QueryValue::Integer(0));
        assert_ne!(
            QueryValue::Integer(1),
            QueryValue::Text("1".to_string())
        );
    }

    #[test]
    fn clone_preserves_value() {
        let a = QueryValue::Text("rust".into());
        assert_eq!(a.clone(), a);

        let n = QueryValue::Null;
        assert_eq!(n.clone(), n);

        let i = QueryValue::Integer(42);
        assert_eq!(i.clone(), i);

        let r = QueryValue::Real(2.71);
        assert_eq!(r.clone(), r);
    }

    #[test]
    fn variant_set_is_exactly_four_no_wildcard() {
        // Exhaustive match with no wildcard arm: if a variant is ADDED to
        // QueryValue, this stops compiling; if one is removed, the positive
        // construction tests stop compiling. Together they pin the set to
        // exactly { Null, Integer, Real, Text }.
        fn assert_exhaustive(v: QueryValue) {
            match v {
                QueryValue::Null => {}
                QueryValue::Integer(_) => {}
                QueryValue::Real(_) => {}
                QueryValue::Text(_) => {}
            }
        }
        assert_exhaustive(QueryValue::Null);
        assert_exhaustive(QueryValue::Integer(0));
        assert_exhaustive(QueryValue::Real(0.0));
        assert_exhaustive(QueryValue::Text(String::new()));
    }

    #[test]
    fn debug_formats_without_panicking() {
        assert!(!format!("{:?}", QueryValue::Real(1.5)).is_empty());
        assert!(!format!("{:?}", QueryValue::Null).is_empty());
        assert!(!format!("{:?}", QueryValue::Integer(7)).is_empty());
        assert!(!format!("{:?}", QueryValue::Text("x".into())).is_empty());
    }
}
