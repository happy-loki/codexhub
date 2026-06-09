use anyhow::Context;
use axum::{
    Json,
    extract::{Path as AxumPath, Query},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub(super) async fn plugin_legacy_list() -> Json<Vec<serde_json::Value>> {
    let installed = installed_plugin_ids();
    Json(
        local_plugin_catalog()
            .into_iter()
            .map(|plugin| {
                let id = plugin.config_id();
                json!({
                    "name": plugin.name,
                    "marketplace_name": plugin.marketplace,
                    "enabled": installed.contains(&id),
                })
            })
            .collect(),
    )
}

pub(super) async fn plugin_legacy_featured() -> Json<Vec<String>> {
    Json(
        local_plugin_catalog()
            .into_iter()
            .map(|plugin| plugin.config_id())
            .collect(),
    )
}

pub(super) async fn plugin_legacy_enable(
    AxumPath(plugin_id): AxumPath<String>,
) -> impl IntoResponse {
    match install_local_plugin(&plugin_id) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "id": plugin_id, "enabled": true })),
        ),
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": err.to_string() })),
        ),
    }
}

#[derive(Deserialize)]
pub(super) struct PluginPageQuery {
    scope: Option<String>,
}

pub(super) async fn plugin_catalog_page(
    Query(query): Query<PluginPageQuery>,
) -> Json<serde_json::Value> {
    if query
        .scope
        .as_deref()
        .is_some_and(|scope| scope.eq_ignore_ascii_case("WORKSPACE"))
    {
        return Json(empty_plugin_page_json());
    }
    Json(plugin_page_json(None))
}

pub(super) async fn plugin_empty_page() -> Json<serde_json::Value> {
    Json(empty_plugin_page_json())
}

pub(super) async fn plugin_installed_page() -> Json<serde_json::Value> {
    let installed = installed_plugin_ids();
    Json(plugin_page_json(Some(&installed)))
}

pub(super) async fn plugin_detail(AxumPath(plugin_id): AxumPath<String>) -> impl IntoResponse {
    let Some(plugin) = find_local_plugin_by_remote_id(&plugin_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "local plugin not found" })),
        )
            .into_response();
    };
    Json(plugin.to_remote_directory_item(installed_plugin_ids().contains(&plugin.config_id())))
        .into_response()
}

pub(super) async fn plugin_install(AxumPath(plugin_id): AxumPath<String>) -> impl IntoResponse {
    match install_local_plugin(&plugin_id) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "id": plugin_id, "enabled": true })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

fn plugin_page_json(installed_filter: Option<&BTreeSet<String>>) -> serde_json::Value {
    let installed = installed_plugin_ids();
    let plugins = local_plugin_catalog()
        .into_iter()
        .filter(|plugin| {
            installed_filter
                .map(|ids| ids.contains(&plugin.config_id()))
                .unwrap_or(true)
        })
        .map(|plugin| {
            let enabled = installed.contains(&plugin.config_id());
            if installed_filter.is_some() {
                plugin.to_remote_installed_item(enabled)
            } else {
                plugin.to_remote_directory_item(enabled)
            }
        })
        .collect::<Vec<_>>();
    json!({
        "plugins": plugins,
        "pagination": {
            "next_page_token": null
        }
    })
}

fn empty_plugin_page_json() -> serde_json::Value {
    json!({
        "plugins": [],
        "pagination": {
            "next_page_token": null
        }
    })
}

#[derive(Debug, Clone)]
struct LocalPluginCatalogEntry {
    name: String,
    marketplace: String,
    version: Option<String>,
    root: PathBuf,
    description: Option<String>,
    keywords: Vec<String>,
    interface: serde_json::Value,
    skills: Vec<LocalPluginSkill>,
}

#[derive(Debug, Clone)]
struct LocalPluginSkill {
    name: String,
    description: String,
}

impl LocalPluginCatalogEntry {
    fn config_id(&self) -> String {
        format!("{}@{}", self.name, self.marketplace)
    }

    fn remote_id(&self) -> String {
        format!("local~{}~{}", self.marketplace, self.name)
    }

    fn to_remote_directory_item(&self, enabled: bool) -> serde_json::Value {
        let release = self.release_json();
        json!({
            "id": self.remote_id(),
            "name": self.name,
            "scope": "GLOBAL",
            "installation_policy": if enabled { "INSTALLED_BY_DEFAULT" } else { "AVAILABLE" },
            "authentication_policy": "ON_USE",
            "status": "ENABLED",
            "release": release,
        })
    }

    fn to_remote_installed_item(&self, enabled: bool) -> serde_json::Value {
        let mut value = self.to_remote_directory_item(enabled);
        if let Some(object) = value.as_object_mut() {
            object.insert("enabled".to_string(), json!(enabled));
            object.insert(
                "disabled_skill_names".to_string(),
                serde_json::Value::Array(Vec::new()),
            );
        }
        value
    }

