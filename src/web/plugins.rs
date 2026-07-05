use std::{fs, path::PathBuf, time::SystemTime};

use axum::{
    Json, Router,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

use crate::app_state::SharedState;
use crate::codex_app_config;

// Preferred ordering for locally usable curated plugins. Entries that are not
// present in the filtered local catalog (e.g. because they need a remote OpenAI
// backend) are simply skipped, so this list never surfaces unusable plugins.
const PREFERRED_LOCAL_FEATURED_PLUGINS: &[&str] = &[
    "superpowers",
    "remotion",
    "game-studio",
    "zotero",
    "codex-security",
    "sentry",
    "circleci",
    "render",
];
const MAX_FEATURED_PLUGINS: usize = 8;
const OPENAI_BUNDLED_MARKETPLACE: &str = "openai-bundled";
const OPENAI_CURATED_MARKETPLACE: &str = "openai-curated";
const OPENAI_CURATED_REMOTE_MARKETPLACE: &str = "openai-curated-remote";
const CODEXHUB_CURATED_REMOTE_ID_PREFIX: &str = "plugins~codexhub-local-";
const CODEXHUB_BUNDLED_REMOTE_ID_PREFIX: &str = "plugins~codexhub-bundled-";
const LOCAL_BUNDLED_REMOTE_ID_PREFIX: &str = "local~openai-bundled~";

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/backend-api/ps/plugins/list", get(list_plugins))
        .route("/backend-api/ps/plugins/installed", get(installed_plugins))
        .route("/backend-api/ps/plugins/suggested", get(suggested_plugins))
        .route(
            "/backend-api/ps/plugins/{plugin_id}/install",
            post(install_plugin),
        )
        .route(
            "/backend-api/ps/plugins/{plugin_id}/skills/{skill_name}",
            get(plugin_skill_detail),
        )
        .route("/backend-api/ps/plugins/{plugin_id}", get(plugin_detail))
        .route("/backend-api/plugins/featured", get(featured_plugins))
}

async fn list_plugins() -> Response {
    // codexhub intentionally serves an EMPTY remote plugin catalog here.
    //
    // The Codex desktop app merges two independent plugin sources:
    //   1. the on-disk local marketplace `openai-curated` (display name
    //      "Codex official"), read directly from
    //      `~/.codex/.tmp/plugins/.agents/plugins/marketplace.json`, and
    //   2. this HTTP "remote" catalog (`/backend-api/ps/plugins/*`).
    //
    // Everything this endpoint could return already lives in the local
    // marketplace, where installs go through the working `marketplacePath`
    // branch (materialized into `~/.codex/plugins/cache/openai-curated/...`).
    // When we also advertised the same plugins here, the app surfaced a second
    // "OpenAI Curated Remote" tab whose installs route through the remote
    // branch and fail with `MissingBundleDownloadUrl` because codexhub cannot
    // provide an HTTPS `bundle_download_url`. Returning an empty catalog drops
    // that broken duplicate tab and leaves the working local tab untouched.
    Json(empty_plugin_page()).into_response()
}

async fn installed_plugins() -> Json<Value> {
    let plugins = load_installed_remote_plugins();
    Json(json!({
        "plugins": plugins,
        "pagination": {
            "next_page_token": null
        }
    }))
}

async fn featured_plugins() -> Json<Value> {
    let plugins = load_local_curated_remote_plugins().unwrap_or_default();
    let featured = featured_plugin_names(&plugins)
        .into_iter()
        // Featured ids must match the LOCAL marketplace plugin ids
        // (`<name>@openai-curated`) so the app highlights entries in the
        // "Codex official" tab. The remote (`openai-curated-remote`) catalog is
        // no longer served, so featuring against it would highlight nothing.
        .map(|name| format!("{name}@{OPENAI_CURATED_MARKETPLACE}"))
        .collect::<Vec<_>>();
    Json(json!(featured))
}

async fn suggested_plugins() -> Response {
    match load_local_curated_remote_plugins() {
        Ok(plugins) => Json(json!({
            "enabled": true,
            "plugins": recommended_plugins(&plugins),
        }))
        .into_response(),
        Err(_) => Json(json!({
            "enabled": false,
            "plugins": [],
        }))
        .into_response(),
    }
}

