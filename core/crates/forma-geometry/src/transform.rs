//! Affine transforms of contours.

use forma_model::{Path, Transform};

use crate::direction::reversed_nodes;

/// Applies `t` to every node of `paths`.
///
/// Node kinds and `smooth` flags are kept, and so are the contour ids. A
/// transform that mirrors (negative determinant) turns a counter-clockwise
/// contour clockwise, so each closed contour is reversed as well and keeps its
/// direction: an exterior stays counter-clockwise and a hole clockwise. Open
/// contours have no direction and keep their drawing order.
///
/// A closed contour that cannot be reversed because it is malformed (see
/// [`to_bezpath`](crate::to_bezpath)) is moved without being reversed.
#[must_use]
pub fn transform_paths(paths: &[Path], t: &Transform) -> Vec<Path> {
    let mirrors = t.determinant() < 0.0;
    paths
        .iter()
        .map(|path| {
            let mut nodes = if mirrors && path.closed {
                reversed_nodes(path).unwrap_or_else(|_| path.nodes.clone())
            } else {
                path.nodes.clone()
            };
            for node in &mut nodes {
                (node.x, node.y) = t.apply(node.x, node.y);
            }
            Path {
                id: path.id,
                closed: path.closed,
                nodes,
            }
        })
        .collect()
}
