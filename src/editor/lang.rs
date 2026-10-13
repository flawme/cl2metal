//! Per-language conventions: comment prefixes, indent units, display names.
//!
//! Deliberately a table, not a plugin system: the editor's language support is
//! highlighting (from syntect) plus these few facts, and a table is honest
//! about that.

use std::path::Path;

/// How a language comments out a line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Language {
    pub(crate) name: &'static str,
    /// The line-comment prefix, or "" when the language has none.
    pub(crate) comment: &'static str,
    /// The indent unit in columns, when the language has a convention.
    pub(crate) indent: usize,
}

const PLAIN: Language = Language {
    name: "Plain Text",
    comment: "",
    indent: 4,
};

/// The language for a file, by extension and then by file name.
pub(crate) fn language(path: Option<&Path>) -> Language {
    let Some(path) = path else {
        return PLAIN;
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    // Files whose whole name matters.
    match name.as_str() {
        "makefile" | "gnumakefile" => {
            return Language {
                name: "Makefile",
                comment: "#",
                indent: 4,
            };
        }
        "dockerfile" => {
            return Language {
                name: "Dockerfile",
                comment: "#",
                indent: 4,
            };
        }
        ".gitignore" | ".dockerignore" | ".env" => {
            return Language {
                name: "Config",
                comment: "#",
                indent: 4,
            };
        }
        _ => {}
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rs" => Language {
            name: "Rust",
            comment: "//",
            indent: 4,
        },
        "py" | "pyi" => Language {
            name: "Python",
            comment: "#",
            indent: 4,
        },
        "js" | "mjs" | "cjs" | "jsx" => Language {
            name: "JavaScript",
            comment: "//",
            indent: 2,
        },
        "ts" | "tsx" | "mts" => Language {
            name: "TypeScript",
            comment: "//",
            indent: 2,
        },
        "c" | "h" => Language {
            name: "C",
            comment: "//",
            indent: 4,
        },
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => Language {
            name: "C++",
            comment: "//",
            indent: 4,
        },
        "go" => Language {
            name: "Go",
            comment: "//",
            indent: 4,
        },
        "java" => Language {
            name: "Java",
            comment: "//",
            indent: 4,
        },
        "sh" | "bash" | "zsh" | "fish" => Language {
            name: "Shell",
            comment: "#",
            indent: 4,
        },
        "toml" => Language {
            name: "TOML",
            comment: "#",
            indent: 4,
        },
        "yaml" | "yml" => Language {
            name: "YAML",
            comment: "#",
            indent: 2,
        },
        "json" => Language {
            name: "JSON",
            comment: "",
            indent: 2,
        },
        "md" | "markdown" => Language {
            name: "Markdown",
            comment: "",
            indent: 4,
        },
        "html" | "htm" => Language {
            name: "HTML",
            comment: "",
            indent: 2,
        },
        "css" => Language {
            name: "CSS",
            comment: "",
            indent: 2,
        },
        "sql" => Language {
            name: "SQL",
            comment: "--",
            indent: 4,
        },
        "lua" => Language {
            name: "Lua",
            comment: "--",
            indent: 2,
        },
        "vim" => Language {
            name: "Vim Script",
            comment: "\"",
            indent: 2,
        },
        "ini" | "cfg" | "conf" => Language {
            name: "Config",
            comment: "#",
            indent: 4,
        },
        _ => PLAIN,
    }
}

/// The line-comment prefix for a file.
pub(crate) fn comment_prefix(path: Option<&Path>) -> &'static str {
    language(path).comment
}

/// The display name for a file's language.
pub(crate) fn language_name(path: Option<&Path>) -> &'static str {
    language(path).name
}

/// The indent unit for a file.
pub(crate) fn indent_width(path: Option<&Path>) -> usize {
    language(path).indent
}

/// Whether a path looks like a file the editor should open (not a directory,
/// not a binary).
pub(crate) fn looks_like_text(path: &Path) -> bool {
    let Some(ext) = path.extension().map(|e| e.to_string_lossy().to_lowercase()) else {
        return true;
    };
    !matches!(
        ext.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "bmp"
            | "ico"
            | "webp"
            | "pdf"
            | "zip"
            | "gz"
            | "xz"
            | "zst"
            | "bz2"
            | "7z"
            | "tar"
            | "so"
            | "a"
            | "o"
            | "rlib"
            | "rmeta"
            | "dylib"
            | "dll"
            | "exe"
            | "bin"
            | "class"
            | "pyc"
            | "wasm"
            | "mp3"
            | "mp4"
            | "mkv"
            | "wav"
            | "ogg"
            | "ttf"
            | "otf"
            | "woff"
            | "woff2"
            | "db"
            | "sqlite"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn languages_are_recognised_by_extension_and_name() {
        assert_eq!(language_name(Some(&PathBuf::from("a/b/main.rs"))), "Rust");
        assert_eq!(language_name(Some(&PathBuf::from("x.py"))), "Python");
        assert_eq!(language_name(Some(&PathBuf::from("Makefile"))), "Makefile");
        assert_eq!(
            language_name(Some(&PathBuf::from("Dockerfile"))),
            "Dockerfile"
        );
        assert_eq!(
            language_name(Some(&PathBuf::from("notes.unknownext"))),
            "Plain Text"
        );
        assert_eq!(language_name(None), "Plain Text");
    }

    #[test]
    fn comment_prefixes_follow_the_language() {
        assert_eq!(comment_prefix(Some(&PathBuf::from("a.rs"))), "//");
        assert_eq!(comment_prefix(Some(&PathBuf::from("a.py"))), "#");
        assert_eq!(comment_prefix(Some(&PathBuf::from("a.sql"))), "--");
        // A language without line comments has none to offer.
        assert_eq!(comment_prefix(Some(&PathBuf::from("a.json"))), "");
    }

    #[test]
    fn binary_extensions_are_not_text() {
        assert!(looks_like_text(&PathBuf::from("src/main.rs")));
        assert!(looks_like_text(&PathBuf::from("README")));
        assert!(!looks_like_text(&PathBuf::from("lib/libfoo.so")));
        assert!(!looks_like_text(&PathBuf::from("image.png")));
        // A file with no extension is offered as text: telling a binary from a
        // script needs content sniffing, which the editor does not do.
        assert!(looks_like_text(&PathBuf::from("target/debug/cl2metal")));
    }
}
