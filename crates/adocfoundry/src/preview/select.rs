//! Selecting rendered text (#794): the text drawn from the view tree, recorded as it is laid
//! out so that a pointer finds a place in it, with the selection painted over it. The preview
//! and the visual editor's read-only content share it.
//!
//! What is laid out only says where a place is on screen. What a selection covers is worked out
//! by the engine from the view tree, so it can reach beyond what is drawn.

use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

use adoc_render::fragment::{Flow, LeafKey, Place, OBJECT};
use gpui::{
    fill, px, size, AnyElement, App, Bounds, CursorStyle, Element, ElementId, GlobalElementId,
    Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point,
    SharedString, StyledText, TextLayout, Window,
};

/// How long a click on read-only text waits for a second before it shows its source, which
/// moves what is drawn from under the pointer.
pub(crate) const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(300);

/// Text drawn this frame: part of a leaf, with its layout.
#[derive(Clone)]
pub(crate) struct Piece {
    pub key: LeafKey,
    /// Where it starts in the leaf's text.
    start: usize,
    /// The text laid out, with a gap for each typeset formula.
    text: SharedString,
    gaps: Vec<Range<usize>>,
    layout: TextLayout,
}

impl Piece {
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.layout.bounds()
    }

    /// The leaf offset nearest `position`.
    pub fn offset_at(&self, position: Point<Pixels>) -> usize {
        let mut index = match self.layout.index_for_position(position) {
            // The nearer side of the character under the pointer.
            Ok(index) => self.nearer(index, position),
            Err(index) => index,
        };
        index = index.min(self.text.len());
        self.start + self.leaf_offset(index)
    }

    /// The side of the character at `index` that is nearer `position`.
    fn nearer(&self, index: usize, position: Point<Pixels>) -> usize {
        let Some(character) = self.text.get(index..).and_then(|rest| rest.chars().next()) else {
            return index;
        };
        let next = index + character.len_utf8();
        let sides = self
            .layout
            .position_for_index(index)
            .zip(self.layout.position_for_index(next));
        match sides {
            Some((left, right))
                if left.y == right.y && position.x - left.x > right.x - position.x =>
            {
                next
            }
            _ => index,
        }
    }

    /// The top of the line `offset` of the leaf is on, where it is, and the line's height.
    pub fn point_of(&self, offset: usize) -> Option<(Point<Pixels>, Pixels)> {
        let shown = self.shown_index(offset.checked_sub(self.start)?);
        let position = self
            .layout
            .position_for_index(shown.min(self.text.len().max(1)))?;
        Some((position, self.layout.line_height()))
    }

    /// Whether `offset` of the leaf is in this piece, or at its end.
    pub fn holds(&self, offset: usize) -> bool {
        offset >= self.start && offset - self.start <= self.leaf_offset(self.text.len())
    }

    /// Where `index` of the text laid out is in the leaf's part: a gap stands for one object.
    fn leaf_offset(&self, index: usize) -> usize {
        let mut taken = 0;
        for gap in &self.gaps {
            if index <= gap.start {
                break;
            }
            if index < gap.end {
                // Inside a formula: the nearer end of it.
                let after = index - gap.start > gap.len() / 2;
                let before = gap.start - taken;
                return before + if after { OBJECT.len_utf8() } else { 0 };
            }
            taken += gap.len();
            taken -= OBJECT.len_utf8();
        }
        index - taken
    }

    /// Where `offset` of the leaf's part is in the text laid out.
    fn shown_index(&self, offset: usize) -> usize {
        let mut added = 0;
        for gap in &self.gaps {
            if offset + added <= gap.start {
                break;
            }
            added += gap.len();
            added -= OBJECT.len_utf8();
        }
        offset + added
    }
}

/// Text a view draws from the view tree, and the pointer's part in selecting it.
#[derive(Clone, Default)]
pub(crate) struct Layer(Rc<Drawn>);

#[derive(Default)]
struct Drawn {
    pieces: RefCell<Vec<Piece>>,
    /// The pointer selected something since it was last pressed, so its release is not a click.
    dragged: Cell<bool>,
}

impl Layer {
    /// Forgets what the last frame drew.
    pub fn clear(&self) {
        self.0.pieces.borrow_mut().clear();
    }

    pub fn set_dragged(&self, dragged: bool) {
        self.0.dragged.set(dragged);
    }

    pub fn dragged(&self) -> bool {
        self.0.dragged.get()
    }

    /// The piece under `position`.
    pub fn at(&self, position: Point<Pixels>) -> Option<Piece> {
        self.0
            .pieces
            .borrow()
            .iter()
            .rev()
            .find(|piece| piece.bounds().contains(&position))
            .cloned()
    }

    /// The piece that holds `offset` of `key`, if it is drawn.
    pub fn holding(&self, key: &LeafKey, offset: usize) -> Option<Piece> {
        self.0
            .pieces
            .borrow()
            .iter()
            .find(|piece| piece.key == *key && piece.holds(offset))
            .cloned()
    }

    /// Every piece drawn.
    pub fn pieces(&self) -> Vec<Piece> {
        self.0.pieces.borrow().clone()
    }

    fn record(&self, piece: Piece) {
        self.0.pieces.borrow_mut().push(piece);
    }
}

