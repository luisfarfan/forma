//! Registry entries for the tools that Task 16 implements.
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
         roots. Overwriting an existing file is destructive and may require approval \
         depending on the server policy (APPROVAL_REQUIRED with a plan to confirm).",
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
