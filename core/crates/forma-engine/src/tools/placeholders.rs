//! Registry entries for the tools that Tasks 14-16 implement.
//!
//! Each placeholder has its final name, title, toolset, effects and output
//! kind, accepts any object and answers `UNSUPPORTED` ("not implemented
//! yet"). The task that implements a tool moves its spec next to its handler
//! and replaces the entry in the registry.

use forma_model::Actor;
use serde_json::Value;

use crate::engine::Engine;
use crate::errors::ToolError;
use crate::registry::{CommandSpec, Effects, OutputKind, ToolOutput, Toolset, any_object_schema};

/// The handler of every placeholder.
fn not_implemented(
    _engine: &mut Engine,
    _actor: &Actor,
    _args: Value,
) -> Result<ToolOutput, ToolError> {
    Err(ToolError::unsupported("not implemented yet"))
}

macro_rules! placeholder {
    ($const:ident, $name:literal, $title:literal, $toolset:ident, $effects:ident, $output:ident, $description:expr) => {
        pub(crate) const $const: CommandSpec = CommandSpec {
            name: $name,
            title: $title,
            description: $description,
            toolset: Toolset::$toolset,
            effects: Effects::$effects,
            output: OutputKind::$output,
            input_schema: any_object_schema,
            output_schema: None,
            handler: not_implemented,
        };
    };
}

// core

placeholder!(
    FONT_SET_INFO,
    "font_set_info",
    "Set font info",
    Core,
    WRITE,
    Json,
    concat!(
        "Change the naming and version information of the font in `doc`.",
        base_rev_note!()
    )
);
placeholder!(
    FONT_SET_VERTICAL_METRICS,
    "font_set_vertical_metrics",
    "Set vertical metrics",
    Core,
    WRITE,
    Json,
    concat!(
        "Change the vertical metrics (ascender, descender, x-height, cap height) of one \
         master of `doc`.",
        base_rev_note!()
    )
);
placeholder!(
    EDIT_BATCH,
    "edit_batch",
    "Edit in batch",
    Core,
    WRITE,
    Json,
    concat!(
        "Apply several font, glyph and metrics operations to `doc` atomically, as one \
         undoable transaction: either every operation applies or none does.",
        base_rev_note!()
    )
);
placeholder!(
    PLAN_COMMIT,
    "plan_commit",
    "Commit plan",
    Core,
    DESTRUCTIVE,
    Json,
    "Apply a plan returned with APPROVAL_REQUIRED, identified by its `plan_token`. Only \
     commit a plan the user has confirmed; a token is single-use and expires."
);
placeholder!(
    PLAN_DISCARD,
    "plan_discard",
    "Discard plan",
    Core,
    WRITE,
    Json,
    "Discard a pending plan, identified by its `plan_token`, without applying it."
);

// glyphs

placeholder!(
    GLYPH_LIST,
    "glyph_list",
    "List glyphs",
    Glyphs,
    READ,
    Json,
    "List the glyphs of `doc` page by page, with their names, code points, advance widths \
     and contour and node counts. The result includes the current `rev`."
);
placeholder!(
    GLYPH_GET,
    "glyph_get",
    "Get glyph",
    Glyphs,
    READ,
    Json,
    "Get one glyph of `doc`: code points, advance width, side bearings, bounding box and \
     outline, both as structured contours and as an SVG path. The result includes the \
     current `rev`."
);
placeholder!(
    GLYPH_CREATE,
    "glyph_create",
    "Create glyph",
    Glyphs,
    WRITE,
    Json,
    concat!(
        "Create a glyph in `doc` with a name, optional code points, an advance width and an \
         outline given as an SVG path or as structured contours.",
        base_rev_note!()
    )
);
placeholder!(
    GLYPH_UPDATE,
    "glyph_update",
    "Update glyph",
    Glyphs,
    WRITE,
    Json,
    concat!(
        "Rename a glyph of `doc` or change its code points.",
        base_rev_note!()
    )
);
placeholder!(
    GLYPH_DELETE,
    "glyph_delete",
    "Delete glyphs",
    Glyphs,
    DESTRUCTIVE,
    Json,
    concat!(
        "Delete glyphs from `doc`. Destructive: answers APPROVAL_REQUIRED with a plan to \
         confirm.",
        base_rev_note!()
    )
);
placeholder!(
    GLYPH_SET_OUTLINE,
    "glyph_set_outline",
    "Set glyph outline",
    Glyphs,
    DESTRUCTIVE,
    Json,
    concat!(
        "Replace every contour of one glyph layer of `doc` with a new outline, given as an \
         SVG path or as structured contours. Destructive when the layer is not empty.",
        base_rev_note!()
    )
);
placeholder!(
    CONTOUR_EDIT,
    "contour_edit",
    "Edit contours",
    Glyphs,
    WRITE,
    Json,
    concat!(
        "Add, remove, reverse or transform contours of a glyph of `doc`.",
        base_rev_note!()
    )
);
placeholder!(
    NODES_EDIT,
    "nodes_edit",
    "Edit nodes",
    Glyphs,
    WRITE,
    Json,
    concat!(
        "Move, insert or delete nodes of a glyph contour of `doc`, or switch them between \
         smooth and corner.",
        base_rev_note!()
    )
);

