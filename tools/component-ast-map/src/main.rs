// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use cargo_metadata::{MetadataCommand, Package, PackageId};
use serde::Serialize;
use syn::{
    File, ItemUse, Path as SynPath, UseTree,
    visit::{self, Visit},
};
use walkdir::WalkDir;

#[derive(Debug)]
struct Cli {
    workspace_root: PathBuf,
    output: PathBuf,
    json_output: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
struct WorkspaceCrate {
    package_name: String,
    module_name: String,
    manifest_path: String,
    src_dir: String,
}

#[derive(Debug, Clone, Default, Serialize)]
struct ReferenceStats {
    node_count: usize,
    file_count: usize,
    files: Vec<String>,
    examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DependencyEdge {
    from: String,
    to: String,
    declared: bool,
    observed_in_ast: bool,
    ast_node_count: usize,
    ast_file_count: usize,
    example_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Report {
    workspace_root: String,
    crates: Vec<WorkspaceCrate>,
    edges: Vec<DependencyEdge>,
    reverse_dependencies: BTreeMap<String, Vec<String>>,
}

#[derive(Default)]
struct MutableReferenceStats {
    node_count: usize,
    files: BTreeSet<String>,
    examples: BTreeSet<String>,
}

impl MutableReferenceStats {
    fn record(&mut self, file: &str, example: String) {
        self.node_count += 1;
        self.files.insert(file.to_string());
        if self.examples.len() < 8 {
            self.examples.insert(example);
        }
    }

    fn freeze(self) -> ReferenceStats {
        ReferenceStats {
            node_count: self.node_count,
            file_count: self.files.len(),
            files: self.files.into_iter().collect(),
            examples: self.examples.into_iter().collect(),
        }
    }
}

struct AstReferenceCollector<'a> {
    workspace_modules: &'a BTreeMap<String, String>,
    current_module: &'a str,
    current_file: &'a str,
    hits: BTreeMap<String, MutableReferenceStats>,
}

impl<'a> AstReferenceCollector<'a> {
    fn record_path(&mut self, path: &SynPath) {
        if path.leading_colon.is_some() {
            return;
        }

        let Some(first) = path.segments.first() else {
            return;
        };
        let root = first.ident.to_string();
        if matches!(root.as_str(), "crate" | "self" | "super") || root == self.current_module {
            return;
        }

        let Some(target_package) = self.workspace_modules.get(&root) else {
            return;
        };

        let rendered = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>()
            .join("::");
        self.hits
            .entry(target_package.clone())
            .or_default()
            .record(self.current_file, rendered);
    }

    fn record_use_tree(&mut self, prefix: Vec<String>, tree: &UseTree) {
        match tree {
            UseTree::Path(node) => {
                let mut next_prefix = prefix;
                next_prefix.push(node.ident.to_string());
                self.record_use_tree(next_prefix, &node.tree);
            }
            UseTree::Name(node) => {
                let mut full_path = prefix;
                full_path.push(node.ident.to_string());
                self.record_use_path(full_path);
            }
            UseTree::Rename(node) => {
                let mut full_path = prefix;
                full_path.push(node.ident.to_string());
                self.record_use_path(full_path);
            }
            UseTree::Glob(_) => {
                self.record_use_path(prefix);
            }
            UseTree::Group(node) => {
                for item in &node.items {
                    self.record_use_tree(prefix.clone(), item);
                }
            }
        }
    }

    fn record_use_path(&mut self, full_path: Vec<String>) {
        let Some(root) = full_path.first() else {
            return;
        };
        if matches!(root.as_str(), "crate" | "self" | "super") || root == self.current_module {
            return;
        }
        let Some(target_package) = self.workspace_modules.get(root) else {
            return;
        };

        self.hits
            .entry(target_package.clone())
            .or_default()
            .record(self.current_file, full_path.join("::"));
    }
}

impl<'ast, 'a> Visit<'ast> for AstReferenceCollector<'a> {
    fn visit_item_use(&mut self, node: &'ast ItemUse) {
        self.record_use_tree(Vec::new(), &node.tree);
    }

    fn visit_path(&mut self, node: &'ast SynPath) {
        self.record_path(node);
        visit::visit_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.record_path(&node.path);
        visit::visit_macro(self, node);
    }
}

fn main() -> Result<()> {
    let cli = parse_cli(env::args().skip(1).collect())?;
    let report = build_report(&cli.workspace_root)?;
    let markdown = render_markdown(&report);

    if let Some(parent) = cli.output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output dir {}", parent.display()))?;
    }
    fs::write(&cli.output, markdown)
        .with_context(|| format!("failed to write {}", cli.output.display()))?;

