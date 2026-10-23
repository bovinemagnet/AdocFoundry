//! The formatting toolbar: a block-type picker, marks, links, lists, and inserting blocks.
//! Every button runs the same engine command as its key binding and palette entry.

use adoc_engine::{commands::ids, BlockType, Mark, NodeKind, VisualValue};
use adoc_render::AdmonitionKind;
use gpui::{
    actions, div, Context, InteractiveElement as _, IntoElement, ParentElement as _, Stateful,
    Styled as _, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::DropdownMenu as _,
    ActiveTheme as _, Selectable as _, Sizable as _,
};

use super::VisualView;
use crate::editor::bindings::{
    DeleteTableColumn, DeleteTableRow, InsertCodeBlock, InsertImage, InsertTable,
    InsertTableColumn, InsertTableRow,
};

actions!(
    visual_block,
    [
        SetParagraph,
        SetHeading1,
        SetHeading2,
        SetHeading3,
        SetHeading4,
        SetHeading5,
        SetCode,
        SetQuote,
        SetNote,
        SetTip,
        SetImportant,
        SetCaution,
        SetWarning,
        InsertNote,
    ]
);

/// The block-type actions' handlers.
pub fn actions(view: Stateful<gpui::Div>, cx: &mut Context<VisualView>) -> Stateful<gpui::Div> {
    let set = |kind: BlockType| {
        move |this: &mut VisualView, window: &mut Window, cx: &mut Context<VisualView>| {
            this.run(
                ids::EDIT_SET_BLOCK_TYPE,
                VisualValue::BlockType(kind),
                window,
                cx,
            );
        }
    };
    let paragraph = set(BlockType::Paragraph);
    let heading = [1, 2, 3, 4, 5].map(|level| set(BlockType::Heading(level)));
    let code = set(BlockType::Code);
    let quote = set(BlockType::Quote);
    let admonitions = [
        AdmonitionKind::Note,
        AdmonitionKind::Tip,
        AdmonitionKind::Important,
        AdmonitionKind::Caution,
        AdmonitionKind::Warning,
    ]
    .map(|kind| set(BlockType::Admonition(kind)));
    let [h1, h2, h3, h4, h5] = heading;
    let [note, tip, important, caution, warning] = admonitions;
    view.on_action(
        cx.listener(move |this, _: &SetParagraph, window, cx| paragraph(this, window, cx)),
    )
    .on_action(cx.listener(move |this, _: &SetHeading1, window, cx| h1(this, window, cx)))
    .on_action(cx.listener(move |this, _: &SetHeading2, window, cx| h2(this, window, cx)))
    .on_action(cx.listener(move |this, _: &SetHeading3, window, cx| h3(this, window, cx)))
    .on_action(cx.listener(move |this, _: &SetHeading4, window, cx| h4(this, window, cx)))
    .on_action(cx.listener(move |this, _: &SetHeading5, window, cx| h5(this, window, cx)))
    .on_action(cx.listener(move |this, _: &SetCode, window, cx| code(this, window, cx)))
    .on_action(cx.listener(move |this, _: &SetQuote, window, cx| quote(this, window, cx)))
    .on_action(cx.listener(move |this, _: &SetNote, window, cx| note(this, window, cx)))
    .on_action(cx.listener(move |this, _: &SetTip, window, cx| tip(this, window, cx)))
    .on_action(cx.listener(move |this, _: &SetImportant, window, cx| important(this, window, cx)))
    .on_action(cx.listener(move |this, _: &SetCaution, window, cx| caution(this, window, cx)))
    .on_action(cx.listener(move |this, _: &SetWarning, window, cx| warning(this, window, cx)))
    .on_action(cx.listener(|this, _: &InsertNote, window, cx| {
        this.run(
            ids::EDIT_INSERT_ADMONITION,
            VisualValue::Admonition(AdmonitionKind::Note),
            window,
            cx,
        );
    }))
}

impl VisualView {
    /// What the picker shows for the caret's block, and whether the caret is in a table cell.
    fn caret_block(&self) -> (String, bool) {
        let Some(shown) = &self.shown else {
            return (String::new(), false);
        };
        let Some((path, node, _)) = shown.model.text_block_at(self.selection.head) else {
            return ("Not editable".to_owned(), false);
        };
        let parent = path.split_last().and_then(|(_, parent)| {
            (!parent.is_empty())
                .then(|| shown.model.node(parent))
                .flatten()
        });
        let label = match (&node.kind, parent.map(|parent| &parent.kind)) {
            (NodeKind::Paragraph, Some(NodeKind::Admonition { kind, .. })) => {
                kind.label().to_owned()
            }
            (NodeKind::Paragraph, Some(NodeKind::Quote)) => "Quote".to_owned(),
            (NodeKind::Paragraph, _) => "Paragraph".to_owned(),
            (NodeKind::Section { level } | NodeKind::Heading { level }, _) => {
                format!("Heading {level}")
            }
            (NodeKind::Title, _) => "Title".to_owned(),
            (NodeKind::ListItem { .. }, _) => "List item".to_owned(),
            (NodeKind::Code { .. }, _) => "Code".to_owned(),
            (NodeKind::Literal, _) => "Literal".to_owned(),
            (NodeKind::Cell, _) => "Table cell".to_owned(),
            (NodeKind::Term | NodeKind::Description, _) => "Description".to_owned(),
            _ => "Text".to_owned(),
        };
        (label, node.kind == NodeKind::Cell)
    }

