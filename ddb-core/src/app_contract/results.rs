//! Result types for the application contract layer.

#[derive(Debug, Clone)]
pub struct BrokenBacklink {
    pub source_id: String,
    pub source_path: String,
}

#[derive(Debug, Clone)]
pub struct DeleteResult {
    pub broken_backlinks: Vec<BrokenBacklink>,
}
