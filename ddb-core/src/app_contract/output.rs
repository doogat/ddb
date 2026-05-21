#[derive(Debug, Clone)]
pub struct AppWarning {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct AppOutput<T> {
    pub value: T,
    pub warnings: Vec<AppWarning>,
}
