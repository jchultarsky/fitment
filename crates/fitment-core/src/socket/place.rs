//! Search trees (`docs/research/m3-socket.md`, N4): boxes in a static
//! bounding-volume hierarchy, asked for those a box meets or those an
//! infinite line passes within a distance of. Building is n log n and a
//! query log n plus what it returns, so the work of a socket grows with
//! what lies near each part, not with the square of the assembly. A box
//! left unknown (an unbounded element, a part with no extent) is returned
//! by every query: what cannot be bounded is never pruned.

use super::geo::V;

/// An axis-aligned box: lowest and highest corner.
pub(crate) type Aabb = (V, V);

/// The most items a leaf holds.
const LEAF: usize = 4;

enum Node {
    Leaf {
        bounds: Aabb,
        from: usize,
        to: usize,
    },
    Inner {
        bounds: Aabb,
        left: usize,
        right: usize,
    },
}

impl Node {
    fn bounds(&self) -> &Aabb {
        match self {
            Self::Leaf { bounds, .. } | Self::Inner { bounds, .. } => bounds,
        }
    }
}

/// The boxes of items `0..n`, searchable.
pub(crate) struct Tree {
    nodes: Vec<Node>,
    /// Item indices, in leaf order.
    items: Vec<usize>,
    boxes: Vec<Option<Aabb>>,
    /// Items with no box, returned by every query.
    unbounded: Vec<usize>,
}

fn union(a: &Aabb, b: &Aabb) -> Aabb {
    (
        [a.0[0].min(b.0[0]), a.0[1].min(b.0[1]), a.0[2].min(b.0[2])],
        [a.1[0].max(b.1[0]), a.1[1].max(b.1[1]), a.1[2].max(b.1[2])],
    )
}

fn finite(b: &Aabb) -> bool {
    b.0.iter().chain(&b.1).all(|x| x.is_finite())
}

/// True if `a` and `b`, grown by `by`, meet.
pub(crate) fn meet(a: &Aabb, b: &Aabb, by: f64) -> bool {
    (0..3).all(|i| a.0[i] <= b.1[i] + by && b.0[i] <= a.1[i] + by)
}

/// True if the line through `point` along `dir` passes within the box
/// grown by `by` (the slab test).
fn crosses(b: &Aabb, point: V, dir: V, by: f64) -> bool {
    let (mut low, mut high) = (f64::NEG_INFINITY, f64::INFINITY);
    for i in 0..3 {
        let (lo, hi) = (b.0[i] - by, b.1[i] + by);
        if dir[i].abs() < 1e-12 {
            if point[i] < lo || point[i] > hi {
                return false;
            }
            continue;
        }
        let (s, t) = ((lo - point[i]) / dir[i], (hi - point[i]) / dir[i]);
        low = low.max(s.min(t));
        high = high.min(s.max(t));
        if low > high {
            return false;
        }
    }
    true
}

impl Tree {
    /// A tree over `boxes`, item `i` being `boxes[i]`; `None`, or a box
    /// with an infinite side, is unbounded.
    pub(crate) fn new(boxes: Vec<Option<Aabb>>) -> Self {
        let boxes: Vec<Option<Aabb>> = boxes.into_iter().map(|b| b.filter(finite)).collect();
        let mut bounded: Vec<usize> = Vec::new();
        let mut unbounded = Vec::new();
        for (i, b) in boxes.iter().enumerate() {
            if b.is_some() {
                bounded.push(i);
            } else {
                unbounded.push(i);
            }
        }
        let mut tree = Self {
            nodes: Vec::new(),
            items: Vec::new(),
            boxes,
            unbounded,
        };
        if !bounded.is_empty() {
            tree.build(&mut bounded);
        }
        tree
    }

    fn box_of(&self, i: usize) -> Aabb {
        self.boxes
            .get(i)
            .copied()
            .flatten()
            .unwrap_or(([0.0; 3], [0.0; 3]))
    }