// geometry

placeholder!(
    GEOMETRY_CHECK,
    "geometry_check",
    "Check geometry",
    Geometry,
    READ,
    Json,
    "Check the outlines of `doc` for open contours, wrong direction, missing extremes, \
     duplicate nodes, tiny segments and off-grid nodes. The result includes the current \
     `rev`."
);
placeholder!(
    GEOMETRY_FIX,
    "geometry_fix",
    "Fix geometry",
    Geometry,
    WRITE,
    Json,
    concat!(
        "Apply the safe automatic fixes for the geometry issues of `doc` that geometry_check \
         reports as fixable.",
        base_rev_note!()
    )
);

// spacing

placeholder!(
    METRICS_GET,
    "metrics_get",
    "Get metrics",
    Spacing,
    READ,
    Json,
    "Get the advance width and the left and right side bearings of glyphs of `doc`. The \
     result includes the current `rev`."
);
placeholder!(
    METRICS_SET,
    "metrics_set",
    "Set metrics",
    Spacing,
    WRITE,
    Json,
    concat!(
        "Set the advance width or the left or right side bearing of glyphs of `doc`.",
        base_rev_note!()
    )
);

// quality

placeholder!(
    VALIDATE,
    "validate",
    "Validate font",
    Quality,
    READ,
    Json,
    "Validate `doc`: data model rules plus checks on the compiled binary, returned as a \
     machine-readable report with the current `rev`."
);

// export

placeholder!(
    EXPORT_FONT,
    "export_font",
    "Export font",
    Export,
    WRITE,
    Json,
    concat!(
        "Compile `doc` to TTF or WOFF2, or export it as UFO, to `path` inside the allowed \
         roots. Overwriting an existing file answers APPROVAL_REQUIRED with a plan to \
         confirm.",
        base_rev_note!()
    )
);

// preview

placeholder!(
    PREVIEW_GLYPH,
    "preview_glyph",
    "Preview glyph",
    Preview,
    READ,
    Image,
    "Render one glyph of `doc` as a PNG image (512 px by default, 1024 px at most), \
     optionally with its nodes and metric lines, or as SVG text."
);
placeholder!(
    PREVIEW_TEXT,
    "preview_text",
    "Preview text",
    Preview,
    READ,
    Image,
    "Render a line of text set with the glyphs of `doc` as a PNG image."
);
placeholder!(
    PREVIEW_COMPARE,
    "preview_compare",
    "Compare glyph revisions",
    Preview,
    READ,
    Image,
    "Render one glyph of `doc` at two revisions overlaid in a PNG image, to compare a \
     change visually."
);

// history

placeholder!(
    HISTORY_LIST,
    "history_list",
    "List history",
    History,
    READ,
    Json,
    "List the transactions of `doc` page by page, oldest first, with their actor, label, \
     revision and touched entities. The result includes the current `rev`."
);
placeholder!(
    HISTORY_UNDO,
    "history_undo",
    "Undo",
    History,
    WRITE,
    Json,
    "Undo your own last transaction in `doc`. If the last transaction belongs to another \
     actor, take it back with history_revert instead."
);
placeholder!(
    HISTORY_REDO,
    "history_redo",
    "Redo",
    History,
    WRITE,
    Json,
    "Redo your own last undone transaction in `doc`."
);
placeholder!(
    HISTORY_REVERT,
    "history_revert",
    "Revert transaction",
    History,
    WRITE,
    Json,
    concat!(
        "Take back one transaction of `doc` as a new undoable transaction, provided the \
         entities it touched have not changed since.",
        base_rev_note!()
    )
);
placeholder!(
    HISTORY_RESTORE,
    "history_restore",
    "Restore revision",
    History,
    DESTRUCTIVE,
    Json,
    concat!(
        "Restore `doc` to the state of an earlier revision as a new undoable transaction. \
         Destructive: answers APPROVAL_REQUIRED with a plan to confirm.",
        base_rev_note!()
    )
);
placeholder!(
    DOCUMENT_DIFF,
    "document_diff",
    "Diff revisions",
    History,
    READ,
    Json,
    "Compare two revisions of `doc` and list the changed font info, vertical metrics and \
     glyphs. The result includes the current `rev`."
);
