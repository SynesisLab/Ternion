//! `fs_search` (§6.2): content search on the bundled ripgrep engine —
//! ripgrep's own library crates (`grep-regex`, `grep-searcher`, `ignore`),
//! so regexes, globs and `.gitignore` semantics match the real `rg`.

use futures::future::BoxFuture;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder, Sink, SinkMatch};
use ignore::WalkBuilder;

use super::super::{Tool, ToolError, ToolExecCtx, ToolOutcome};
use super::{arg_str, arg_usize};

const DEFAULT_MAX: usize = 100;
const MAX_MAX: usize = 500;

/// Owned search parameters, moved into the blocking task.
struct Prepared {
    roots: Vec<String>,
    pattern: String,
    glob: Option<String>,
    max: usize,
    multi_root: bool,
}

pub struct FsSearch;

impl Tool for FsSearch {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "fs_search".into(),
            description: "Search file contents inside a bound workspace with ripgrep: regex, optional filename glob, respects .gitignore. Results are path:line: text."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["pattern"],
                "properties": {
                    "pattern": { "type": "string", "description": "Rust regex, e.g. \"fn \\w+\\(\". Smart case by default." },
                    "path": { "type": "string", "description": "Directory to search (default: every bound workspace root)" },
                    "glob": { "type": "string", "description": "Only search files matching this glob, e.g. \"*.rs\"" },
                    "max_results": { "type": "integer", "description": "Cap on matched lines (default 100, max 500)" }
                }
            }),
        }
    }

    fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecCtx,
    ) -> BoxFuture<'_, Result<ToolOutcome, ToolError>> {
        // Resolve (guard) synchronously — the boxed future must not borrow
        // `ctx`; the walk itself can scan a whole repo, so it runs on the
        // blocking pool.
        let prepared = (|| -> Result<Prepared, ToolError> {
            let roots: Vec<String> = match args.get("path").and_then(|v| v.as_str()) {
                Some(_) => vec![super::resolve_arg(&args, ctx, "path")?.path.display().to_string()],
                None => ctx.workspaces.clone(),
            };
            let pattern = arg_str(&args, "pattern").unwrap_or("").to_string();
            let glob = arg_str(&args, "glob").map(str::to_string);
            let max = arg_usize(&args, "max_results", DEFAULT_MAX, 1, MAX_MAX);
            let multi_root = roots.len() > 1;
            Ok(Prepared {
                roots,
                pattern,
                glob,
                max,
                multi_root,
            })
        })();

        Box::pin(async move {
            match prepared {
                Ok(p) => {
                    tokio::task::spawn_blocking(move || {
                        search_all(&p.roots, &p.pattern, p.glob.as_deref(), p.max, p.multi_root)
                    })
                    .await
                    .map_err(|e| ToolError::Exec(format!("search task failed: {e}")))?
                    .map(ToolOutcome::Ok)
                }
                Err(e) => Err(e),
            }
        })
    }
}

fn search_all(
    roots: &[String],
    pattern: &str,
    glob: Option<&str>,
    max: usize,
    multi_root: bool,
) -> Result<String, ToolError> {
    let matcher = RegexMatcherBuilder::new()
        .case_smart(true)
        .build(pattern)
        .map_err(|e| ToolError::Exec(format!("bad regex: {e}")))?;
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0x00))
        .line_number(true)
        .build();

    let mut out = String::new();
    let mut total = 0usize;
    for root in roots {
        let root = std::path::PathBuf::from(root);
        let mut walk = WalkBuilder::new(&root);
        walk.threads(1);
        // A bound workspace is a deliberate grant; honor .gitignore even
        // outside a git repository (rg's default require_git would skip it).
        walk.require_git(false);
        if let Some(glob) = glob {
            let mut overrides = ignore::overrides::OverrideBuilder::new(&root);
            overrides
                .add(glob)
                .map_err(|e| ToolError::Exec(format!("bad glob `{glob}`: {e}")))?;
            walk.overrides(overrides.build().map_err(|e| {
                ToolError::Exec(format!("bad glob `{glob}`: {e}"))
            })?);
        }
        let mut per_root: Vec<(String, u64, String)> = Vec::new();
        let mut truncated = false;
        for entry in walk.build() {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path().to_path_buf();
            let mut sink = LineCollector::new(max);
            let _ = searcher.search_path(&matcher, &path, &mut sink);
            if sink.hits.len() >= max {
                truncated = true;
            }
            for (line_no, line) in sink.hits {
                let rel = entry
                    .path()
                    .strip_prefix(&root)
                    .unwrap_or(entry.path())
                    .display()
                    .to_string()
                    .replace('\\', "/"); // forward slashes: model-friendly
                per_root.push((rel, line_no, line));
                total += 1;
            }
        }
        // Group by file, ascending line — models read grouped results well.
        per_root.sort();
        if multi_root {
            out.push_str(&format!("# {}\n", root.display()));
        }
        for (rel, line_no, line) in &per_root {
            out.push_str(&format!("{rel}:{line_no}: {line}\n"));
        }
        if truncated {
            out.push_str(&format!("[… more matches not shown — cap {max}]\n"));
        }
        if out.chars().count() > super::super::MAX_RESULT_CHARS {
            break;
        }
    }
    if total == 0 {
        out.push_str("(no matches)\n");
    }
    Ok(out)
}

