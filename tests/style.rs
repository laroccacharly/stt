// `syn::Pat` is non-exhaustive with many variants; wildcards are the point here.
#![allow(
    clippy::unwrap_used,
    clippy::missing_assert_message,
    clippy::wildcard_enum_match_arm
)]

use std::fs;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{ItemUse, Local, Pat, UseTree};

struct Violation {
    line: usize,
    problem: &'static str,
}

#[derive(Default)]
struct LetVisitor {
    violations: Vec<Violation>,
}

impl<'ast> Visit<'ast> for LetVisitor {
    fn visit_local(&mut self, node: &'ast Local) {
        let line: usize = node.span().start().line;
        if node
            .init
            .as_ref()
            .is_some_and(|init| init.diverge.is_some())
        {
            self.violations.push(Violation {
                line,
                problem: "let-else is not allowed, use if let or match",
            });
        }
        if let Some(problem) = pattern_problem(&node.pat) {
            self.violations.push(Violation { line, problem });
        }
        visit::visit_local(self, node);
    }
}

/// Finds `use` groups that reach into a submodule, like `use a::{b, c::D}`:
/// each `use` imports from one module (rustfmt's `imports_granularity = Module`).
#[derive(Default)]
struct UseVisitor {
    lines: Vec<usize>,
}

impl<'ast> Visit<'ast> for UseVisitor {
    fn visit_item_use(&mut self, node: &'ast ItemUse) {
        if has_nested_group_path(&node.tree, false) {
            self.lines.push(node.span().start().line);
        }
    }
}

fn has_nested_group_path(tree: &UseTree, in_group: bool) -> bool {
    match tree {
        UseTree::Path(path) => in_group || has_nested_group_path(&path.tree, false),
        UseTree::Group(group) => group
            .items
            .iter()
            .any(|item| has_nested_group_path(item, true)),
        UseTree::Name(_) | UseTree::Rename(_) | UseTree::Glob(_) => false,
    }
}

fn pattern_problem(pattern: &Pat) -> Option<&'static str> {
    match pattern {
        Pat::Wild(_) => None,
        Pat::Ident(pattern) if pattern.subpat.is_none() => Some("missing type annotation"),
        Pat::Type(typed) => match typed.pat.as_ref() {
            Pat::Wild(_) => None,
            Pat::Ident(pattern) if pattern.subpat.is_none() => None,
            _ => Some("destructuring is not allowed, bind a named type instead"),
        },
        _ => Some("destructuring is not allowed, bind a named type instead"),
    }
}

fn rust_files(directory: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut directories: Vec<PathBuf> = vec![directory.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).expect("read source directory") {
            let path: PathBuf = entry.expect("read source entry").path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files
}

#[test]
fn let_bindings_are_single_names_with_explicit_types() {
    let mut failures: Vec<String> = Vec::new();
    for path in rust_files(Path::new("src")) {
        let source: String = fs::read_to_string(&path).expect("read source file");
        let syntax: syn::File = syn::parse_file(&source).expect("parse source file");
        let mut visitor: LetVisitor = LetVisitor::default();
        visitor.visit_file(&syntax);
        for violation in visitor.violations {
            failures.push(format!(
                "{}:{}: {}",
                path.display(),
                violation.line,
                violation.problem
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "let bindings must be a single name with an explicit type:\n{}",
        failures.join("\n")
    );
}

#[test]
fn each_use_imports_from_one_module() {
    let mut failures: Vec<String> = Vec::new();
    let mut files: Vec<PathBuf> = rust_files(Path::new("src"));
    files.extend(rust_files(Path::new("tests")));
    for path in files {
        let source: String = fs::read_to_string(&path).expect("read source file");
        let syntax: syn::File = syn::parse_file(&source).expect("parse source file");
        let mut visitor: UseVisitor = UseVisitor::default();
        visitor.visit_file(&syntax);
        for line in visitor.lines {
            failures.push(format!("{}:{line}", path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "split `use a::{{b, c::D}}` into one `use` per module (`cargo +nightly fmt -- \
         --config imports_granularity=Module` does it):\n{}",
        failures.join("\n")
    );
}