    fn release_json(&self) -> serde_json::Value {
        json!({
            "version": self.version,
            "display_name": interface_string(&self.interface, "displayName")
                .unwrap_or_else(|| self.name.clone()),
            "description": self.description.clone().unwrap_or_default(),
            "app_ids": [],
            "keywords": self.keywords,
            "interface": remote_release_interface(&self.interface),
            "skills": self.skills.iter().map(|skill| {
                json!({
                    "name": skill.name,
                    "description": skill.description,
                    "interface": {
                        "display_name": skill.name,
                        "short_description": skill.description,
                        "brand_color": interface_string(&self.interface, "brandColor"),
                        "default_prompt": null,
                        "icon_small_url": null,
                        "icon_large_url": null,
                    }
                })
            }).collect::<Vec<_>>(),
        })
    }
}

fn local_plugin_catalog() -> Vec<LocalPluginCatalogEntry> {
    let mut by_id = BTreeMap::<String, LocalPluginCatalogEntry>::new();
    for root in plugin_cache_roots() {
        let Ok(marketplaces) = fs::read_dir(&root) else {
            continue;
        };
        for marketplace in marketplaces.flatten() {
            let marketplace_path = marketplace.path();
            if !marketplace_path.is_dir() {
                continue;
            }
            let marketplace_name = marketplace.file_name().to_string_lossy().to_string();
            let Ok(plugin_dirs) = fs::read_dir(&marketplace_path) else {
                continue;
            };
            for plugin_dir in plugin_dirs.flatten() {
                let plugin_path = plugin_dir.path();
                if !plugin_path.is_dir() {
                    continue;
                }
                if let Some(entry) = newest_plugin_version_entry(&marketplace_name, &plugin_path) {
                    by_id.entry(entry.config_id()).or_insert(entry);
                }
            }
        }
    }
    by_id.into_values().collect()
}

fn newest_plugin_version_entry(
    marketplace_name: &str,
    plugin_path: &Path,
) -> Option<LocalPluginCatalogEntry> {
    if plugin_path
        .join(".codex-plugin")
        .join("plugin.json")
        .is_file()
    {
        return local_plugin_entry_from_root(marketplace_name, plugin_path.to_path_buf());
    }

    let mut versions = fs::read_dir(plugin_path)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join(".codex-plugin").join("plugin.json").is_file())
        .collect::<Vec<_>>();
    versions.sort();
    versions.reverse();
    versions
        .into_iter()
        .find_map(|root| local_plugin_entry_from_root(marketplace_name, root))
}