    /// The marks the toolbar shows as on: the typing marks with nothing selected, else the
    /// marks every part of the selection has.
    fn active_marks(&self, mark: Mark) -> bool {
        if self.selection.is_empty() {
            return self.marks.contains(mark);
        }
        let Some((_, block)) = self.block_at(self.selection.head) else {
            return false;
        };
        let clip = |offset: usize| offset.clamp(block.range.start, block.range.end);
        let (Some(start), Some(end)) = (
            block.display_offset(clip(self.selection.start())),
            block.display_offset_towards(clip(self.selection.end()), true),
        ) else {
            return false;
        };
        start < end && block.has_mark(start..end, mark)
    }

    pub(crate) fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (block, in_cell) = self.caret_block();
        let focus = self.focus.clone();
        let mark_button =
            |id: &'static str, label: &'static str, mark: Mark, command, cx: &mut Context<Self>| {
                Button::new(id)
                    .ghost()
                    .xsmall()
                    .label(label)
                    .debug_selector(move || format!("toolbar-{id}"))
                    .selected(self.active_marks(mark))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        this.toggle_mark(command, mark, window, cx);
                    }))
            };
        let command_button =
            |id: &'static str, label: &'static str, command, cx: &mut Context<Self>| {
                Button::new(id)
                    .ghost()
                    .xsmall()
                    .label(label)
                    .debug_selector(move || format!("toolbar-{id}"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        this.run(command, VisualValue::None, window, cx);
                    }))
            };
        let block_focus = focus.clone();
        let picker = Button::new("block-type")
            .ghost()
            .xsmall()
            .label(format!("{block} ▾"))
            .dropdown_menu(move |menu, _, _| {
                menu.action_context(block_focus.clone())
                    .menu("Paragraph", Box::new(SetParagraph))
                    .menu("Heading 1", Box::new(SetHeading1))
                    .menu("Heading 2", Box::new(SetHeading2))
                    .menu("Heading 3", Box::new(SetHeading3))
                    .menu("Heading 4", Box::new(SetHeading4))
                    .menu("Heading 5", Box::new(SetHeading5))
                    .separator()
                    .menu("Code block", Box::new(SetCode))
                    .menu("Quote", Box::new(SetQuote))
                    .separator()
                    .menu("Note", Box::new(SetNote))
                    .menu("Tip", Box::new(SetTip))
                    .menu("Important", Box::new(SetImportant))
                    .menu("Caution", Box::new(SetCaution))
                    .menu("Warning", Box::new(SetWarning))
            });
        let insert_focus = focus.clone();
        let insert = Button::new("insert")
            .ghost()
            .xsmall()
            .label("Insert ▾")
            .dropdown_menu(move |menu, _, _| {
                menu.action_context(insert_focus.clone())
                    .menu("Image…", Box::new(InsertImage))
                    .menu("Table", Box::new(InsertTable))
                    .menu("Code block", Box::new(InsertCodeBlock))
                    .menu("Admonition", Box::new(InsertNote))
            });
        let table_focus = focus.clone();
        let table = in_cell.then(|| {
            Button::new("table")
                .ghost()
                .xsmall()
                .label("Table ▾")
                .dropdown_menu(move |menu, _, _| {
                    menu.action_context(table_focus.clone())
                        .menu("Insert row below", Box::new(InsertTableRow))
                        .menu("Delete row", Box::new(DeleteTableRow))
                        .separator()
                        .menu("Insert column after", Box::new(InsertTableColumn))
                        .menu("Delete column", Box::new(DeleteTableColumn))
                })
        });
        let border = cx.theme().border;
        let separator = move || div().w(gpui::px(1.)).h(gpui::px(16.)).bg(border);
        h_flex()
            .id("visual-toolbar")
            .flex_wrap()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(picker)
            .child(separator())
            .child(mark_button(
                "bold",
                "B",
                Mark::Strong,
                ids::EDIT_TOGGLE_STRONG,
                cx,
            ))
            .child(mark_button(
                "italic",
                "I",
                Mark::Emphasis,
                ids::EDIT_TOGGLE_EMPHASIS,
                cx,
            ))
            .child(mark_button(
                "code",
                "Code",
                Mark::Code,
                ids::EDIT_TOGGLE_CODE,
                cx,
            ))
            .child(mark_button(
                "highlight",
                "Mark",
                Mark::Highlight,
                ids::EDIT_TOGGLE_HIGHLIGHT,
                cx,
            ))
            .child(
                Button::new("link")
                    .ghost()
                    .xsmall()
                    .label("Link")
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        this.edit_link(window, cx);
                    })),
            )
            .child(separator())
            .child(command_button(
                "bullets",
                "• List",
                ids::EDIT_TOGGLE_BULLET_LIST,
                cx,
            ))
            .child(command_button(
                "numbers",
                "1. List",
                ids::EDIT_TOGGLE_NUMBERED_LIST,
                cx,
            ))
            .child(command_button(
                "checklist",
                "☑ List",
                ids::EDIT_TOGGLE_CHECKLIST,
                cx,
            ))
            .child(command_button(
                "outdent",
                "Outdent",
                ids::EDIT_OUTDENT_LIST_ITEM,
                cx,
            ))
            .child(command_button(
                "indent",
                "Indent",
                ids::EDIT_INDENT_LIST_ITEM,
                cx,
            ))
            .child(separator())
            .child(insert)
            .children(table)
    }
}
