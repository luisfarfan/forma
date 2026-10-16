//! Registry entries for the tools that Tasks 15 and 16 implement.
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
