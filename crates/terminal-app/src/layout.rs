use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }

    fn center(self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaneTree {
    Leaf {
        pane: PaneId,
    },
    Split {
        axis: SplitAxis,
        ratio: f32,
        first: Box<PaneTree>,
        second: Box<PaneTree>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitHandle {
    path: Vec<bool>,
}

impl PaneTree {
    pub fn leaf(pane: PaneId) -> Self {
        Self::Leaf { pane }
    }

    pub fn panes(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        self.collect_panes(&mut panes);
        panes
    }

    fn collect_panes(&self, panes: &mut Vec<PaneId>) {
        match self {
            Self::Leaf { pane } => panes.push(*pane),
            Self::Split { first, second, .. } => {
                first.collect_panes(panes);
                second.collect_panes(panes);
            }
        }
    }

    pub fn contains(&self, target: PaneId) -> bool {
        match self {
            Self::Leaf { pane } => *pane == target,
            Self::Split { first, second, .. } => first.contains(target) || second.contains(target),
        }
    }

    pub fn split(&mut self, target: PaneId, new_pane: PaneId, axis: SplitAxis) -> bool {
        match self {
            Self::Leaf { pane } if *pane == target => {
                *self = Self::Split {
                    axis,
                    ratio: 0.5,
                    first: Box::new(Self::leaf(target)),
                    second: Box::new(Self::leaf(new_pane)),
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => {
                first.split(target, new_pane, axis) || second.split(target, new_pane, axis)
            }
        }
    }

    pub fn close(&mut self, target: PaneId) -> Option<PaneId> {
        let replacement = match self {
            Self::Leaf { .. } => return None,
            Self::Split { first, second, .. } if first.contains(target) => {
                if matches!(first.as_ref(), Self::Leaf { pane } if *pane == target) {
                    Some((**second).clone())
                } else {
                    let focus = first.close(target)?;
                    return Some(focus);
                }
            }
            Self::Split { first, second, .. } if second.contains(target) => {
                if matches!(second.as_ref(), Self::Leaf { pane } if *pane == target) {
                    Some((**first).clone())
                } else {
                    let focus = second.close(target)?;
                    return Some(focus);
                }
            }
            Self::Split { .. } => return None,
        };
        let replacement = replacement?;
        let focus = replacement.panes()[0];
        *self = replacement;
        Some(focus)
    }

    pub fn layout(&self, rect: Rect) -> Vec<(PaneId, Rect)> {
        let mut output = Vec::new();
        self.layout_into(rect, &mut output);
        output
    }

    pub fn visible_layout(&self, rect: Rect, zoomed: Option<PaneId>) -> Vec<(PaneId, Rect)> {
        match zoomed.filter(|pane| self.contains(*pane)) {
            Some(pane) => vec![(pane, rect)],
            None => self.layout(rect),
        }
    }

    pub fn split_handle_at(
        &self,
        x: f32,
        y: f32,
        rect: Rect,
        threshold: f32,
    ) -> Option<SplitHandle> {
        self.find_split_handle_at(x, y, rect, threshold)
            .map(|path| SplitHandle { path })
    }

    fn find_split_handle_at(
        &self,
        x: f32,
        y: f32,
        rect: Rect,
        threshold: f32,
    ) -> Option<Vec<bool>> {
        let Self::Split {
            axis,
            ratio,
            first,
            second,
        } = self
        else {
            return None;
        };

        let ratio = ratio.clamp(0.1, 0.9);

        let (first_rect, second_rect) = match axis {
            SplitAxis::Vertical => {
                let first_width = rect.width * ratio;

                (
                    Rect {
                        width: first_width,
                        ..rect
                    },
                    Rect {
                        x: rect.x + first_width,
                        width: rect.width - first_width,
                        ..rect
                    },
                )
            }

            SplitAxis::Horizontal => {
                let first_height = rect.height * ratio;

                (
                    Rect {
                        height: first_height,
                        ..rect
                    },
                    Rect {
                        y: rect.y + first_height,
                        height: rect.height - first_height,
                        ..rect
                    },
                )
            }
        };

        if first_rect.contains(x, y) {
            if let Some(mut path) = first.find_split_handle_at(x, y, first_rect, threshold) {
                path.insert(0, false);
                return Some(path);
            }
        }

        if second_rect.contains(x, y) {
            if let Some(mut path) = second.find_split_handle_at(x, y, second_rect, threshold) {
                path.insert(0, true);
                return Some(path);
            }
        }

        let on_divider = match axis {
            SplitAxis::Vertical => {
                let divider = rect.x + rect.width * ratio;

                (x - divider).abs() <= threshold && y >= rect.y && y < rect.y + rect.height
            }

            SplitAxis::Horizontal => {
                let divider = rect.y + rect.height * ratio;

                (y - divider).abs() <= threshold && x >= rect.x && x < rect.x + rect.width
            }
        };

        on_divider.then(Vec::new)
    }

    pub fn resize_split_by_pixels(
        &mut self,
        handle: &SplitHandle,
        rect: Rect,
        delta_x: f32,
        delta_y: f32,
    ) -> bool {
        self.resize_split_path(&handle.path, rect, delta_x, delta_y)
    }

    fn resize_split_path(&mut self, path: &[bool], rect: Rect, delta_x: f32, delta_y: f32) -> bool {
        let Self::Split {
            axis,
            ratio,
            first,
            second,
        } = self
        else {
            return false;
        };

        if path.is_empty() {
            let delta = match axis {
                SplitAxis::Vertical if rect.width > 0.0 => delta_x / rect.width,

                SplitAxis::Horizontal if rect.height > 0.0 => delta_y / rect.height,

                _ => 0.0,
            };

            if delta == 0.0 {
                return false;
            }

            let old_ratio = *ratio;
            *ratio = (*ratio + delta).clamp(0.1, 0.9);

            return (*ratio - old_ratio).abs() > f32::EPSILON;
        }

        let ratio_value = ratio.clamp(0.1, 0.9);

        let (first_rect, second_rect) = match axis {
            SplitAxis::Vertical => {
                let first_width = rect.width * ratio_value;

                (
                    Rect {
                        width: first_width,
                        ..rect
                    },
                    Rect {
                        x: rect.x + first_width,
                        width: rect.width - first_width,
                        ..rect
                    },
                )
            }

            SplitAxis::Horizontal => {
                let first_height = rect.height * ratio_value;

                (
                    Rect {
                        height: first_height,
                        ..rect
                    },
                    Rect {
                        y: rect.y + first_height,
                        height: rect.height - first_height,
                        ..rect
                    },
                )
            }
        };

        if path[0] {
            second.resize_split_path(&path[1..], second_rect, delta_x, delta_y)
        } else {
            first.resize_split_path(&path[1..], first_rect, delta_x, delta_y)
        }
    }

    fn layout_into(&self, rect: Rect, output: &mut Vec<(PaneId, Rect)>) {
        match self {
            Self::Leaf { pane } => output.push((*pane, rect)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let ratio = ratio.clamp(0.1, 0.9);
                let (first_rect, second_rect) = match axis {
                    SplitAxis::Vertical => {
                        let first_width = rect.width * ratio;
                        (
                            Rect {
                                width: first_width,
                                ..rect
                            },
                            Rect {
                                x: rect.x + first_width,
                                width: rect.width - first_width,
                                ..rect
                            },
                        )
                    }
                    SplitAxis::Horizontal => {
                        let first_height = rect.height * ratio;
                        (
                            Rect {
                                height: first_height,
                                ..rect
                            },
                            Rect {
                                y: rect.y + first_height,
                                height: rect.height - first_height,
                                ..rect
                            },
                        )
                    }
                };
                first.layout_into(first_rect, output);
                second.layout_into(second_rect, output);
            }
        }
    }

    pub fn focus_in_direction(
        &self,
        current: PaneId,
        direction: Direction,
        rect: Rect,
    ) -> Option<PaneId> {
        let layout = self.layout(rect);
        let (_, current_rect) = layout.iter().find(|(pane, _)| *pane == current)?;
        let (current_x, current_y) = current_rect.center();
        layout
            .iter()
            .filter(|(pane, _)| *pane != current)
            .filter_map(|(pane, candidate)| {
                let (x, y) = candidate.center();
                let primary = match direction {
                    Direction::Left if x < current_x => current_x - x,
                    Direction::Right if x > current_x => x - current_x,
                    Direction::Up if y < current_y => current_y - y,
                    Direction::Down if y > current_y => y - current_y,
                    _ => return None,
                };
                let secondary = match direction {
                    Direction::Left | Direction::Right => (y - current_y).abs(),
                    Direction::Up | Direction::Down => (x - current_x).abs(),
                };
                Some((*pane, primary + secondary * 2.0))
            })
            .min_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(pane, _)| pane)
    }

    pub fn swap_panes(&mut self, first: PaneId, second: PaneId) -> bool {
        if first == second || !self.contains(first) || !self.contains(second) {
            return false;
        }

        self.swap_pane_ids(first, second);
        true
    }

    fn swap_pane_ids(&mut self, first: PaneId, second: PaneId) {
        match self {
            Self::Leaf { pane } => {
                if *pane == first {
                    *pane = second;
                } else if *pane == second {
                    *pane = first;
                }
            }
            Self::Split {
                first: left,
                second: right,
                ..
            } => {
                left.swap_pane_ids(first, second);
                right.swap_pane_ids(first, second);
            }
        }
    }

    pub fn resize_toward(&mut self, target: PaneId, direction: Direction, amount: f32) -> bool {
        self.resize_inner(target, direction, amount).is_some()
    }

    fn resize_inner(&mut self, target: PaneId, direction: Direction, amount: f32) -> Option<bool> {
        match self {
            Self::Leaf { pane } => (*pane == target).then_some(false),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let in_first = first.contains(target);
                let in_second = second.contains(target);
                if !in_first && !in_second {
                    return None;
                }
                let matching_axis = matches!(
                    (axis, direction),
                    (SplitAxis::Vertical, Direction::Left | Direction::Right)
                        | (SplitAxis::Horizontal, Direction::Up | Direction::Down)
                );
                if matching_axis {
                    let positive = matches!(direction, Direction::Right | Direction::Down);
                    let delta = if in_first == positive {
                        amount
                    } else {
                        -amount
                    };
                    *ratio = (*ratio + delta).clamp(0.1, 0.9);
                    return Some(true);
                }
                let child = if in_first { first } else { second };
                match child.resize_inner(target, direction, amount) {
                    Some(true) => Some(true),
                    _ => Some(false),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_rect() -> Rect {
        Rect {
            width: 100.0,
            height: 100.0,
            ..Rect::default()
        }
    }
    #[test]
    fn swaps_panes_without_changing_tree_shape() {
        let mut tree = PaneTree::leaf(PaneId(1));
        tree.split(PaneId(1), PaneId(2), SplitAxis::Vertical);
        tree.split(PaneId(2), PaneId(3), SplitAxis::Horizontal);

        assert_eq!(tree.panes(), vec![PaneId(1), PaneId(2), PaneId(3)]);

        assert!(tree.swap_panes(PaneId(1), PaneId(3)));

        assert_eq!(tree.panes(), vec![PaneId(3), PaneId(2), PaneId(1)]);

        assert!(!tree.swap_panes(PaneId(1), PaneId(99)));
    }

    #[test]
    fn divider_can_be_hit_and_resized_with_pixels() {
        let mut tree = PaneTree::leaf(PaneId(1));
        tree.split(PaneId(1), PaneId(2), SplitAxis::Vertical);

        let rect = full_rect();

        let handle = tree
            .split_handle_at(50.0, 50.0, rect, 5.0)
            .expect("divider should be detected");

        assert!(tree.resize_split_by_pixels(&handle, rect, 10.0, 0.0,));

        let layout = tree.layout(rect);
        assert!((layout[0].1.width - 60.0).abs() < 0.01);
    }

    #[test]
    fn split_nested_layout_and_directional_focus() {
        let mut tree = PaneTree::leaf(PaneId(1));
        assert!(tree.split(PaneId(1), PaneId(2), SplitAxis::Vertical));
        assert!(tree.split(PaneId(2), PaneId(3), SplitAxis::Horizontal));
        let layout = tree.layout(full_rect());
        assert_eq!(layout.len(), 3);
        assert_eq!(layout[0].1.width, 50.0);
        assert_eq!(layout[1].1.height, 50.0);
        assert_eq!(
            tree.focus_in_direction(PaneId(1), Direction::Right, full_rect()),
            Some(PaneId(2))
        );
        assert_eq!(
            tree.focus_in_direction(PaneId(2), Direction::Down, full_rect()),
            Some(PaneId(3))
        );
    }

    #[test]
    fn close_collapses_nested_parent_and_preserves_remaining_panes() {
        let mut tree = PaneTree::leaf(PaneId(1));
        tree.split(PaneId(1), PaneId(2), SplitAxis::Vertical);
        tree.split(PaneId(2), PaneId(3), SplitAxis::Horizontal);
        assert_eq!(tree.close(PaneId(2)), Some(PaneId(3)));
        assert_eq!(tree.panes(), vec![PaneId(1), PaneId(3)]);
        assert_eq!(tree.close(PaneId(1)), Some(PaneId(3)));
        assert_eq!(tree, PaneTree::leaf(PaneId(3)));
        assert_eq!(tree.close(PaneId(3)), None);
    }

    #[test]
    fn ratios_resize_and_clamp() {
        let mut tree = PaneTree::leaf(PaneId(1));
        tree.split(PaneId(1), PaneId(2), SplitAxis::Vertical);
        assert!(tree.resize_toward(PaneId(1), Direction::Right, 0.1));
        assert_eq!(tree.layout(full_rect())[0].1.width, 60.000004);
        for _ in 0..20 {
            tree.resize_toward(PaneId(1), Direction::Left, 0.1);
        }
        assert_eq!(tree.layout(full_rect())[0].1.width, 10.0);
    }

    #[test]
    fn pane_tree_serialization_round_trip() {
        let mut tree = PaneTree::leaf(PaneId(7));
        tree.split(PaneId(7), PaneId(8), SplitAxis::Horizontal);
        let encoded = toml::to_string(&tree).unwrap();
        let decoded: PaneTree = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, tree);
    }

    #[test]
    fn zoom_changes_only_the_derived_layout() {
        let mut tree = PaneTree::leaf(PaneId(1));
        tree.split(PaneId(1), PaneId(2), SplitAxis::Vertical);
        let before = tree.clone();
        assert_eq!(tree.visible_layout(full_rect(), Some(PaneId(2))).len(), 1);
        assert_eq!(
            tree.visible_layout(full_rect(), Some(PaneId(2)))[0],
            (PaneId(2), full_rect())
        );
        assert_eq!(tree, before);
    }
}