async fn plugin_detail(Path(plugin_id): Path<String>) -> Response {
    match find_local_fallback_plugin_detail(&plugin_id) {
        Ok(Some(plugin)) => Json(plugin).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!("plugin {plugin_id} not found")
            })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": err
            })),
        )
            .into_response(),
    }
}

async fn install_plugin(Path(plugin_id): Path<String>) -> Response {
    match find_local_fallback_plugin_detail(&plugin_id) {
        Ok(Some(_)) => Json(json!({
            "id": plugin_id,
            "enabled": true,
            "app_ids_needing_auth": [],
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!("plugin {plugin_id} not found")
            })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": err
            })),
        )
            .into_response(),
    }
}

async fn plugin_skill_detail(Path((plugin_id, skill_name)): Path<(String, String)>) -> Response {
    match find_local_fallback_plugin_skill(&plugin_id, &skill_name) {
        Ok(Some(contents)) => Json(json!({
            "plugin_id": plugin_id,
            "name": skill_name,
            "skill_md_contents": contents,
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!("plugin skill {plugin_id}/{skill_name} not found")
            })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": err
            })),
        )
            .into_response(),
    }
}

fn empty_plugin_page() -> Value {
    json!({
        "plugins": [],
        "pagination": {
            "next_page_token": null
        }
    })
}

fn load_local_curated_remote_plugins() -> Result<Vec<Value>, String> {
    let path = curated_marketplace_path();
    let contents = std::fs::read_to_string(&path).map_err(|err| {
        format!(
            "failed to read local curated marketplace {}: {err}",
            path.display()
        )
    })?;
    let manifest: Value = serde_json::from_str(&contents).map_err(|err| {
        format!(
            "failed to parse local curated marketplace {}: {err}",
            path.display()
        )
    })?;
    let plugins = manifest
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "local curated marketplace {} does not contain plugins array",
                path.display()
            )
        })?;

    Ok(plugins
        .iter()
        .filter(|plugin| !curated_plugin_requires_remote_backend(plugin))
        .filter_map(|plugin| {
            local_marketplace_plugin_to_remote(
                plugin,
                OPENAI_CURATED_REMOTE_MARKETPLACE,
                CODEXHUB_CURATED_REMOTE_ID_PREFIX,
            )
        })
        .collect())
}

fn find_local_fallback_plugin_detail(plugin_id: &str) -> Result<Option<Value>, String> {
    if let Some(plugin) = find_local_bundled_compat_plugin(plugin_id)? {
        return Ok(Some(remote_directory_item_to_detail(plugin)));
    }
    Ok(find_local_curated_remote_plugin(plugin_id)?.map(remote_directory_item_to_detail))
}

fn find_local_fallback_plugin_skill(
    plugin_id: &str,
    skill_name: &str,
) -> Result<Option<String>, String> {
    let Some(plugin_name) = bundled_plugin_name_from_compat_id(plugin_id) else {
        return Ok(None);
    };

    read_local_bundled_skill(&plugin_name, skill_name)
}

fn load_installed_remote_plugins() -> Vec<Value> {
    installed_plugin_config_ids()
        .into_iter()
        .filter_map(|plugin_id| {
            let plugin = if plugin_id.ends_with("@openai-curated")
                || plugin_id.ends_with(&format!("@{OPENAI_CURATED_REMOTE_MARKETPLACE}"))
            {
                let plugin_name = plugin_id.split('@').next().unwrap_or_default();
                find_local_curated_remote_plugin(plugin_name).ok().flatten()
            } else {
                None
            }?;

            Some(installed_plugin_item(plugin))
        })
        .collect()
}

fn installed_plugin_item(mut plugin: Value) -> Value {
    if let Value::Object(map) = &mut plugin {
        map.insert("enabled".to_string(), Value::Bool(true));
        map.insert("disabled_skill_names".to_string(), Value::Array(Vec::new()));
    }
    plugin
}

