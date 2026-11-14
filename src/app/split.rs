//! Binary split layout. Leaves reference views; documents remain shared.

use crate::app::view::ViewId;
use crate::term::renderer::Rect;

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Debug)]
pub enum SplitNode {
    Leaf(ViewId),
    Split {
        axis: Axis,
        ratio: f64,
        first: Box<SplitNode>,
        second: Box<SplitNode>,
    },
}

impl SplitNode {
    pub fn leaf(id: ViewId) -> Self {
        Self::Leaf(id)
    }

    /// Balanced tree for restoring a small number of independent views.
    pub fn from_leaves(ids: &[ViewId]) -> Self {
        fn build(ids: &[ViewId], depth: usize) -> SplitNode {
            match ids {
                [] => SplitNode::leaf(0),
                [id] => SplitNode::leaf(*id),
                _ => {
                    let mid = ids.len().div_ceil(2);
                    let axis = if depth.is_multiple_of(2) {
                        Axis::Vertical
                    } else {
                        Axis::Horizontal
                    };
                    SplitNode::Split {
                        axis,
                        ratio: 0.5,
                        first: Box::new(build(&ids[..mid], depth + 1)),
                        second: Box::new(build(&ids[mid..], depth + 1)),
                    }
                }
            }
        }
        build(ids, 0)
    }

    pub fn leaves(&self) -> Vec<ViewId> {
        match self {
            Self::Leaf(id) => vec![*id],
            Self::Split { first, second, .. } => {
                let mut out = first.leaves();
                out.extend(second.leaves());
                out
            }
        }
    }

    pub fn split(&mut self, target: ViewId, axis: Axis, new_view: ViewId) -> bool {
        match self {
            Self::Leaf(id) if *id == target => {
                let old = *id;
                *self = Self::Split {
                    axis,
                    ratio: 0.5,
                    first: Box::new(Self::Leaf(old)),
                    second: Box::new(Self::Leaf(new_view)),
                };
                true
            }
            Self::Split { first, second, .. } => {
                first.split(target, axis, new_view) || second.split(target, axis, new_view)
            }
            _ => false,
        }
    }

    /// Close a leaf, returning the view that should receive focus.
    pub fn close(&mut self, target: ViewId) -> Option<(bool, ViewId)> {
        match self {
            Self::Leaf(id) => {
                if *id == target {
                    Some((true, *id))
                } else {
                    None
                }
            }
            Self::Split { first, second, .. } => {
                if matches!(&**first, Self::Leaf(id) if *id == target) {
                    let replacement = (**second).clone();
                    *self = replacement;
                    return Some((false, self.leaves().first().copied().unwrap_or(target)));
                }
                if matches!(&**second, Self::Leaf(id) if *id == target) {
                    let replacement = (**first).clone();
                    *self = replacement;
                    return Some((false, self.leaves().first().copied().unwrap_or(target)));
                }
                if let Some((last, focus)) = first.close(target) {
                    return Some((last, focus));
                }
                if let Some((last, focus)) = second.close(target) {
                    return Some((last, focus));
                }
                None
            }
        }
    }

    pub fn equalize(&mut self) {
        if let Self::Split {
            ratio,
            first,
            second,
            ..
        } = self
        {
            *ratio = 0.5;
            first.equalize();
            second.equalize();
        }
    }

    pub fn rectangles(&self, rect: Rect) -> Vec<(ViewId, Rect)> {
        let mut out = Vec::new();
        self.collect(rect, &mut out);
        out
    }

    fn collect(&self, rect: Rect, out: &mut Vec<(ViewId, Rect)>) {
        match self {
            Self::Leaf(id) => out.push((*id, rect)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
                ..
            } => {
                if rect.width < 2 || rect.height < 2 {
                    out.push((self.leaves().first().copied().unwrap_or(0), rect));
                    return;
                }
                let ratio = ratio.clamp(0.2, 0.8);
                match axis {
                    Axis::Vertical => {
                        let left = (rect.width as f64 * ratio).round().max(1.0) as usize;
                        let left = left.min(rect.width.saturating_sub(1));
                        first.collect(
                            Rect {
                                x: rect.x,
                                y: rect.y,
                                width: left,
                                height: rect.height,
                            },
                            out,
                        );
                        second.collect(
                            Rect {
                                x: rect.x + left,
                                y: rect.y,
                                width: rect.width - left,
                                height: rect.height,
                            },
                            out,
                        );
                    }
                    Axis::Horizontal => {
                        let top = (rect.height as f64 * ratio).round().max(1.0) as usize;
                        let top = top.min(rect.height.saturating_sub(1));
                        first.collect(
                            Rect {
                                x: rect.x,
                                y: rect.y,
                                width: rect.width,
                                height: top,
                            },
                            out,
                        );
                        second.collect(
                            Rect {
                                x: rect.x,
                                y: rect.y + top,
                                width: rect.width,
                                height: rect.height - top,
                            },
                            out,
                        );
                    }
                }
            }
        }
    }
}

pub fn direction_matches(focus: &Rect, candidate: &Rect, right: bool, down: bool) -> bool {
    let fcx = focus.x * 2 + focus.width;
    let fcy = focus.y * 2 + focus.height;
    let ccx = candidate.x * 2 + candidate.width;
    let ccy = candidate.y * 2 + candidate.height;
    if right {
        ccx > fcx && (ccx - fcx) >= (ccy.abs_diff(fcy))
    } else if !right && !down {
        ccx < fcx && (fcx - ccx) >= (ccy.abs_diff(fcy))
    } else if down {
        ccy > fcy && (ccy - fcy) >= (ccx.abs_diff(fcx))
    } else {
        ccy < fcy && (fcy - ccy) >= (ccx.abs_diff(fcx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_leaves_are_preserved() {
        assert_eq!(SplitNode::from_leaves(&[1, 2, 3]).leaves(), vec![1, 2, 3]);
    }

    #[test]
    fn split_and_close_preserve_remaining_leaf() {
        let mut root = SplitNode::leaf(1);
        assert!(root.split(1, Axis::Vertical, 2));
        assert_eq!(root.leaves(), vec![1, 2]);
        let rects = root.rectangles(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        });
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].1.width, 40);
        assert_eq!(root.close(2), Some((false, 1)));
        assert_eq!(root.leaves(), vec![1]);
    }

    #[test]
    fn small_terminal_does_not_create_invisible_splits() {
        let root = SplitNode::Split {
            axis: Axis::Vertical,
            ratio: 0.5,
            first: Box::new(SplitNode::leaf(1)),
            second: Box::new(SplitNode::leaf(2)),
        };
        let rects = root.rectangles(Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 5,
        });
        assert_eq!(rects.len(), 1);
    }

    #[test]
    fn focus_direction_picks_the_nearest_axis() {
        let focus = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        let right = Rect {
            x: 40,
            y: 0,
            width: 40,
            height: 10,
        };
        let below = Rect {
            x: 0,
            y: 10,
            width: 40,
            height: 10,
        };
        assert!(direction_matches(&focus, &right, true, false));
        assert!(!direction_matches(&focus, &below, true, false));
        assert!(direction_matches(&focus, &below, false, true));
    }
}
