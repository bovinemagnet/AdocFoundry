use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Module {
    pub component: String,
    pub version: Option<String>,
    pub name: String,
    pub root: PathBuf,
    pub nav: Option<PathBuf>,
}