fn installed_plugin_config_ids() -> Vec<String> {
    let config_path = codex_home().join("config.toml");
    let Ok(contents) = fs::read_to_string(config_path) else {
        return Vec::new();
    };
    let Ok(doc) = contents.parse::<toml_edit::DocumentMut>() else {
        return Vec::new();
    };
    let Some(plugins) = doc.get("plugins").and_then(|item| item.as_table()) else {
        return Vec::new();
    };

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
}

fn find_local_curated_remote_plugin(plugin_id: &str) -> Result<Option<Value>, String> {
    Ok(load_local_curated_remote_plugins()?
        .into_iter()
        .find(|plugin| plugin_matches_id(plugin, plugin_id, OPENAI_CURATED_REMOTE_MARKETPLACE)))
}

fn find_local_bundled_compat_plugin(plugin_id: &str) -> Result<Option<Value>, String> {
    let Some(plugin_name) = bundled_plugin_name_from_compat_id(plugin_id) else {
        return Ok(None);
    };
    let path = bundled_marketplace_path();
    let contents = fs::read_to_string(&path).map_err(|err| {
        format!(
            "failed to read local bundled marketplace {}: {err}",
            path.display()
        )
    })?;
    let manifest: Value = serde_json::from_str(&contents).map_err(|err| {
        format!(
            "failed to parse local bundled marketplace {}: {err}",
            path.display()
        )
    })?;
    let plugins = manifest
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "local bundled marketplace {} does not contain plugins array",
                path.display()
            )
        })?;

    Ok(plugins
        .iter()
        .find(|plugin| plugin.get("name").and_then(Value::as_str) == Some(plugin_name.as_str()))
        .and_then(|plugin| {
            let remote_id = bundled_compat_remote_id(plugin_id, &plugin_name);
            let plugin = local_marketplace_plugin_to_remote_with_id(
                plugin,
                OPENAI_BUNDLED_MARKETPLACE,
                Some(remote_id.as_str()),
                CODEXHUB_BUNDLED_REMOTE_ID_PREFIX,
            )?;
            Some(with_local_bundled_skills(plugin, &plugin_name))
        }))
}

fn bundled_plugin_name_from_compat_id(plugin_id: &str) -> Option<String> {
    plugin_id
        .strip_prefix(CODEXHUB_BUNDLED_REMOTE_ID_PREFIX)
        .map(str::to_string)
        .or_else(|| {
            plugin_id
                .strip_prefix(LOCAL_BUNDLED_REMOTE_ID_PREFIX)
                .map(str::to_string)
        })
        .or_else(|| {
            plugin_id
                .strip_suffix(&format!("@{OPENAI_BUNDLED_MARKETPLACE}"))
                .map(str::to_string)
        })
        .filter(|name| !name.is_empty())
}

fn bundled_compat_remote_id(plugin_id: &str, plugin_name: &str) -> String {
    if plugin_id.starts_with(CODEXHUB_BUNDLED_REMOTE_ID_PREFIX)
        || plugin_id.starts_with(LOCAL_BUNDLED_REMOTE_ID_PREFIX)
    {
        return plugin_id.to_string();
    }
    format!("{CODEXHUB_BUNDLED_REMOTE_ID_PREFIX}{plugin_name}")
}

fn read_local_bundled_skill(plugin_name: &str, skill_name: &str) -> Result<Option<String>, String> {
    if !is_safe_path_segment(plugin_name) || !is_safe_path_segment(skill_name) {
        return Ok(None);
    }

    for root in local_bundled_skill_root_candidate_paths(plugin_name)? {
        let path = root.join(skill_name).join("SKILL.md");
        if path.is_file() {
            return fs::read_to_string(&path).map(Some).map_err(|err| {
                format!(
                    "failed to read bundled plugin skill {}: {err}",
                    path.display()
                )
            });
        }
    }

    Ok(None)
}