/// Collects matched lines until the cap, then signals the searcher to stop.
struct LineCollector {
    max: usize,
    hits: Vec<(u64, String)>,
}

impl LineCollector {
    fn new(max: usize) -> Self {
        Self {
            max,
            hits: Vec::new(),
        }
    }
}

impl Sink for LineCollector {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &grep_searcher::Searcher, mat: &SinkMatch) -> Result<bool, Self::Error> {
        if self.hits.len() >= self.max {
            return Ok(false); // stop searching this file
        }
        let line = String::from_utf8_lossy(mat.bytes())
            .trim_end_matches(['\r', '\n'])
            .to_string();
        let line_no = mat.line_number().unwrap_or(0);
        self.hits.push((line_no, line));
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(root: &std::path::Path) -> ToolExecCtx {
        ToolExecCtx {
            workspaces: vec![root.display().to_string()],
            attachments: None,
        }
    }

    #[test]
    fn finds_regex_matches_grouped_by_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        std::fs::write(dir.path().join("b.md"), "# Title\nsome fn words\n").unwrap();
        let root = dir.path().canonicalize().unwrap();
        let out = search_all(
            &[root.display().to_string()],
            r"fn \w+",
            None,
            DEFAULT_MAX,
            false,
        )
        .unwrap();
        assert!(out.contains("src/a.rs:1: fn alpha() {}"), "{out}");
        assert!(out.contains("src/a.rs:2: fn beta() {}"), "{out}");
        assert!(out.contains("b.md:2: some fn words"), "{out}");
        // Grouped: the .md block sorts before src/.
        let md = out.find("b.md").unwrap();
        let rs = out.find("src/a.rs").unwrap();
        assert!(md < rs, "grouped by file: {out}");
    }

    #[test]
    fn no_matches_note() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let out = search_all(
            &[dir.path().canonicalize().unwrap().display().to_string()],
            "zebra",
            None,
            DEFAULT_MAX,
            false,
        )
        .unwrap();
        assert!(out.contains("no matches"), "{out}");
    }

    #[test]
    fn glob_restricts_file_set() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "target here\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "target here\n").unwrap();
        let out = search_all(
            &[dir.path().canonicalize().unwrap().display().to_string()],
            "target",
            Some("*.rs"),
            DEFAULT_MAX,
            false,
        )
        .unwrap();
        assert!(out.contains("a.rs"), "{out}");
        assert!(!out.contains("b.txt"), "{out}");
    }

    #[test]
    fn respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "secret.txt\n").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("open.txt"), "needle\n").unwrap();
        let out = search_all(
            &[dir.path().canonicalize().unwrap().display().to_string()],
            "needle",
            None,
            DEFAULT_MAX,
            false,
        )
        .unwrap();
        assert!(out.contains("open.txt"), "{out}");
        assert!(!out.contains("secret.txt"), ".gitignore respected: {out}");
    }

    #[test]
    fn cap_stops_before_the_firehose() {
        let dir = tempfile::tempdir().unwrap();
        let big: String = (0..2000).map(|_| "hit\n").collect();
        std::fs::write(dir.path().join("big.txt"), big).unwrap();
        let out = search_all(
            &[dir.path().canonicalize().unwrap().display().to_string()],
            "hit",
            None,
            50,
            false,
        )
        .unwrap();
        assert_eq!(out.lines().count(), 51, "50 hits + cap note: {out}");
        assert!(out.contains("[… more matches"), "{out}");
    }

    #[test]
    fn bad_regex_is_a_tool_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = search_all(
            &[dir.path().canonicalize().unwrap().display().to_string()],
            "(unclosed",
            None,
            DEFAULT_MAX,
            false,
        )
        .unwrap_err();
        assert!(err.message().contains("bad regex"), "{err:?}");
    }

    #[tokio::test]
    async fn executes_through_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "the answer is 42\n").unwrap();
        let out = FsSearch
            .execute(
                serde_json::json!({"pattern": "answer"}),
                &ctx_with(dir.path()),
            )
            .await;
        let out = crate::tools::fs::unwrap_ok(out.unwrap());
        assert!(out.contains("f.txt:1"), "{out}");
    }

    #[tokio::test]
    async fn searches_every_bound_root_when_path_omitted() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("x.txt"), "shared\n").unwrap();
        std::fs::write(b.path().join("y.txt"), "shared\n").unwrap();
        let ctx = ToolExecCtx {
            workspaces: vec![
                a.path().canonicalize().unwrap().display().to_string(),
                b.path().canonicalize().unwrap().display().to_string(),
            ],
        attachments: None,
        };
        let out = FsSearch
            .execute(serde_json::json!({"pattern": "shared"}), &ctx)
            .await;
        let out = crate::tools::fs::unwrap_ok(out.unwrap());
        assert!(out.contains("# "), "per-root headers: {out}");
        assert!(out.contains("x.txt:1"), "{out}");
        assert!(out.contains("y.txt:1"), "{out}");
    }
}