    if let Some(json_output) = cli.json_output {
        if let Some(parent) = json_output.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create JSON output dir {}", parent.display())
            })?;
        }
        let body = serde_json::to_string_pretty(&report).context("failed to encode JSON report")?;
        fs::write(&json_output, body)
            .with_context(|| format!("failed to write {}", json_output.display()))?;
    }

    println!(
        "wrote component dependency map to {}",
        cli.output.display()
    );
    Ok(())
}

fn parse_cli(args: Vec<String>) -> Result<Cli> {
    let mut workspace_root = None;
    let mut output = None;
    let mut json_output = None;

    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--workspace-root" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    bail!("--workspace-root requires a value");
                };
                workspace_root = Some(PathBuf::from(value));
            }
            "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    bail!("--output requires a value");
                };
                output = Some(PathBuf::from(value));
            }
            "--json-output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    bail!("--json-output requires a value");
                };
                json_output = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                println!(
                    "Usage: component-ast-map --workspace-root <path> --output <markdown> [--json-output <json>]"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
        index += 1;
    }

    Ok(Cli {
        workspace_root: workspace_root.context("missing --workspace-root")?,
        output: output.context("missing --output")?,
        json_output,
    })
}

fn build_report(workspace_root: &Path) -> Result<Report> {
    let metadata = MetadataCommand::new()
        .current_dir(workspace_root)
        .exec()
        .context("failed to load cargo metadata")?;

    let workspace_members = metadata
        .workspace_members
        .iter()
        .cloned()
        .collect::<BTreeSet<PackageId>>();

    let packages = metadata
        .packages
        .iter()
        .filter(|package| workspace_members.contains(&package.id))
        .filter(|package| package.name != "component-ast-map")
        .collect::<Vec<_>>();

    let workspace_crates = packages
        .iter()
        .map(|package| workspace_crate_from_package(workspace_root, package))
        .collect::<Result<Vec<_>>>()?;

    let module_to_package = workspace_crates
        .iter()
        .map(|item| (item.module_name.clone(), item.package_name.clone()))
        .collect::<BTreeMap<_, _>>();

    let declared = declared_workspace_edges(&workspace_crates, &packages);
    let observed = observed_workspace_edges(workspace_root, &workspace_crates, &module_to_package)?;

    let mut all_pairs = BTreeSet::new();
    for (from, targets) in &declared {
        for to in targets {
            all_pairs.insert((from.clone(), to.clone()));
        }
    }
    for (from, targets) in &observed {
        for to in targets.keys() {
            all_pairs.insert((from.clone(), to.clone()));
        }
    }

    let edges = all_pairs
        .into_iter()
        .map(|(from, to)| {
            let declared_edge = declared
                .get(&from)
                .map(|targets| targets.contains(&to))
                .unwrap_or(false);
            let observed_stats = observed.get(&from).and_then(|targets| targets.get(&to));
            DependencyEdge {
                from,
                to,
                declared: declared_edge,
                observed_in_ast: observed_stats.is_some(),
                ast_node_count: observed_stats.map(|stats| stats.node_count).unwrap_or(0),
                ast_file_count: observed_stats.map(|stats| stats.file_count).unwrap_or(0),
                example_paths: observed_stats
                    .map(|stats| stats.examples.clone())
                    .unwrap_or_default(),
            }
        })
        .collect::<Vec<_>>();

    let mut reverse_dependencies = BTreeMap::new();
    for edge in &edges {
        reverse_dependencies
            .entry(edge.to.clone())
            .or_insert_with(Vec::new)
            .push(edge.from.clone());
    }
    for dependencies in reverse_dependencies.values_mut() {
        dependencies.sort();
        dependencies.dedup();
    }

    Ok(Report {
        workspace_root: workspace_root_label(workspace_root),
        crates: workspace_crates,
        edges,
        reverse_dependencies,
    })
}

fn workspace_crate_from_package(workspace_root: &Path, package: &Package) -> Result<WorkspaceCrate> {
    let manifest_path = PathBuf::from(package.manifest_path.as_str());
    let manifest_dir = manifest_path
        .parent()
        .context("package manifest has no parent directory")?;
    let src_dir = manifest_dir.join("src");

    Ok(WorkspaceCrate {
        package_name: package.name.clone(),
        module_name: package.name.replace('-', "_"),
        manifest_path: relative_display(workspace_root, &manifest_path),
        src_dir: relative_display(workspace_root, &src_dir),
    })
}

fn declared_workspace_edges(
    workspace_crates: &[WorkspaceCrate],
    packages: &[&Package],
) -> BTreeMap<String, BTreeSet<String>> {
    let package_names = workspace_crates
        .iter()
        .map(|item| item.package_name.clone())
        .collect::<BTreeSet<_>>();

    let mut declared = BTreeMap::new();
    for package in packages {
        let targets = package
            .dependencies
            .iter()
            .filter_map(|dependency| {
                package_names
                    .contains(&dependency.name)
                    .then(|| dependency.name.clone())
            })
            .collect::<BTreeSet<_>>();
        declared.insert(package.name.clone(), targets);
    }
    declared
}