fn local_bundled_skill_root_candidate_paths(plugin_name: &str) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let cache_root = codex_home()
        .join("plugins")
        .join("cache")
        .join(OPENAI_BUNDLED_MARKETPLACE)
        .join(plugin_name);

    if cache_root.is_dir() {
        let mut cached_paths = Vec::new();
        for entry in fs::read_dir(&cache_root).map_err(|err| {
            format!(
                "failed to read bundled plugin cache {}: {err}",
                cache_root.display()
            )
        })? {
            let entry = entry.map_err(|err| {
                format!(
                    "failed to read bundled plugin cache entry {}: {err}",
                    cache_root.display()
                )
            })?;
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }

            let skills_root = entry.path().join("skills");
            if !skills_root.is_dir() {
                continue;
            }

            let modified = skills_root
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            cached_paths.push((modified, skills_root));
        }
        cached_paths.sort_by(|left, right| right.0.cmp(&left.0));
        paths.extend(cached_paths.into_iter().map(|(_, path)| path));
    }

    paths.push(
        codex_home()
            .join(".tmp")
            .join("bundled-marketplaces")
            .join(OPENAI_BUNDLED_MARKETPLACE)
            .join("plugins")
            .join(plugin_name)
            .join("skills"),
    );

    Ok(paths)
}

fn with_local_bundled_skills(mut plugin: Value, plugin_name: &str) -> Value {
    let skills = local_bundled_skill_summaries(plugin_name);
    if skills.is_empty() {
        return plugin;
    }

    if let Some(release) = plugin.get_mut("release").and_then(Value::as_object_mut) {
        release.insert("skills".to_string(), Value::Array(skills));
    }

    plugin
}

fn local_bundled_skill_summaries(plugin_name: &str) -> Vec<Value> {
    if !is_safe_path_segment(plugin_name) {
        return Vec::new();
    }

    let Ok(roots) = local_bundled_skill_root_candidate_paths(plugin_name) else {
        return Vec::new();
    };

    for root in roots {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };

        let mut skills = entries
            .flatten()
            .filter_map(|entry| {
                if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                    return None;
                }

                let fallback_name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path().join("SKILL.md");
                let contents = fs::read_to_string(path).ok()?;
                let name = front_matter_value(&contents, "name").unwrap_or(fallback_name);
                let description =
                    front_matter_value(&contents, "description").unwrap_or_else(|| name.clone());
                let short_description = front_matter_value(&contents, "short_description")
                    .or_else(|| Some(description.clone()));

                Some(json!({
                    "name": name,
                    "description": description,
                    "interface": {
                        "display_name": name,
                        "short_description": short_description,
                        "brand_color": null,
                        "default_prompt": null,
                        "icon_small_url": null,
                        "icon_large_url": null
                    }
                }))
            })
            .collect::<Vec<_>>();

        if !skills.is_empty() {
            skills.sort_by(|left, right| {
                let left = left.get("name").and_then(Value::as_str).unwrap_or_default();
                let right = right
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                left.cmp(right)
            });
            return skills;
        }
    }

    Vec::new()
}

