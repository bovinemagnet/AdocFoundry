use std::{collections::BTreeMap, fmt, path::PathBuf, sync::Arc};

use adoc_core::SourceRange;

use adoc_lang::Location;

use crate::{DocumentId, EditPreview, Engine, EngineError, SearchRequest};

/// The stable identifier of a command, such as `document.save`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommandId(&'static str);

impl CommandId {
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandCategory {
    Document,
    Edit,
    Navigate,
    Workspace,
    View,
    Help,
}

/// When a command is available to the user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandContext {
    Always,
    ActiveDocument,
    /// A tab is active, a document's or an image's: closing and keeping tabs.
    ActiveTab,
    /// The active tab is an image.
    ActiveImage,
    /// A selection in the visual editor: the active document is shown visually or split.
    Selection,
    Antora,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub id: CommandId,
    /// Shown in the palette; English, title case.
    pub title: &'static str,
    pub category: CommandCategory,
    /// A `cmd-s` style binding, mapped to the platform by the front end.
    pub default_binding: Option<&'static str>,
    pub when: CommandContext,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CommandArgs {
    None,
    Document(DocumentId),
    /// An open image, for the commands that act on tabs.
    Image(crate::ImageId),
    Offset {
        id: DocumentId,
        offset: usize,
    },
    Range {
        id: DocumentId,
        range: SourceRange,
    },
    Text {
        id: DocumentId,
        range: SourceRange,
        text: String,
    },
    Path(PathBuf),
    /// A place in a file, which need not be open.
    Position(crate::NavigationPosition),
    Search(SearchRequest),
    Rename {
        id: DocumentId,
        new_path: PathBuf,
    },
    /// A visual editor command: a selection in a document's visual model.
    Visual(crate::VisualArgs),
    /// A PDF export: what to export and where to write it.
    Export(crate::ExportArgs),
    /// A publication profile, by name: `[pdf.profiles.<name>]`.
    Profile(String),
    Custom(serde_json::Value),
    /// A sidebar tree, for the commands that open and close its branches.
    Panel(crate::SidebarPanel),
    /// What to look for, across the workspace or in one document.
    Query {
        query: String,
        document: Option<DocumentId>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum CommandOutcome {
    Done,
    Opened(DocumentId),
    /// An image opened, or activated, in an image tab.
    OpenedImage(crate::ImageId),
    Location(Location),
    External(PathBuf),
    Preview(EditPreview),
    /// The references to an anchor or a page, in file order.
    References(Vec<crate::SearchMatch>),
    /// Sections, anchors and page titles to go to.
    Symbols(Vec<crate::SymbolMatch>),
    /// The selection after a visual command, as source offsets.
    Selection(SourceRange),
    /// A web address for the front end to open or copy.
    Url(String),
    /// The front end is to read the releases feed at this address and hand it to
    /// `Engine::record_update_check`: the engine makes no request itself.
    CheckForUpdates(String),
    /// What the doctor report needs from the engine, for the front end to complete with
    /// `Doctor::run`, off its main thread, and show.
    Diagnostics(Box<crate::Doctor>),
    /// Help › About, for the front end to name itself in and give its notices to
    /// (`About::program`, `About::notices`), and show.
    About(Box<crate::About>),
    /// A commit was made; these open documents were saved first.
    Committed {
        commit: String,
        saved: Vec<PathBuf>,
    },
    /// A commit for the front end to offer: the message to start from and the paths staged.
    CommitDraft {
        message: String,
        paths: Vec<PathBuf>,
    },
    /// Unsaved text that sessions which have ended left behind, for the front end to offer.
    Recoverable(Vec<crate::Recoverable>),
    /// A page's saved versions, newest first, for the front end to compare and restore.
    LocalHistory {
        document: DocumentId,
        versions: Vec<crate::SavedVersion>,
    },
    Cancelled,
}

impl CommandOutcome {
    /// The outcome of opening `item`.
    #[must_use]
    pub fn opened(item: crate::TabItem) -> Self {
        match item {
            crate::TabItem::Document(id) => Self::Opened(id),
            crate::TabItem::Image(id) => Self::OpenedImage(id),
        }
    }
}

pub trait Command: Send + Sync {
    fn spec(&self) -> &CommandSpec;
    fn run(&self, engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryError {
    Duplicate(CommandId),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(id) => write!(formatter, "command `{id}` is already registered"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Every command the engine can run, keyed by id.
#[derive(Clone, Default)]
pub struct CommandRegistry {
    commands: BTreeMap<CommandId, Arc<dyn Command>>,
}

impl CommandRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, command: Box<dyn Command>) -> Result<(), RegistryError> {
        let id = command.spec().id;
        if self.commands.contains_key(&id) {
            return Err(RegistryError::Duplicate(id));
        }
        self.commands.insert(id, Arc::from(command));
        Ok(())
    }

    #[must_use]
    pub fn get(&self, id: CommandId) -> Option<&dyn Command> {
        self.commands.get(&id).map(AsRef::as_ref)
    }

    pub(crate) fn shared(&self, id: CommandId) -> Option<Arc<dyn Command>> {
        self.commands.get(&id).cloned()
    }

    pub fn all(&self) -> impl Iterator<Item = &CommandSpec> {
        self.commands.values().map(|command| command.spec())
    }
}

impl fmt::Debug for CommandRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.commands.keys())
            .finish()
    }
}

/// A command whose behaviour is a plain function, which is how the built-in commands are defined.
pub(crate) struct FnCommand {
    pub(crate) spec: CommandSpec,
    pub(crate) run: fn(&mut Engine, CommandArgs) -> Result<CommandOutcome, EngineError>,
}

impl Command for FnCommand {
    fn spec(&self) -> &CommandSpec {
        &self.spec
    }

    fn run(&self, engine: &mut Engine, args: CommandArgs) -> Result<CommandOutcome, EngineError> {
        (self.run)(engine, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop(_: &mut Engine, _: CommandArgs) -> Result<CommandOutcome, EngineError> {
        Ok(CommandOutcome::Done)
    }

    fn command(id: &'static str) -> Box<dyn Command> {
        Box::new(FnCommand {
            spec: CommandSpec {
                id: CommandId::new(id),
                title: "Test",
                category: CommandCategory::Help,
                default_binding: None,
                when: CommandContext::Always,
            },
            run: noop,
        })
    }

    #[test]
    fn rejects_duplicate_ids() {
        let mut registry = CommandRegistry::new();
        registry.register(command("test.one")).unwrap();
        assert_eq!(
            registry.register(command("test.one")),
            Err(RegistryError::Duplicate(CommandId::new("test.one")))
        );
        assert!(registry.get(CommandId::new("test.one")).is_some());
        assert_eq!(registry.all().count(), 1);
    }
}
