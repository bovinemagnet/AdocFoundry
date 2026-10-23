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
pub use component::ComponentDescriptor;
pub use descriptor::{
    parse_component_descriptor, read_component_descriptor, DescriptorError, DescriptorErrorKind,
};
pub use discovery::{discover_antora_workspace, DiscoveryResult};
pub use module::Module;
pub use nav::{parse_nav, NavEntry};
pub use nav_tree::{nav_tree, NavError, NavItem, NavTree};
pub use playbook::{parse_playbook, read_playbook, Playbook, PLAYBOOK_FILE};
pub use resource::{AntoraCoordinate, AntoraResource, ResourceFamily};
pub use resource_id::{
    parse_resource_id, AntoraContext, AntoraResolver, AntoraResourceId, ResolutionError,
    ResolutionResult, ResourceIdParseError,
};
