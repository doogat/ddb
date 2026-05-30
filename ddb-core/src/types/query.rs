/// Adapter-neutral query parameter value used by foundation, service, and
/// transport code. Adapter-specific conversion belongs at adapter boundaries.
///
/// Exactly four variants cover the current query-parameter domain. `Bool` is
/// absent because GraphQL booleans normalize to `Integer(0|1)` at the filter
/// boundary (`ddb-server/src/filter.rs` `gql_to_sql`) before reaching this
/// type. `Blob` is absent because no current query-parameter path constructs
/// blob values; adding either variant would be dead surface until a concrete
/// use-case arrives.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_set_matches_sql_parameter_domain() {
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
}