fn front_matter_value(contents: &str, key: &str) -> Option<String> {
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

fn is_safe_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn recommended_plugins(plugins: &[Value]) -> Vec<Value> {
    featured_plugin_names(plugins)
        .into_iter()
        .filter_map(|name| {
            plugins
                .iter()
                .find(|plugin| plugin.get("name").and_then(Value::as_str) == Some(name.as_str()))
        })
        .filter_map(recommended_plugin_item)
        .collect()
}

/// Chooses which locally usable plugins to feature, preferring the curated
/// ordering and then filling any remaining slots with the rest of the catalog.
fn featured_plugin_names(plugins: &[Value]) -> Vec<String> {
    let available = plugins
        .iter()
        .filter_map(|plugin| plugin.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();

    let mut ordered = Vec::new();
    for preferred in PREFERRED_LOCAL_FEATURED_PLUGINS {
        if available.iter().any(|name| name == preferred) {
            ordered.push((*preferred).to_string());
        }
    }
    for name in available {
        if ordered.len() >= MAX_FEATURED_PLUGINS {
            break;
        }
        if !ordered.iter().any(|existing| existing == &name) {
            ordered.push(name);
        }
    }
    ordered.truncate(MAX_FEATURED_PLUGINS);
    ordered
}

fn recommended_plugin_item(plugin: &Value) -> Option<Value> {
    let id = plugin.get("id")?.as_str()?;
    let name = plugin.get("name")?.as_str()?;
    let display_name = plugin
        .get("release")
        .and_then(|release| release.get("display_name"))
        .and_then(Value::as_str)
        .unwrap_or(name);

    Some(json!({
        "id": id,
        "name": name,
        "status": "ENABLED",
        "installation_policy": "AVAILABLE",
        "release": {
            "display_name": display_name,
            "app_ids": [],
        },
    }))
}

fn local_marketplace_plugin_to_remote(
    plugin: &Value,
    marketplace_name: &str,
    id_prefix: &str,
) -> Option<Value> {
    local_marketplace_plugin_to_remote_with_id(plugin, marketplace_name, None, id_prefix)
}

fn local_marketplace_plugin_to_remote_with_id(
    plugin: &Value,
    marketplace_name: &str,
    remote_id: Option<&str>,
    id_prefix: &str,
) -> Option<Value> {
    let name = plugin.get("name")?.as_str()?;
    let remote_id = remote_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("{id_prefix}{name}"));
    let interface = plugin.get("interface");
    let display_name = interface
        .and_then(|item| item.get("displayName").or_else(|| item.get("display_name")))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| display_name_from_slug(name));
    let short_description = interface
        .and_then(|item| {
            item.get("shortDescription")
                .or_else(|| item.get("short_description"))
        })
        .and_then(Value::as_str)
        .unwrap_or("Use this plugin with Codex");
    let category = plugin.get("category").and_then(Value::as_str).or_else(|| {
        interface
            .and_then(|item| item.get("category"))
            .and_then(Value::as_str)
    });
    let long_description = interface_string(interface, &["longDescription", "long_description"]);
    let developer_name = interface_string(interface, &["developerName", "developer_name"]);
    let website_url = interface_string(interface, &["websiteURL", "websiteUrl", "website_url"]);
    let privacy_policy_url = interface_string(
        interface,
        &["privacyPolicyURL", "privacyPolicyUrl", "privacy_policy_url"],
    );
    let terms_of_service_url = interface_string(
        interface,
        &[
            "termsOfServiceURL",
            "termsOfServiceUrl",
            "terms_of_service_url",
        ],
    );
    let brand_color = interface_string(interface, &["brandColor", "brand_color"]);
    let composer_icon_url = interface_string(
        interface,
        &["composerIconURL", "composerIconUrl", "composer_icon_url"],
    );
    let logo_url = interface_string(interface, &["logoURL", "logoUrl", "logo_url"]);
    let logo_url_dark =
        interface_string(interface, &["logoURLDark", "logoUrlDark", "logo_url_dark"]);

    Some(json!({
        "id": remote_id,
        "name": name,
        "scope": "GLOBAL",
        "installation_policy": "AVAILABLE",
        "authentication_policy": "ON_USE",
        "status": "ENABLED",
        "release": {
            "version": "local",
            "display_name": display_name,
            "description": short_description,
            "app_ids": [],
            "keywords": [],
            "interface": {
                "short_description": short_description,
                "long_description": long_description,
                "developer_name": developer_name,
                "category": category,
                "capabilities": string_array(interface, "capabilities"),
                "website_url": website_url,
                "privacy_policy_url": privacy_policy_url,
                "terms_of_service_url": terms_of_service_url,
                "brand_color": brand_color,
                "default_prompt": interface_string(interface, &["defaultPrompt", "default_prompt"]),
                "default_prompts": string_array_opt(interface, &["defaultPrompts", "default_prompts"]),
                "composer_icon_url": composer_icon_url,
                "logo_url": logo_url,
                "logo_url_dark": logo_url_dark,
                "screenshot_urls": string_array_any(interface, &["screenshotUrls", "screenshot_urls"])
            },
            "skills": []
        },
        "codexhub_marketplace_name": marketplace_name
    }))
}