/// How far `position` is from `bounds`, down and then across: zero inside them.
pub(crate) fn distance(bounds: &Bounds<Pixels>, position: Point<Pixels>) -> (Pixels, Pixels) {
    let beyond = |value: Pixels, low: Pixels, high: Pixels| {
        if value < low {
            low - value
        } else if value > high {
            value - high
        } else {
            px(0.)
        }
    };
    (
        beyond(position.y, bounds.top(), bounds.bottom()),
        beyond(position.x, bounds.left(), bounds.right()),
    )
}

/// The piece of `pieces` nearest `position`.
pub(crate) fn nearest(pieces: &[Piece], position: Point<Pixels>) -> Option<&Piece> {
    pieces.iter().min_by(|one, other| {
        distance(&one.bounds(), position)
            .partial_cmp(&distance(&other.bounds(), position))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// The part of each leaf that is selected.
#[derive(Clone)]
pub(crate) struct Selected(Rc<RangeOf>);

type RangeOf = dyn Fn(&LeafKey) -> Option<Range<usize>>;

impl Selected {
    /// What `range` says of each leaf.
    pub fn new(range: impl Fn(&LeafKey) -> Option<Range<usize>> + 'static) -> Self {
        Self(Rc::new(range))
    }

    /// What lies between two places of `flow`.
    pub fn between(flow: Rc<Flow>, from: Place, to: Place) -> Self {
        Self::new(move |key| flow.selected(flow.position(key)?, from, to))
    }
}

/// What drawn text needs to be selected.
#[derive(Clone)]
pub(crate) struct Select {
    pub layer: Layer,
    pub selected: Option<Selected>,
    pub colour: Hsla,
}

/// The layout of a text and its formulas' gaps, filled in as the text is laid out.
pub(crate) type LaidOut = Rc<RefCell<Option<(TextLayout, SharedString, Vec<Range<usize>>)>>>;

/// `text` as text that can be selected: the part of the leaf `key` starting at `start`.
pub(crate) fn plain(
    text: impl Into<SharedString>,
    key: LeafKey,
    start: usize,
    select: Option<&Select>,
) -> AnyElement {
    let text = text.into();
    styled(StyledText::new(text.clone()), text, key, start, select)
}

/// `styled`, which draws `text`, as text that can be selected.
pub(crate) fn styled(
    styled: StyledText,
    text: SharedString,
    key: LeafKey,
    start: usize,
    select: Option<&Select>,
) -> AnyElement {
    let layout = styled.layout().clone();
    let laid_out: LaidOut = Rc::new(RefCell::new(Some((layout, text, Vec::new()))));
    selectable(styled.into_any_element(), laid_out, key, start, select)
}

/// `inner` as text that can be selected, `laid_out` by the time it is painted.
pub(crate) fn selectable(
    inner: AnyElement,
    laid_out: LaidOut,
    key: LeafKey,
    start: usize,
    select: Option<&Select>,
) -> AnyElement {
    match select {
        Some(select) => Selectable {
            inner,
            laid_out,
            key,
            start,
            select: select.clone(),
        }
        .into_any_element(),
        None => inner,
    }
}

/// Text drawn as its element draws it, recorded in the layer, with its selected part painted.
struct Selectable {
    inner: AnyElement,
    laid_out: LaidOut,
    key: LeafKey,
    start: usize,
    select: Select,
}

impl IntoElement for Selectable {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Selectable {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.inner.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.inner.prepaint(window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Before the text, whose links ask for a hand.
        window.set_cursor_style(CursorStyle::IBeam, hitbox);
        self.inner.paint(window, cx);
        let Some((layout, text, gaps)) = self.laid_out.borrow_mut().take() else {
            return;
        };
        let piece = Piece {
            key: self.key.clone(),
            start: self.start,
            text,
            gaps,
            layout,
        };
        let length = piece.leaf_offset(piece.text.len());
        let selected = self
            .select
            .selected
            .as_ref()
            .and_then(|selected| (selected.0)(&self.key))
            .map(|range| range.start.max(self.start)..range.end.min(self.start + length))
            .filter(|range| range.start < range.end);
        if let Some(range) = selected {
            let shown = piece.shown_index(range.start - self.start)
                ..piece.shown_index(range.end - self.start);
            for rect in rects(&piece.layout, shown, &piece.text) {
                window.paint_quad(fill(rect, self.select.colour));
            }
        }
        self.select.layer.record(piece);
    }
}

/// The rectangles covering `range` of laid-out `text`, one for each line.
pub(crate) fn rects(layout: &TextLayout, range: Range<usize>, text: &str) -> Vec<Bounds<Pixels>> {
    let line_height = layout.line_height();
    let mut rects: Vec<Bounds<Pixels>> = Vec::new();
    let mut boundaries: Vec<usize> = text
        .char_indices()
        .map(|(index, _)| index)
        .filter(|index| range.contains(index))
        .collect();
    boundaries.push(range.end.min(text.len()));
    for pair in boundaries.windows(2) {
        let (Some(from), Some(to)) = (
            layout.position_for_index(pair[0]),
            layout.position_for_index(pair[1]),
        ) else {
            continue;
        };
        // A character that ends a wrapped line is given a nominal width.
        let right = if to.y == from.y {
            to.x
        } else {
            from.x + line_height / 3.
        };
        let rect = Bounds::new(from, size(right - from.x, line_height));
        match rects.last_mut() {
            Some(last)
                if last.origin.y == rect.origin.y && last.right() >= rect.left() - px(1.) =>
            {
                last.size.width = rect.right() - last.left();
            }
            _ => rects.push(rect),
        }
    }
    rects
}
