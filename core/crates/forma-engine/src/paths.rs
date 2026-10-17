//! The path sandbox: every file a tool reads or writes must lie inside one of
//! the allowed roots.
//!
//! Paths are compared after canonicalization (symbolic links and `..`
//! resolved), so neither `..` nor a link inside a root can reach a file
//! outside it. Checking and then using a path is not atomic: a link swapped in
//! between the two by another process is out of scope.

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::json;

use crate::engine::{Engine, path_text};
use crate::errors::ToolError;

/// The native document extensions, compared case-insensitively.
const DOCUMENT_EXTENSIONS: [&str; 2] = ["forma", "formapackage"];

impl Engine {
    /// Resolves `p` and checks that it lies inside an allowed root, returning
    /// the canonical path.
    ///
    /// * A relative `p` is resolved against the first allowed root.
    /// * If `p` exists, it is canonicalized (following symbolic links).
    /// * Otherwise its parent is canonicalized and the file name appended; the
    ///   parent must exist.
    /// * The result must start with one of the canonical allowed roots.
    ///
    /// # Errors
    ///
    /// * `PATH_NOT_ALLOWED` if there are no allowed roots, if the resolved
    ///   path lies outside them, or if `p` is a link that cannot be resolved;
    ///   a path outside the roots is reported as such even when its parent
    ///   does not exist, so nothing is revealed about other directories;
    /// * `INVALID_ARGUMENT` if `p` has no file name, or if its parent
    ///   directory does not exist (inside the roots);
    /// * `NOT_FOUND` if `must_exist` and nothing exists at `p`;
    /// * `IO_ERROR` if the file system cannot be queried.
    pub fn check_allowed(&self, p: &Path, must_exist: bool) -> Result<PathBuf, ToolError> {
        let Some(first_root) = self.roots.first() else {
            return Err(ToolError::path_not_allowed(
                "no allowed roots are configured; file access is disabled",
            ));
        };
        let path = if p.is_absolute() {
            p.to_path_buf()
        } else {
            first_root.join(p)
        };

        let (resolved, exists) = if path.symlink_metadata().is_ok() {
            let canonical = dunce::canonicalize(&path).map_err(|error| {
                ToolError::path_not_allowed(format!(
                    "path '{}' cannot be resolved inside the allowed roots: {error}",
                    p.display()
                ))
            })?;
            (canonical, true)
        } else {
            (self.resolve_missing(p, &path)?, false)
        };

        if !self.is_within_roots(&resolved) {
            return Err(self.outside_roots(p));
        }
        if must_exist && !exists {
            return Err(ToolError::not_found(format!(
                "no such file or folder: '{}'",
                p.display()
            )));
        }
        Ok(resolved)
    }

    /// [`check_allowed`](Self::check_allowed) for a native document: the path
    /// must also end in `.forma` or `.formapackage` (any case).
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT` for another extension, then every error of
    /// [`check_allowed`](Self::check_allowed).
    pub fn check_document_path(&self, p: &Path, must_exist: bool) -> Result<PathBuf, ToolError> {
        if !has_document_extension(p) {
            return Err(ToolError::invalid_argument(format!(
                "'{}' is not a FORMA document path: use a .forma file or a .formapackage folder",
                p.display()
            )));
        }
        self.check_allowed(p, must_exist)
    }

    /// The canonical path of a file that does not exist yet: its canonical
    /// parent plus its file name.
    fn resolve_missing(&self, p: &Path, path: &Path) -> Result<PathBuf, ToolError> {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(ToolError::invalid_argument(format!(
                "'{}' does not name a file",
                p.display()
            )));
        };
        match dunce::canonicalize(parent) {
            Ok(dir) => Ok(dir.join(name)),
            Err(error) if is_missing(&error) => {
                // Say "outside the roots" rather than "missing parent" for a
                // path that would land outside them anyway.
                if self.lands_within_roots(path) {
                    Err(ToolError::invalid_argument(
                        "parent directory does not exist",
                    ))
                } else {
                    Err(self.outside_roots(p))
                }
            }
            Err(error) => Err(ToolError::io(format!(
                "cannot resolve '{}': {error}",
                p.display()
            ))),
        }
    }

    /// Whether a path whose parent does not exist would lie inside the roots:
    /// its nearest existing ancestor, canonicalized, followed by the missing
    /// components. A `..` among the missing components cannot be resolved and
    /// counts as outside.
    fn lands_within_roots(&self, path: &Path) -> bool {
        let mut missing: Vec<&OsStr> = Vec::new();
        let mut ancestor = path;
        loop {
            if let Ok(canonical) = dunce::canonicalize(ancestor) {
                let mut candidate = canonical;
                for component in missing.iter().rev() {
                    candidate.push(component);
                }
                return self.is_within_roots(&candidate);
            }
            let Some(parent) = ancestor.parent() else {
                return false;
            };
            match ancestor.components().next_back() {
                Some(Component::Normal(name)) => missing.push(name),
                _ => return false,
            }
            ancestor = parent;
        }
    }

    fn is_within_roots(&self, canonical: &Path) -> bool {
        self.roots.iter().any(|root| canonical.starts_with(root))
    }

    fn outside_roots(&self, p: &Path) -> ToolError {
        let roots: Vec<String> = self.roots.iter().map(|root| path_text(root)).collect();
        ToolError::path_not_allowed(format!(
            "path '{}' is outside the allowed roots",
            p.display()
        ))
        .with_details(json!({ "allowed_roots": roots }))
    }
}

/// Whether `p` ends in `.forma` or `.formapackage`, ignoring case.
#[must_use]
pub fn has_document_extension(p: &Path) -> bool {
    p.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            DOCUMENT_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// Whether an error means that a path (or one of its directories) does not
/// exist.
fn is_missing(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}
