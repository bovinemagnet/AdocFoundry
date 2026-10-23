//! Scroll sync: where each block lies in the preview's content, recorded as it is laid out, so
//! the caret can scroll the preview to its block and a scrolled preview can name its topmost
//! block.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{canvas, AnyElement, IntoElement as _, Pixels, ScrollHandle, Styled as _};

/// A block's vertical extent in the scrolled content, independent of the scroll position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub top: Pixels,
    pub bottom: Pixels,
}

/// Block extents by trail, refreshed on every frame.
pub type Extents = Rc<RefCell<HashMap<Vec<usize>, Extent>>>;

/// An invisible element filling its block that records the block's extent when laid out. The
/// block must be positioned relatively.
pub fn recorder(trail: Vec<usize>, extents: &Extents, scroll: &ScrollHandle) -> AnyElement {
    let extents = extents.clone();
    let scroll = scroll.clone();
    canvas(
        move |bounds, _, _| {
            let container = scroll.bounds();
            let offset = scroll.offset();
            let top = bounds.origin.y - container.origin.y - offset.y;
            extents.borrow_mut().insert(
                trail.clone(),
                Extent {
                    top,
                    bottom: top + bounds.size.height,
                },
            );
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// The innermost block containing the content position `y`, or else the first block below it.
#[must_use]
pub fn block_at_y(extents: &HashMap<Vec<usize>, Extent>, y: Pixels) -> Option<Vec<usize>> {
    let containing = extents
        .iter()
        .filter(|(_, extent)| extent.top <= y && y < extent.bottom)
        .max_by(|(left, _), (right, _)| left.len().cmp(&right.len()).then(right.cmp(left)))
        .map(|(trail, _)| trail.clone());
    containing.or_else(|| {
        extents
            .iter()
            .filter(|(_, extent)| extent.top >= y)
            .min_by(|(left_trail, left), (right_trail, right)| {
                left.top
                    .partial_cmp(&right.top)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(right_trail.len().cmp(&left_trail.len()))
            })
            .map(|(trail, _)| trail.clone())
    })
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;

    #[test]
    fn finds_the_innermost_block_at_a_position() {
        let mut extents = HashMap::new();
        let extent = |top: f32, bottom: f32| Extent {
            top: px(top),
            bottom: px(bottom),
        };
        extents.insert(vec![0], extent(0., 100.));
        extents.insert(vec![0, 0], extent(10., 40.));
        extents.insert(vec![0, 1], extent(50., 90.));
        extents.insert(vec![1], extent(120., 200.));

        assert_eq!(block_at_y(&extents, px(20.)), Some(vec![0, 0]));
        assert_eq!(block_at_y(&extents, px(45.)), Some(vec![0]));
        assert_eq!(
            block_at_y(&extents, px(110.)),
            Some(vec![1]),
            "the gap goes to the next"
        );
        assert_eq!(block_at_y(&extents, px(300.)), None);
    }
}