fn observed_workspace_edges(
    workspace_root: &Path,
    workspace_crates: &[WorkspaceCrate],
    module_to_package: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, BTreeMap<String, ReferenceStats>>> {
    let mut report = BTreeMap::new();

    for crate_info in workspace_crates {
        let src_dir = workspace_root.join(&crate_info.src_dir);
        if !src_dir.exists() {
            report.insert(crate_info.package_name.clone(), BTreeMap::new());
            continue;
        }

        let mut per_target = BTreeMap::<String, MutableReferenceStats>::new();
        for entry in WalkDir::new(&src_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
        {
            let file_path = entry.path().to_path_buf();
            let relative_file = relative_display(workspace_root, &file_path);
            let source = fs::read_to_string(&file_path)
                .with_context(|| format!("failed to read {}", file_path.display()))?;
            let syntax: File = syn::parse_file(&source)
                .with_context(|| format!("failed to parse {}", file_path.display()))?;

            let mut collector = AstReferenceCollector {
                workspace_modules: module_to_package,
                current_module: &crate_info.module_name,
                current_file: &relative_file,
                hits: BTreeMap::new(),
            };
            collector.visit_file(&syntax);

            for (target, stats) in collector.hits {
                let entry = per_target.entry(target).or_default();
                entry.node_count += stats.node_count;
                entry.files.extend(stats.files);
                entry.examples.extend(stats.examples);
            }
        }

        report.insert(
            crate_info.package_name.clone(),
            per_target
                .into_iter()
                .map(|(target, stats)| (target, stats.freeze()))
                .collect(),
        );
    }

    Ok(report)
}

fn relative_display(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn workspace_root_label(workspace_root: &Path) -> String {
    if workspace_root == Path::new(".") {
        return ".".to_string();
    }

    workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| workspace_root.display().to_string())
}

fn render_markdown(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("# Component Dependency Map\n\n");
    out.push_str("This file is generated from the Rust workspace using `cargo metadata` plus a `syn` AST walk.\n\n");
    out.push_str("Generator:\n");
    out.push_str("- `tools/component-ast-map`\n");
    out.push_str("- Regenerate with `cargo run --manifest-path tools/component-ast-map/Cargo.toml -- --workspace-root . --output docs/component-dependency-map.md --json-output docs/component-dependency-map.json`\n");
    out.push_str(&format!("- Workspace root: `{}`\n\n", report.workspace_root));

    out.push_str("## Workspace Crates\n\n");
    out.push_str("| Crate | Module Path Root | Manifest | Source Dir |\n");
    out.push_str("| --- | --- | --- | --- |\n");
    for item in &report.crates {
        out.push_str(&format!(
            "| `{}` | `{}` | `{}` | `{}` |\n",
            item.package_name, item.module_name, item.manifest_path, item.src_dir
        ));
    }
    out.push('\n');

    out.push_str("## Dependency Edges\n\n");
    out.push_str("| From | To | Declared in Cargo | Seen in AST | AST Nodes | Files | Example Symbols |\n");
    out.push_str("| --- | --- | --- | --- | ---: | ---: | --- |\n");
    for edge in &report.edges {
        let examples = if edge.example_paths.is_empty() {
            "-".to_string()
        } else {
            edge.example_paths
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join("<br>")
        };
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} | {} | {} |\n",
            edge.from,
            edge.to,
            yes_no(edge.declared),
            yes_no(edge.observed_in_ast),
            edge.ast_node_count,
            edge.ast_file_count,
            examples
        ));
    }
    out.push('\n');

    out.push_str("## Reverse Dependencies\n\n");
    for crate_name in report.crates.iter().map(|item| item.package_name.as_str()) {
        let incoming = report
            .reverse_dependencies
            .get(crate_name)
            .cloned()
            .unwrap_or_default();
        out.push_str(&format!("### `{}`\n\n", crate_name));
        if incoming.is_empty() {
            out.push_str("- No workspace crate currently depends on this crate.\n\n");
        } else {
            for dependency in incoming {
                out.push_str(&format!("- `{}`\n", dependency));
            }
            out.push('\n');
        }
    }

    out.push_str("## Notes\n\n");
    out.push_str("- `Declared in Cargo` comes from workspace package manifests.\n");
    out.push_str("- `Seen in AST` comes from source-level references such as `use foo::...`, type paths, expression paths, and macro paths.\n");
    out.push_str("- This map is intended to keep the gateway-lite data plane and the future auth-broker sidecar loosely coupled.\n");

    out
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
