//! Keep the dependency allowlist in docs/architecture.md executable, without network.
use std::{collections::BTreeMap, path::Path};
use toml::Value;

fn manifest(path: &Path) -> Value {
    toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn documented_dependencies(root: &Path) -> BTreeMap<String, Vec<String>> {
    let document = std::fs::read_to_string(root.join("docs/architecture.md")).unwrap();
    let mut allowed = BTreeMap::new();
    for line in document
        .lines()
        .skip_while(|line| *line != "## Capas")
        .skip(1)
    {
        if line.starts_with("## ") {
            break;
        }
        let columns: Vec<_> = line.split('|').map(str::trim).collect();
        if columns.len() != 6 || !columns[2].starts_with("`rad-") {
            continue;
        }
        let name = columns[2].split('`').nth(1).unwrap().to_owned();
        let dependencies = if columns[4].starts_with("todas las anteriores") {
            allowed.keys().cloned().collect()
        } else if columns[4] == "ninguna" {
            Vec::new()
        } else {
            // Parenthetical roadmap notes are not extra allowed dependencies.
            columns[4]
                .split(" (")
                .next()
                .unwrap()
                .split(',')
                .map(|name| format!("rad-{}", name.trim()))
                .collect()
        };
        assert!(allowed.insert(name, dependencies).is_none());
    }
    assert!(allowed.len() >= 19, "architecture table was not parsed");
    allowed
}

fn check_workspace(root: &Path, workspace: &Path, allowed: &BTreeMap<String, Vec<String>>) {
    let workspace_manifest = manifest(&workspace.join("Cargo.toml"));
    for member in workspace_manifest["workspace"]["members"]
        .as_array()
        .unwrap()
    {
        let path = workspace
            .join(member.as_str().unwrap())
            .join("Cargo.toml")
            .canonicalize()
            .unwrap();
        assert!(
            path.starts_with(root),
            "workspace member outside repository"
        );
        let document = manifest(&path);
        let name = document["package"]["name"].as_str().unwrap();
        let extension = workspace != root && name != "rad-plugin-sdk";
        let permitted = if extension {
            vec!["rad-plugin-sdk".into(), "rad-plugin-contract".into()]
        } else {
            allowed
                .get(name)
                .unwrap_or_else(|| panic!("undocumented package: {name}"))
                .clone()
        };
        let mut tables = Vec::new();
        if let Some(table) = document.get("dependencies").and_then(Value::as_table) {
            tables.push(table);
        }
        if let Some(targets) = document.get("target").and_then(Value::as_table) {
            for target in targets.values() {
                if let Some(table) = target.get("dependencies").and_then(Value::as_table) {
                    tables.push(table);
                }
            }
        }
        for table in tables {
            for (alias, specification) in table {
                assert_ne!(name, "rad-core", "rad-core must have no dependencies");
                let specification =
                    if specification.get("workspace").and_then(Value::as_bool) == Some(true) {
                        &workspace_manifest["workspace"]["dependencies"][alias]
                    } else {
                        specification
                    };
                let dependency = specification
                    .get("package")
                    .and_then(Value::as_str)
                    .unwrap_or(alias);
                let local = if let Some(relative) =
                    specification.get("path").and_then(Value::as_str)
                {
                    let directory =
                        if table[alias].get("workspace").and_then(Value::as_bool) == Some(true) {
                            workspace
                        } else {
                            path.parent().unwrap()
                        };
                    let local_manifest = directory
                        .join(relative)
                        .join("Cargo.toml")
                        .canonicalize()
                        .unwrap();
                    assert!(
                        local_manifest.starts_with(root),
                        "dependency outside repository: {name}"
                    );
                    Some(
                        manifest(&local_manifest)["package"]["name"]
                            .as_str()
                            .unwrap()
                            .to_owned(),
                    )
                } else if dependency.starts_with("rad-") {
                    Some(dependency.to_owned())
                } else {
                    None
                };
                if let Some(local) = local {
                    assert!(
                        permitted.contains(&local),
                        "undocumented dependency: {name} -> {local}"
                    );
                    let adapters = [
                        "rad-store",
                        "rad-credentials",
                        "rad-package",
                        "rad-plugin-manager",
                        "rad-plugin-host",
                        "rad-transport",
                        "rad-deck",
                        "rad-launcher",
                    ];
                    assert!(
                        !(adapters.contains(&name) && adapters.contains(&local.as_str())),
                        "adapters must not compose each other: {name} -> {local}"
                    );
                    if matches!(name, "rad-client" | "rad-editor" | "rad-tray") {
                        assert!(
                            [
                                "rad-wire",
                                "rad-api",
                                "rad-client",
                                "rad-plugin-contract",
                                "rad-update"
                            ]
                            .contains(&local.as_str()),
                            "client depends on implementation: {name} -> {local}"
                        );
                    }
                    if workspace != root {
                        assert!(
                            ["rad-plugin-sdk", "rad-plugin-contract"].contains(&local.as_str()),
                            "extension depends on host implementation: {name} -> {local}"
                        );
                    }
                }
                if matches!(
                    name,
                    "rad-api"
                        | "rad-wire"
                        | "rad-plugin-contract"
                        | "rad-plugin-package"
                        | "rad-media"
                        | "rad-codec"
                        | "rad-authority"
                ) {
                    assert!(
                        !["rusqlite", "mio", "libc", "ureq", "gio", "glib", "eframe"]
                            .contains(&dependency),
                        "I/O in pure layer: {name} -> {dependency}"
                    );
                }
            }
        }
    }
}

#[test]
fn workspace_dependencies_follow_documented_layers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let allowed = documented_dependencies(&root);
    check_workspace(&root, &root, &allowed);
    check_workspace(&root, &root.join("plugins"), &allowed);
}
