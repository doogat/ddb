mod data_doogat;
mod schema_parse;
mod typedef;
mod update_apply;

pub(crate) use data_doogat::{build_data_doogat, resolve_insert_title};
pub(super) use data_doogat::recompute_template_title;
pub use schema_parse::schema_from_parsed;
pub use typedef::build_typedef_doogat;
pub(crate) use update_apply::apply_updates_to_doogat;
pub(super) use update_apply::rename_key_in_doogat;
