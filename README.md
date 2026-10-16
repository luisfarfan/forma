# FORMA

FORMA is an open-source, cross-platform (macOS, Windows, Linux) professional font editor with a Rust core and a first-class [Model Context Protocol](https://modelcontextprotocol.io) server, so AI agents such as Claude Code or Codex can drive every editor operation through the same engine as the UI and the CLI.

> **Status:** early development — Milestone 1 (headless core + MCP + CLI). There is no graphical editor yet.

## Goals

- Create, edit, space, kern, validate and export real fonts (TTF/WOFF2 first; variable and CFF later).
- One typographic engine for the desktop app, the `forma` CLI and the `forma mcp` server — no duplicated business logic.
- Agent-safe editing: transactions, per-entity revisions, undo/redo, and explicit approval for destructive operations.
- Verifiable output: every export is validated (OTS, fontspector, shaping checks).

## Documentation

Design documents (in Spanish) live in [`docs/`](docs/):
research, tech stack, architecture, roadmap, reverse-engineering policy, risks and the MCP/AI architecture.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