fn interface_string(interface: Option<&Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        interface?
            .get(*key)
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn string_array(interface: Option<&Value>, key: &str) -> Vec<String> {
    string_array_any(interface, &[key])
}

fn string_array_opt(interface: Option<&Value>, keys: &[&str]) -> Option<Vec<String>> {
    let values = string_array_any(interface, keys);
    (!values.is_empty()).then_some(values)
}

fn string_array_any(interface: Option<&Value>, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .find_map(|key| {
            interface?.get(*key).and_then(Value::as_array).map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
        })
        .unwrap_or_default()
}

fn remote_directory_item_to_detail(plugin: Value) -> Value {
    plugin
}

fn plugin_matches_id(plugin: &Value, plugin_id: &str, marketplace_name: &str) -> bool {
    let id = plugin.get("id").and_then(Value::as_str);
    let name = plugin.get("name").and_then(Value::as_str);
    id == Some(plugin_id)
        || name == Some(plugin_id)
        || name
            .map(|name| format!("{name}@{marketplace_name}"))
            .as_deref()
            == Some(plugin_id)
}

fn curated_marketplace_path() -> PathBuf {
    codex_home()
        .join(".tmp")
        .join("plugins")
        .join(".agents")
        .join("plugins")
        .join("marketplace.json")
}

fn curated_marketplace_root() -> PathBuf {
    codex_home().join(".tmp").join("plugins")
}

/// Decides whether a curated-marketplace plugin depends on a remote backend
/// codexhub does not provide when running against a local gateway.
///
/// Two dependency kinds are treated as "remote-backed" and hidden from the
/// local plugin directory:
/// - `.app.json`: an OpenAI Apps/Connector whose tools live behind the ChatGPT
///   `codex_apps` MCP (Gmail, Google Drive, Linear, ...).
/// - `.mcp.json` with an HTTP/SSE transport: a hosted MCP served over the
///   network (e.g. `https://mcp.notion.com/mcp`).
///
/// Skill-only plugins and plugins whose `.mcp.json` launches a local stdio
/// process (`command`) stay visible because they work without the remote
/// OpenAI backend.
fn curated_plugin_requires_remote_backend(plugin: &Value) -> bool {
    let Some(dir) = curated_plugin_dir(plugin) else {
        return false;
    };
    plugin_dir_requires_remote_backend(&dir)
}

pub(crate) fn plugin_dir_requires_remote_backend(dir: &std::path::Path) -> bool {
    if dir.join(".app.json").is_file() {
        return true;
    }
    mcp_manifest_is_remote(&dir.join(".mcp.json"))
}

fn curated_plugin_dir(plugin: &Value) -> Option<PathBuf> {
    let raw_path = plugin
        .get("source")
        .and_then(|source| source.get("path"))
        .and_then(Value::as_str)?;
    let relative = raw_path.trim_start_matches("./").replace('\\', "/");
    if relative.is_empty() {
        return None;
    }

    let mut dir = curated_marketplace_root();
    for segment in relative.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return None,
            segment => dir.push(segment),
        }
    }
    Some(dir)
}

fn mcp_manifest_is_remote(path: &std::path::Path) -> bool {
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_str::<Value>(&contents) else {
        return false;
    };
    let Some(servers) = manifest.get("mcpServers").and_then(Value::as_object) else {
        return false;
    };

    servers.values().any(mcp_server_is_remote)
}

fn mcp_server_is_remote(server: &Value) -> bool {
    // A local stdio server is launched via `command`; anything else that only
    // exposes a network endpoint (`http`/`sse` transport, or a bare `url`) is
    // treated as remote.
    if server.get("command").and_then(Value::as_str).is_some() {
        return false;
    }
    if let Some(kind) = server.get("type").and_then(Value::as_str) {
        return matches!(kind, "http" | "sse" | "streamable-http" | "streamable_http");
    }
    server.get("url").and_then(Value::as_str).is_some()
}

fn bundled_marketplace_path() -> PathBuf {
    codex_home()
        .join(".tmp")
        .join("bundled-marketplaces")
        .join(OPENAI_BUNDLED_MARKETPLACE)
        .join(".agents")
        .join("plugins")
        .join("marketplace.json")
}

