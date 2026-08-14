use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentDescriptor {
    pub name: String,
    pub title: Option<String>,
    pub version: String,
    pub root: PathBuf,
    pub nav: Vec<PathBuf>,
}
