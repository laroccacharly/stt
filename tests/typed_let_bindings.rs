use std::fs;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Local, Pat};

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
