//! Single glyph previews as SVG.

use forma_geometry::path_to_svg;
use forma_model::{Font, Glyph, MasterId, NodeKind};

use crate::draw::{
    Frame, HANDLE_RGB, METRICS_RGB, NODE_SIDE_PX, OFF_CURVE_RGB, ON_CURVE_RGB, check_size, handles,
    layer_of, metrics_of, outline,
};
use crate::{PreviewOptions, RenderError};

/// Formats a number with at most four decimals and no trailing zeros.
fn num(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// Renders a glyph as a standalone SVG document.
///
/// The `viewBox` is in font units and covers the same square as
/// [`glyph_png`](crate::glyph_png) frames (the glyph box with a 10% margin);
/// `width` and `height` are `opts.size`. The document has a white background
/// and a single `<path fill="black">` holding all the contours, with a
/// `scale(1,-1)` transform turning the y-up font coordinates into SVG's y-down
/// ones. `show_metrics` and `show_nodes` add the guides and nodes of the PNG
/// preview as `<g id="metrics">` and `<g id="nodes">` groups. If the master's
/// ascender and descender are unusable (not finite, or the ascender is not
/// above the descender), proportional metrics are used instead: ascender 80%
/// and descender -20% of the units per em.
///
/// # Errors
///
/// * [`RenderError::InvalidSize`] if `opts.size` is outside `16..=1024`;
/// * [`RenderError::MissingLayer`] if the glyph has no layer for `master`;
/// * [`RenderError::Geometry`] if a contour cannot be converted to a curve, or
///   if the box cannot be framed ("degenerate framing", as for
///   [`glyph_png`](crate::glyph_png)).
pub fn glyph_svg(
    font: &Font,
    glyph: &Glyph,
    master: MasterId,
    opts: &PreviewOptions,
) -> Result<String, RenderError> {
    let size = check_size(opts.size)?;
    let layer = layer_of(glyph, master)?;
    // `path_to_svg` writes nothing for a contour it cannot convert; check
    // them all up front so that failure is an error rather than a gap.
    outline(&glyph.name, layer)?;

    let metrics = metrics_of(font, Some(master));
    let frame = Frame::new(&metrics, layer.advance_width, size)?;
    let half = frame.side / 2.0;
    // Font units per pixel: the stroke width of a 1 px line.
    let pixel = frame.side / frame.size;
    let left = frame.cx - half;
    let (bottom, top) = (frame.cy - half, frame.cy + half);
    let side = num(frame.side);

    let mut lines = vec![
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{size}\" height=\"{size}\" \
             viewBox=\"{} {} {side} {side}\">",
            num(left),
            num(-top),
        ),
        format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{side}\" height=\"{side}\" fill=\"white\"/>",
            num(left),
            num(-top),
        ),
    ];

    if opts.show_metrics {
        lines.push(format!(
            "<g id=\"metrics\" stroke=\"{}\" stroke-width=\"{}\" transform=\"scale(1,-1)\">",
            hex(METRICS_RGB),
            num(pixel),
        ));
        let heights = [
            0.0,
            metrics.x_height,
            metrics.cap_height,
            metrics.ascender,
            metrics.descender,
        ];
        for y in heights.into_iter().filter(|y| y.is_finite()) {
            lines.push(format!(
                "<line x1=\"{}\" y1=\"{y}\" x2=\"{}\" y2=\"{y}\"/>",
                num(left),
                num(left + frame.side),
                y = num(y),
            ));
        }
        let advance = layer.advance_width;
        let advance = (advance.is_finite() && advance > 0.0).then_some(advance);
        for x in std::iter::once(0.0).chain(advance) {
            lines.push(format!(
                "<line x1=\"{x}\" y1=\"{}\" x2=\"{x}\" y2=\"{}\"/>",
                num(bottom),
                num(top),
                x = num(x),
            ));
        }
        lines.push("</g>".to_owned());
    }

    let data = layer
        .paths
        .iter()
        .map(path_to_svg)
        .filter(|d| !d.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(format!(
        "<path fill=\"black\" fill-rule=\"nonzero\" transform=\"scale(1,-1)\" d=\"{data}\"/>"
    ));

    if opts.show_nodes {
        lines.push("<g id=\"nodes\" transform=\"scale(1,-1)\">".to_owned());
        for path in &layer.paths {
            for (off, on) in handles(path) {
                lines.push(format!(
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" \
                     stroke-width=\"{}\"/>",
                    num(off.x),
                    num(off.y),
                    num(on.x),
                    num(on.y),
                    hex(HANDLE_RGB),
                    num(pixel),
                ));
            }
        }
        let square = NODE_SIDE_PX * pixel;
        for path in &layer.paths {
            for node in &path.nodes {
                let rgb = if node.kind == NodeKind::OffCurve {
                    OFF_CURVE_RGB
                } else {
                    ON_CURVE_RGB
                };
                lines.push(format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
                    num(node.x - square / 2.0),
                    num(node.y - square / 2.0),
                    num(square),
                    num(square),
                    hex(rgb),
                ));
            }
        }
        lines.push("</g>".to_owned());
    }

    lines.push("</svg>".to_owned());
    lines.push(String::new());
    Ok(lines.join("\n"))
}
