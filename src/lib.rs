mod catalog;
mod component;
mod descriptor;
mod discovery;
mod module;
mod resource;
mod resource_id;

pub use catalog::AntoraCatalog;
pub use component::ComponentDescriptor;
pub use descriptor::{
    parse_component_descriptor, read_component_descriptor, DescriptorError, DescriptorErrorKind,
};
pub use discovery::{discover_antora_workspace, DiscoveryResult};
pub use module::Module;
pub use resource::{AntoraCoordinate, AntoraResource, ResourceFamily};
pub use resource_id::{
    parse_resource_id, AntoraContext, AntoraResolver, AntoraResourceId, ResolutionError,
    ResolutionResult, ResourceIdParseError,
};
