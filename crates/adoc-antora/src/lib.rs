mod catalog;
mod component;
mod descriptor;
mod discovery;
mod module;
mod nav;
mod nav_tree;
mod playbook;
mod resource;
mod resource_id;

pub use catalog::{
    compare_component_versions, compare_versions, AntoraCatalog, ReloadReport, Resolved,
};
pub use component::{AttributeEntry, ComponentDescriptor};
pub use descriptor::{
    check_component_descriptor, parse_component_descriptor, read_component_descriptor,
    DescriptorError, DescriptorErrorKind,
};
pub use discovery::{
    discover_antora_files, discover_antora_workspace, discover_antora_workspace_ignoring,
    page_aliases, DiscoveryResult,
};
pub use module::Module;
pub use nav::{parse_nav, parse_nav_and_includes, NavEntry};
pub use nav_tree::{
    nav_tree, nav_tree_each, nav_trees_of, NavError, NavItem, NavTree, Unfollowed, UnfollowedKind,
};
pub use playbook::{
    parse_playbook, read_playbook, BranchVersions, ContentSource, Playbook, PublishedVersions,
    PLAYBOOK_FILE,
};
pub use resource::{AntoraCoordinate, AntoraResource, ResourceFamily};
pub use resource_id::{
    parse_resource_id, AntoraContext, AntoraResolver, AntoraResourceId, ResolutionError,
    ResolutionResult, ResourceIdParseError,
};