fn display_name_from_slug(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn codex_home() -> PathBuf {
    codex_app_config::default_codex_home()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after UNIX epoch")
            .as_nanos();
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "codexhub-plugins-test-{}-{}-{}",
            std::process::id(),
            nanos,
            sequence
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn app_json_plugin_is_remote_backed() {
        let dir = unique_temp_dir();
        std::fs::write(dir.join(".app.json"), r#"{"apps":{"gmail":{"id":"x"}}}"#)
            .expect("write app.json");
        assert!(plugin_dir_requires_remote_backend(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn http_mcp_plugin_is_remote_backed() {
        let dir = unique_temp_dir();
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"notion":{"type":"http","url":"https://mcp.notion.com/mcp"}}}"#,
        )
        .expect("write mcp.json");
        assert!(plugin_dir_requires_remote_backend(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stdio_mcp_plugin_is_local() {
        let dir = unique_temp_dir();
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"xcode":{"command":"npx","args":["-y","xcodebuildmcp@latest"]}}}"#,
        )
        .expect("write mcp.json");
        assert!(!plugin_dir_requires_remote_backend(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn skill_only_plugin_is_local() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(dir.join("skills")).expect("create skills dir");
        assert!(!plugin_dir_requires_remote_backend(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bare_url_mcp_without_command_is_remote() {
        let dir = unique_temp_dir();
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"svc":{"url":"https://mcp.example.com/mcp"}}}"#,
        )
        .expect("write mcp.json");
        assert!(plugin_dir_requires_remote_backend(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn featured_names_prefer_local_ordering_then_fill() {
        let plugins = vec![
            json!({ "name": "remotion" }),
            json!({ "name": "aardvark-tool" }),
            json!({ "name": "superpowers" }),
        ];
        let featured = featured_plugin_names(&plugins);
        assert_eq!(featured.first().map(String::as_str), Some("superpowers"));
        assert!(featured.iter().any(|name| name == "remotion"));
        assert!(featured.iter().any(|name| name == "aardvark-tool"));
        assert!(featured.len() <= MAX_FEATURED_PLUGINS);
    }

    #[test]
    fn curated_plugin_dir_rejects_parent_traversal() {
        let plugin = json!({ "source": { "path": "./plugins/../secret" } });
        assert!(curated_plugin_dir(&plugin).is_none());
    }

    #[test]
    fn curated_plugin_dir_resolves_relative_path() {
        let plugin = json!({ "source": { "path": "./plugins/remotion" } });
        let dir = curated_plugin_dir(&plugin).expect("dir");
        let tail = dir
            .components()
            .rev()
            .take(2)
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(tail, vec!["remotion".to_string(), "plugins".to_string()]);
    }

    #[tokio::test]
    async fn list_plugins_returns_empty_remote_catalog() {
        // The remote catalog must stay empty so the desktop app does not render
        // a duplicate "OpenAI Curated Remote" tab whose installs fail. Plugins
        // are surfaced only via the local `openai-curated` marketplace.
        let response = list_plugins().await;
        let (parts, body) = response.into_parts();
        assert_eq!(parts.status, StatusCode::OK);
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("read body");
        let value: Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(
            value
                .get("plugins")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
        assert!(value.get("pagination").is_some());
    }

    #[tokio::test]
    async fn featured_ids_target_local_curated_marketplace() {
        // Featured ids must reference the LOCAL marketplace suffix so the app
        // highlights entries in the "Codex official" tab, not the retired
        // remote catalog.
        let response = featured_plugins().await;
        let Json(value) = response;
        if let Some(ids) = value.as_array() {
            for id in ids {
                let id = id.as_str().expect("featured id string");
                assert!(
                    id.ends_with(&format!("@{OPENAI_CURATED_MARKETPLACE}")),
                    "featured id {id} must target the local curated marketplace"
                );
                assert!(
                    !id.ends_with(&format!("@{OPENAI_CURATED_REMOTE_MARKETPLACE}")),
                    "featured id {id} must not target the retired remote catalog"
                );
            }
        }
    }
}
