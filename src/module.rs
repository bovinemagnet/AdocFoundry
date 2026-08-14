use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Module {
    pub component: String,
    pub version: String,
    pub name: String,
    pub root: PathBuf,
}
