NO FINDINGS

The MEDIUM finding (dropped GraphQL coverage) is resolved: the delta adds `ServerGuard::start`, an `executeSql` CREATE TABLE ... SINGLETON DEFAULT VALUES, and three `select_scalar` checks (count/theme/schema_version), matching `select_scalar`'s real signature in `common.rs:365`. CLI assertions stay alongside it. No new issues in the delta.