fn local_plugin_entry_from_root(
    marketplace_name: &str,
    root: PathBuf,
) -> Option<LocalPluginCatalogEntry> {
    let manifest_path = root.join(".codex-plugin").join("plugin.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(manifest_path).ok()?).ok()?;
    let name = manifest.get("name")?.as_str()?.trim().to_string();
    if name.is_empty() || manifest.get("apps").is_some() {
        return None;
    }
    let skills = load_local_plugin_skills(&root, &manifest);
    if skills.is_empty() {
        return None;
    }
    Some(LocalPluginCatalogEntry {
        name,
        marketplace: marketplace_name.to_string(),
        version: manifest
            .get("version")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        root,
        description: manifest
            .get("description")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        keywords: manifest
            .get("keywords")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        interface: manifest
            .get("interface")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
        skills,
    })
}

fn load_local_plugin_skills(root: &Path, manifest: &serde_json::Value) -> Vec<LocalPluginSkill> {
    let skills_path = manifest
        .get("skills")
        .and_then(|value| value.as_str())
        .map(|value| root.join(value.trim_start_matches("./")))
        .unwrap_or_else(|| root.join("skills"));
    let Ok(skill_dirs) = fs::read_dir(skills_path) else {
        return Vec::new();
    };
    let mut skills = skill_dirs
        .flatten()
        .filter_map(|entry| {
            let path = entry.path().join("SKILL.md");
            let contents = fs::read_to_string(path).ok()?;
            let name = yaml_front_matter_value(&contents, "name")
                .unwrap_or_else(|| entry.file_name().to_string_lossy().to_string());
            let description =
                yaml_front_matter_value(&contents, "description").unwrap_or_else(|| name.clone());
            Some(LocalPluginSkill { name, description })
        })
        .collect::<Vec<_>>();
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    skills
}

fn yaml_front_matter_value(contents: &str, key: &str) -> Option<String> {
    let mut lines = contents.lines();
    if lines.next()? != "---" {
        return None;
    }
    for line in lines {
        if line == "---" {
            break;
        }
        let Some((left, right)) = line.split_once(':') else {
            continue;
        };
        if left.trim() == key {
            return Some(
                right
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            )
            .filter(|value| !value.is_empty());
        }
    }
    None
}

fn remote_release_interface(interface: &serde_json::Value) -> serde_json::Value {
    json!({
        "short_description": interface_string(interface, "shortDescription"),
        "long_description": interface_string(interface, "longDescription"),
        "developer_name": interface_string(interface, "developerName"),
        "category": interface_string(interface, "category"),
        "capabilities": interface
            .get("capabilities")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        "website_url": interface_string(interface, "websiteURL")
            .or_else(|| interface_string(interface, "websiteUrl")),
        "privacy_policy_url": interface_string(interface, "privacyPolicyURL")
            .or_else(|| interface_string(interface, "privacyPolicyUrl")),
        "terms_of_service_url": interface_string(interface, "termsOfServiceURL")
            .or_else(|| interface_string(interface, "termsOfServiceUrl")),
        "brand_color": interface_string(interface, "brandColor"),
        "default_prompt": interface.get("defaultPrompt").and_then(|value| {
            value.as_array().map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }),
        "composer_icon_url": null,
        "logo_url": null,
        "screenshot_urls": [],
    })
}

fn interface_string(interface: &serde_json::Value, key: &str) -> Option<String> {
    interface
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn find_local_plugin_by_remote_id(remote_id: &str) -> Option<LocalPluginCatalogEntry> {
    local_plugin_catalog()
        .into_iter()
        .find(|plugin| plugin.remote_id() == remote_id || plugin.config_id() == remote_id)
}

fn install_local_plugin(plugin_id: &str) -> anyhow::Result<()> {
    let plugin = find_local_plugin_by_remote_id(plugin_id)
        .ok_or_else(|| anyhow::anyhow!("local plugin not found"))?;
    let codex_home = default_codex_home_for_web();
    let target = codex_home
        .join("plugins")
        .join("cache")
        .join(&plugin.marketplace)
        .join(&plugin.name)
        .join(plugin.version.as_deref().unwrap_or("local"));
    if plugin.root != target {
        if target.exists() {
            fs::remove_dir_all(&target)
                .with_context(|| format!("failed to replace plugin cache {}", target.display()))?;
        }
        copy_dir_all(&plugin.root, &target)?;
    }
    write_enabled_plugin_config(&codex_home.join("config.toml"), &plugin.config_id())?;
    Ok(())
}

fn installed_plugin_ids() -> BTreeSet<String> {
    let path = default_codex_home_for_web().join("config.toml");
    let Ok(raw) = fs::read_to_string(path) else {
        return BTreeSet::new();
    };
    let Ok(doc) = raw.parse::<toml_edit::DocumentMut>() else {
        return BTreeSet::new();
    };
    doc.get("plugins")
        .and_then(|item| item.as_table())
        .map(|plugins| {
            plugins
                .iter()
                .filter_map(|(id, item)| {
                    item.as_table()
                        .and_then(|table| table.get("enabled"))
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false)
                        .then(|| id.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_enabled_plugin_config(config_path: &Path, plugin_id: &str) -> anyhow::Result<()> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut doc = if config_path.is_file() {
        fs::read_to_string(config_path)?
            .parse::<toml_edit::DocumentMut>()
            .unwrap_or_else(|_| toml_edit::DocumentMut::new())
    } else {
        toml_edit::DocumentMut::new()
    };
    if !doc.contains_key("plugins") {
        doc["plugins"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let plugins = doc["plugins"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config plugins entry is not a table"))?;
    let mut plugin_table = toml_edit::Table::new();
    plugin_table["enabled"] = toml_edit::value(true);
    plugins[plugin_id] = toml_edit::Item::Table(plugin_table);
    fs::write(config_path, doc.to_string())?;
    Ok(())
}

fn plugin_cache_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        roots.push(
            Path::new(&home)
                .join(".codex")
                .join("plugins")
                .join("cache"),
        );
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        roots.push(
            Path::new(&local_app_data)
                .join("com.lokizhou.arthas")
                .join("codex")
                .join("plugins")
                .join("cache"),
        );
    }
    if let Some(root) = find_openai_bundled_plugins_root() {
        roots.push(root);
    }
    roots
}

fn default_codex_home_for_web() -> PathBuf {
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        Path::new(&home).join(".codex")
    } else {
        PathBuf::from(".codex")
    }
}

fn find_openai_bundled_plugins_root() -> Option<PathBuf> {
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from)?;
    let windows_apps = program_files.join("WindowsApps");
    let entries = fs::read_dir(windows_apps).ok()?;
    let mut roots = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("OpenAI.Codex_") {
                return None;
            }
            let root = entry.path().join("app").join("resources").join("plugins");
            root.join("openai-bundled")
                .join(".agents")
                .join("plugins")
                .join("marketplace.json")
                .is_file()
                .then_some(root)
        })
        .collect::<Vec<_>>();
    roots.sort();
    roots.pop()
}

fn copy_dir_all(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