    /// Builds the subtree over `members` and returns its node's index.
    fn build(&mut self, members: &mut [usize]) -> usize {
        let mut bounds = self.box_of(members.first().copied().unwrap_or(0));
        let mut centres = (bounds.0, bounds.1);
        for &i in members.iter() {
            let b = self.box_of(i);
            bounds = union(&bounds, &b);
            let c = [0, 1, 2].map(|k| f64::midpoint(b.0[k], b.1[k]));
            centres = union(&centres, &(c, c));
        }
        if members.len() <= LEAF {
            let from = self.items.len();
            self.items.extend_from_slice(members);
            self.nodes.push(Node::Leaf {
                bounds,
                from,
                to: self.items.len(),
            });
            return self.nodes.len() - 1;
        }
        // Split at the median centre along the widest spread of centres.
        let spread = [0, 1, 2].map(|k| centres.1[k] - centres.0[k]);
        let axis = if spread[0] >= spread[1] && spread[0] >= spread[2] {
            0
        } else if spread[1] >= spread[2] {
            1
        } else {
            2
        };
        let centre = |i: usize| {
            let b = self.box_of(i);
            b.0[axis] + b.1[axis]
        };
        let half = members.len() / 2;
        members.select_nth_unstable_by(half, |x, y| {
            centre(*x).total_cmp(&centre(*y)).then(x.cmp(y))
        });
        let (left_members, right_members) = members.split_at_mut(half);
        let left = self.build(left_members);
        let right = self.build(right_members);
        self.nodes.push(Node::Inner {
            bounds,
            left,
            right,
        });
        self.nodes.len() - 1
    }

    /// Every item whose box `test` passes, with the unbounded ones, in
    /// ascending order.
    fn search(&self, test: impl Fn(&Aabb) -> bool) -> Vec<usize> {
        let mut out = self.unbounded.clone();
        let mut stack: Vec<usize> = self.nodes.len().checked_sub(1).into_iter().collect();
        while let Some(k) = stack.pop() {
            let Some(node) = self.nodes.get(k) else {
                continue;
            };
            if !test(node.bounds()) {
                continue;
            }
            match node {
                Node::Leaf { from, to, .. } => {
                    for &i in self.items.get(*from..*to).unwrap_or(&[]) {
                        if self
                            .boxes
                            .get(i)
                            .copied()
                            .flatten()
                            .is_some_and(|b| test(&b))
                        {
                            out.push(i);
                        }
                    }
                }
                Node::Inner { left, right, .. } => {
                    stack.push(*left);
                    stack.push(*right);
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// The items whose boxes meet `query` grown by `by`.
    pub(crate) fn meeting(&self, query: &Aabb, by: f64) -> Vec<usize> {
        if !finite(query) {
            return (0..self.boxes.len()).collect();
        }
        self.search(|b| meet(b, query, by))
    }

    /// The items whose boxes, grown by `by`, the infinite line through
    /// `point` along `dir` crosses.
    pub(crate) fn along(&self, point: V, dir: V, by: f64) -> Vec<usize> {
        if !(point.iter().chain(&dir).all(|x| x.is_finite())) {
            return (0..self.boxes.len()).collect();
        }
        self.search(|b| crosses(b, point, dir, by))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(x: f64, y: f64, z: f64) -> Aabb {
        ([x, y, z], [x + 1.0, y + 1.0, z + 1.0])
    }

    #[test]
    fn a_query_returns_exactly_the_boxes_a_scan_would() {
        // A 20 x 20 grid of unit boxes 2 apart, one unbounded.
        let mut boxes = Vec::new();
        for i in 0..20 {
            for j in 0..20 {
                boxes.push(Some(unit(f64::from(i) * 2.0, f64::from(j) * 2.0, 0.0)));
            }
        }
        boxes.push(None);
        let tree = Tree::new(boxes.clone());
        let scan = |test: &dyn Fn(&Aabb) -> bool| -> Vec<usize> {
            boxes
                .iter()
                .enumerate()
                .filter(|(_, b)| b.as_ref().is_none_or(test))
                .map(|(i, _)| i)
                .collect()
        };
        let query = ([5.5, 5.5, 0.2], [9.2, 7.0, 0.4]);
        assert_eq!(tree.meeting(&query, 0.1), scan(&|b| meet(b, &query, 0.1)));
        let (point, dir) = ([0.5, 0.0, 0.5], [0.0, 1.0, 0.0]);
        assert_eq!(
            tree.along(point, dir, 0.0),
            scan(&|b| crosses(b, point, dir, 0.0))
        );
        let slant = [1.0, 1.0, 0.0];
        assert_eq!(
            tree.along(point, slant, 0.3),
            scan(&|b| crosses(b, point, slant, 0.3))
        );
        // The unbounded box comes back from every query.
        assert_eq!(tree.meeting(&([100.0; 3], [101.0; 3]), 0.0), vec![400]);
    }

    #[test]
    fn an_empty_or_unbounded_tree_answers() {
        assert_eq!(
            Tree::new(Vec::new()).meeting(&([0.0; 3], [1.0; 3]), 0.0),
            Vec::<usize>::new()
        );
        let open = Tree::new(vec![
            Some(([0.0; 3], [f64::INFINITY; 3])),
            Some(unit(0.0, 0.0, 0.0)),
        ]);
        assert_eq!(open.meeting(&([5.0; 3], [6.0; 3]), 0.0), vec![0]);
    }
}
