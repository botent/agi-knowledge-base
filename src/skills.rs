//! Skill import/load support for `/skills import` and prompt injection.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use directories::BaseDirs;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde::{Deserialize, Serialize};
use url::Url;

const GITHUB_API_BASE: &str = "https://api.github.com";
const RAW_GITHUB_BASE: &str = "https://raw.githubusercontent.com";
const REGISTRY_FILENAME: &str = "skills_registry.json";
const MAX_SKILLS_IN_PROMPT: usize = 3;
const MAX_PROMPT_CHARS: usize = 12_000;
const MAX_SKILL_FILE_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportedSkillMeta {
    pub name: String,
    pub title: String,
    pub description: String,
    pub source_url: String,
    pub repository: String,
    pub repository_ref: String,
    pub repository_path: String,
    pub installed_at_utc: String,
}

#[derive(Clone, Debug)]
pub struct LoadedSkill {
    pub meta: ImportedSkillMeta,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct ImportResult {
    pub meta: ImportedSkillMeta,
    pub destination: PathBuf,
    pub file_count: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct SkillRegistry {
    skills: Vec<ImportedSkillMeta>,
}

#[derive(Clone, Debug)]
enum SkillSource {
    SkillsSh {
        owner: String,
        repo: String,
        slug: String,
        source_url: String,
    },
    GitHubPath {
        owner: String,
        repo: String,
        ref_name: Option<String>,
        path: String,
        source_url: String,
    },
}

#[derive(Clone, Debug, Deserialize)]
struct GitHubRepoInfo {
    default_branch: String,
}

#[derive(Clone, Debug, Deserialize)]
struct GitHubTreeResponse {
    tree: Vec<GitHubTreeItem>,
}

#[derive(Clone, Debug, Deserialize)]
struct GitHubTreeItem {
    path: String,
    #[serde(rename = "type")]
    item_type: String,
}

pub async fn import_skill(reference: &str) -> Result<ImportResult> {
    let source = parse_source(reference)?;
    let (owner, repo) = match &source {
        SkillSource::SkillsSh { owner, repo, .. } => (owner.clone(), repo.clone()),
        SkillSource::GitHubPath { owner, repo, .. } => (owner.clone(), repo.clone()),
    };

    let client = github_client()?;
    let repo_info = fetch_repo_info(&client, &owner, &repo).await?;
    let requested_ref = match &source {
        SkillSource::GitHubPath { ref_name, .. } => ref_name.clone(),
        SkillSource::SkillsSh { .. } => None,
    };
    let repo_ref = requested_ref.unwrap_or(repo_info.default_branch);

    let tree = fetch_repo_tree(&client, &owner, &repo, &repo_ref).await?;
    let skill_dir = match &source {
        SkillSource::SkillsSh { slug, .. } => resolve_skills_sh_dir(&tree, slug)?,
        SkillSource::GitHubPath { path, .. } => resolve_github_dir(&tree, path)?,
    };
    let skill_name = skill_dir
        .split('/')
        .next_back()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("Unable to infer skill name from path: {skill_dir}"))?
        .to_string();

    let skill_files = collect_skill_files(&tree, &skill_dir);
    if skill_files.is_empty() {
        bail!("No files found under skill directory: {skill_dir}");
    }
    if !skill_files.iter().any(|path| path.ends_with("/SKILL.md")) {
        bail!("Skill directory does not contain SKILL.md: {skill_dir}");
    }

    ensure_memini_skills_root()?;
    let destination = memini_skills_root().join(&skill_name);
    if destination.exists() {
        bail!(
            "Skill '{}' already exists at {}",
            skill_name,
            destination.display()
        );
    }

    fs::create_dir_all(&destination).with_context(|| {
        format!(
            "Create destination directory for skill at {}",
            destination.display()
        )
    })?;

    let copy_result = copy_skill_files_from_github(
        &client,
        &owner,
        &repo,
        &repo_ref,
        &skill_dir,
        &skill_files,
        &destination,
    )
    .await;

    if let Err(err) = copy_result {
        let _ = fs::remove_dir_all(&destination);
        return Err(err);
    }

    let installed_skill_md = destination.join("SKILL.md");
    let skill_md = fs::read_to_string(&installed_skill_md)
        .with_context(|| format!("Read {}", installed_skill_md.display()))?;
    let (title_opt, desc_opt) = parse_frontmatter(&skill_md);
    let title = title_opt.unwrap_or_else(|| skill_name.clone());
    let description = desc_opt.unwrap_or_else(|| infer_description(&skill_md));
    let source_url = match &source {
        SkillSource::SkillsSh { source_url, .. } => source_url.clone(),
        SkillSource::GitHubPath { source_url, .. } => source_url.clone(),
    };

    let meta = ImportedSkillMeta {
        name: skill_name.clone(),
        title,
        description,
        source_url,
        repository: format!("https://github.com/{owner}/{repo}"),
        repository_ref: repo_ref,
        repository_path: skill_dir,
        installed_at_utc: Utc::now().to_rfc3339(),
    };

    let mut registry = load_registry()?;
    registry
        .skills
        .retain(|existing| existing.name != meta.name);
    registry.skills.push(meta.clone());
    save_registry(&registry)?;

    Ok(ImportResult {
        meta,
        destination,
        file_count: skill_files.len(),
    })
}

pub fn load_imported_skills() -> Result<Vec<LoadedSkill>> {
    ensure_memini_skills_root()?;
    let registry = load_registry()?;
    if registry.skills.is_empty() {
        return Ok(Vec::new());
    }

    let mut loaded = Vec::new();
    for mut meta in registry.skills {
        let skill_md_path = memini_skills_root().join(&meta.name).join("SKILL.md");
        if !skill_md_path.exists() {
            continue;
        }

        let content = match fs::read_to_string(&skill_md_path) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if meta.description.trim().is_empty() {
            let (_, desc_opt) = parse_frontmatter(&content);
            meta.description = desc_opt.unwrap_or_else(|| infer_description(&content));
        }

        loaded.push(LoadedSkill { meta, content });
    }

    loaded.sort_by(|a, b| b.meta.installed_at_utc.cmp(&a.meta.installed_at_utc));
    Ok(loaded)
}

pub fn build_prompt_context(skills: &[LoadedSkill], query: &str) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let selected = select_relevant_skills(skills, query);
    if selected.is_empty() {
        return String::new();
    }

    let mut lines = Vec::new();
    lines.push("Imported skills are available for this task.".to_string());
    lines.push(
        "When a request matches a skill, follow that SKILL.md workflow and constraints."
            .to_string(),
    );

    for skill in selected {
        lines.push(String::new());
        lines.push(format!("Skill: {} ({})", skill.meta.title, skill.meta.name));
        if !skill.meta.description.trim().is_empty() {
            lines.push(format!("Description: {}", skill.meta.description.trim()));
        }
        lines.push(format!("Source: {}", skill.meta.source_url));
        lines.push("Instructions:".to_string());
        lines.push(trim_chars(skill.content.trim(), 3000));
    }

    trim_chars(&lines.join("\n"), MAX_PROMPT_CHARS)
}

fn select_relevant_skills<'a>(skills: &'a [LoadedSkill], query: &str) -> Vec<&'a LoadedSkill> {
    if skills.is_empty() {
        return Vec::new();
    }

    let terms = tokenize(query);
    let mut scored: Vec<(usize, usize)> = skills
        .iter()
        .enumerate()
        .map(|(idx, skill)| {
            if terms.is_empty() {
                return (0usize, idx);
            }
            let content_preview = trim_chars(&skill.content.to_lowercase(), 1200);
            let haystack = format!(
                "{} {} {}",
                skill.meta.name.to_lowercase(),
                skill.meta.description.to_lowercase(),
                content_preview
            );
            let score = terms.iter().filter(|term| haystack.contains(*term)).count();
            (score, idx)
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    let mut selected = Vec::new();
    for (score, idx) in &scored {
        if *score == 0 {
            continue;
        }
        selected.push(&skills[*idx]);
        if selected.len() >= MAX_SKILLS_IN_PROMPT {
            break;
        }
    }

    if selected.is_empty() {
        for (_, idx) in scored.into_iter().take(MAX_SKILLS_IN_PROMPT) {
            selected.push(&skills[idx]);
        }
    }

    selected
}

fn tokenize(input: &str) -> Vec<String> {
    input
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|word| {
            let lower = word.to_lowercase();
            if lower.len() >= 3 { Some(lower) } else { None }
        })
        .collect()
}

fn trim_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let truncated: String = input.chars().take(max_chars).collect();
    format!("{truncated}\n...[truncated]")
}

fn parse_source(reference: &str) -> Result<SkillSource> {
    let mut normalized = reference.trim().to_string();
    if normalized.is_empty() {
        bail!("Missing skill source. Usage: /skills import <skills.sh-url>");
    }
    if !normalized.contains("://") {
        normalized = format!("https://{normalized}");
    }
    let parsed = Url::parse(&normalized).context("Parse skill source URL")?;
    let host = parsed.host_str().unwrap_or_default().to_lowercase();
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|it| it.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();

    if host == "skills.sh" || host == "www.skills.sh" {
        if segments.len() < 3 {
            bail!("skills.sh URL must be in the format: https://skills.sh/<owner>/<repo>/<skill>");
        }
        return Ok(SkillSource::SkillsSh {
            owner: segments[0].to_string(),
            repo: segments[1].to_string(),
            slug: segments[2].to_string(),
            source_url: normalized,
        });
    }

    if host == "github.com" || host == "www.github.com" {
        if segments.len() < 2 {
            bail!("GitHub URL must include owner/repo.");
        }
        let owner = segments[0].to_string();
        let repo = segments[1].trim_end_matches(".git").to_string();
        if segments.len() == 2 {
            bail!(
                "GitHub URL must include a skill path. Example: https://github.com/<owner>/<repo>/tree/main/path/to/skill"
            );
        }

        if segments[2] == "tree" || segments[2] == "blob" {
            if segments.len() < 5 {
                bail!("GitHub tree/blob URL must include a ref and path.");
            }
            return Ok(SkillSource::GitHubPath {
                owner,
                repo,
                ref_name: Some(segments[3].to_string()),
                path: segments[4..].join("/"),
                source_url: normalized,
            });
        }

        return Ok(SkillSource::GitHubPath {
            owner,
            repo,
            ref_name: None,
            path: segments[2..].join("/"),
            source_url: normalized,
        });
    }

    bail!("Unsupported source. Use a skills.sh URL or GitHub URL.")
}

fn github_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .build()
        .context("Create HTTP client")
}

async fn fetch_repo_info(
    client: &reqwest::Client,
    owner: &str,
    repo: &str,
) -> Result<GitHubRepoInfo> {
    let url = format!("{GITHUB_API_BASE}/repos/{owner}/{repo}");
    let response = client
        .get(url)
        .header(USER_AGENT, "memini-skill-import")
        .header(ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .context("Request repository metadata from GitHub")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("GitHub repository lookup failed ({status}): {body}");
    }
    response
        .json::<GitHubRepoInfo>()
        .await
        .context("Parse GitHub repository metadata")
}

async fn fetch_repo_tree(
    client: &reqwest::Client,
    owner: &str,
    repo: &str,
    repo_ref: &str,
) -> Result<Vec<GitHubTreeItem>> {
    let encoded_ref: String = url::form_urlencoded::byte_serialize(repo_ref.as_bytes()).collect();
    let url = format!("{GITHUB_API_BASE}/repos/{owner}/{repo}/git/trees/{encoded_ref}?recursive=1");
    let response = client
        .get(url)
        .header(USER_AGENT, "memini-skill-import")
        .header(ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .context("Request repository tree from GitHub")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("GitHub repository tree lookup failed ({status}): {body}");
    }
    let parsed = response
        .json::<GitHubTreeResponse>()
        .await
        .context("Parse GitHub repository tree")?;
    Ok(parsed.tree)
}

fn resolve_skills_sh_dir(tree: &[GitHubTreeItem], slug: &str) -> Result<String> {
    let slug_lower = slug.to_lowercase();
    let mut exact = Vec::new();
    let mut fallback = Vec::new();

    for item in tree {
        if item.item_type != "blob" || !item.path.ends_with("/SKILL.md") {
            continue;
        }
        let Some(dir) = item.path.strip_suffix("/SKILL.md") else {
            continue;
        };
        let basename = dir
            .split('/')
            .next_back()
            .unwrap_or_default()
            .to_lowercase();
        if basename == slug_lower {
            exact.push(dir.to_string());
        } else if dir.to_lowercase().contains(&format!("/{slug_lower}")) {
            fallback.push(dir.to_string());
        }
    }

    if !exact.is_empty() {
        exact.sort_by_key(|d| d.len());
        return Ok(exact[0].clone());
    }
    if !fallback.is_empty() {
        fallback.sort_by_key(|d| d.len());
        return Ok(fallback[0].clone());
    }

    bail!(
        "Could not locate skill '{}' in the repository tree. Expected a directory ending with /{}/SKILL.md",
        slug,
        slug
    )
}

fn resolve_github_dir(tree: &[GitHubTreeItem], raw_path: &str) -> Result<String> {
    let path = raw_path.trim_matches('/').to_string();
    if path.is_empty() {
        bail!("GitHub skill path cannot be empty.");
    }

    if path.ends_with("SKILL.md") {
        let dir = path.trim_end_matches("SKILL.md").trim_end_matches('/');
        if dir.is_empty() {
            bail!("Root-level SKILL.md is not supported for import.");
        }
        return Ok(dir.to_string());
    }

    let required = format!("{path}/SKILL.md");
    let has_skill_md = tree
        .iter()
        .any(|item| item.item_type == "blob" && item.path == required);
    if has_skill_md {
        return Ok(path);
    }

    bail!("Path '{path}' does not contain SKILL.md in this repository/ref.")
}

fn collect_skill_files(tree: &[GitHubTreeItem], skill_dir: &str) -> Vec<String> {
    let prefix = format!("{skill_dir}/");
    let mut files: Vec<String> = tree
        .iter()
        .filter(|item| item.item_type == "blob" && item.path.starts_with(&prefix))
        .map(|item| item.path.clone())
        .collect();
    files.sort();
    files
}

async fn copy_skill_files_from_github(
    client: &reqwest::Client,
    owner: &str,
    repo: &str,
    repo_ref: &str,
    skill_dir: &str,
    skill_files: &[String],
    destination: &Path,
) -> Result<()> {
    let prefix = format!("{skill_dir}/");
    for repo_path in skill_files {
        let relative = repo_path
            .strip_prefix(&prefix)
            .ok_or_else(|| anyhow!("Unexpected file path outside skill directory: {repo_path}"))?;
        ensure_safe_relative_path(relative)?;

        let encoded_ref: String =
            url::form_urlencoded::byte_serialize(repo_ref.as_bytes()).collect();
        let encoded_path = repo_path
            .split('/')
            .map(|segment| {
                url::form_urlencoded::byte_serialize(segment.as_bytes()).collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("/");
        let raw_url = format!("{RAW_GITHUB_BASE}/{owner}/{repo}/{encoded_ref}/{encoded_path}");

        let response = client
            .get(&raw_url)
            .header(USER_AGENT, "memini-skill-import")
            .send()
            .await
            .with_context(|| format!("Download skill file from {raw_url}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Failed to download {repo_path} ({status}): {body}");
        }
        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("Read bytes from {raw_url}"))?;
        if bytes.len() > MAX_SKILL_FILE_BYTES {
            bail!("File too large to import: {repo_path}");
        }

        let dest_file = destination.join(relative);
        if let Some(parent) = dest_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Create directory {}", parent.display()))?;
        }
        fs::write(&dest_file, &bytes).with_context(|| format!("Write {}", dest_file.display()))?;
    }
    Ok(())
}

fn ensure_safe_relative_path(path: &str) -> Result<()> {
    let rel = Path::new(path);
    if rel.is_absolute() {
        bail!("Skill file path cannot be absolute: {path}");
    }
    for comp in rel.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            bail!("Skill file path cannot contain '..': {path}");
        }
    }
    Ok(())
}

fn parse_frontmatter(markdown: &str) -> (Option<String>, Option<String>) {
    let normalized = markdown.replace("\r\n", "\n");
    if !normalized.starts_with("---\n") {
        return (None, None);
    }

    let rest = &normalized[4..];
    let Some(frontmatter_end) = rest.find("\n---\n") else {
        return (None, None);
    };
    let frontmatter = &rest[..frontmatter_end];

    let mut name = None;
    let mut description = None;
    let mut short_description = None;

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("name:") {
            name = Some(value.trim().trim_matches('"').to_string());
        }
        if let Some(value) = trimmed.strip_prefix("description:") {
            description = Some(value.trim().trim_matches('"').to_string());
        }
        if let Some(value) = trimmed.strip_prefix("short-description:") {
            short_description = Some(value.trim().trim_matches('"').to_string());
        }
    }

    let final_description = description.or(short_description);
    (name, final_description)
}

fn infer_description(markdown: &str) -> String {
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#')
            || trimmed == "---"
            || trimmed.starts_with("name:")
            || trimmed.starts_with("description:")
        {
            continue;
        }
        return trim_chars(trimmed, 180);
    }
    String::new()
}

fn load_registry() -> Result<SkillRegistry> {
    ensure_memini_home()?;
    let path = registry_path();
    if !path.exists() {
        return Ok(SkillRegistry::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("Read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(SkillRegistry::default());
    }
    serde_json::from_str::<SkillRegistry>(&raw).with_context(|| format!("Parse {}", path.display()))
}

fn save_registry(registry: &SkillRegistry) -> Result<()> {
    ensure_memini_home()?;
    let path = registry_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("Create {}", parent.display()))?;
    }
    let json =
        serde_json::to_string_pretty(registry).context("Serialize imported skills registry")?;
    fs::write(&path, json).with_context(|| format!("Write {}", path.display()))
}

fn registry_path() -> PathBuf {
    memini_home().join(REGISTRY_FILENAME)
}

fn ensure_memini_home() -> Result<()> {
    let home = memini_home();
    fs::create_dir_all(&home).with_context(|| format!("Create {}", home.display()))?;
    Ok(())
}

fn ensure_memini_skills_root() -> Result<()> {
    ensure_memini_home()?;
    let root = memini_skills_root();
    fs::create_dir_all(&root).with_context(|| format!("Create {}", root.display()))?;
    Ok(())
}

fn memini_home() -> PathBuf {
    if let Ok(value) = env::var("MEMINI_HOME") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    if let Some(base_dirs) = BaseDirs::new() {
        return base_dirs.home_dir().join("Memini");
    }
    PathBuf::from("Memini")
}

fn memini_skills_root() -> PathBuf {
    memini_home().join("skills")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_source_supports_skills_sh_without_scheme() {
        let parsed = parse_source("skills.sh/example/repo/skill").expect("parse");
        match parsed {
            SkillSource::SkillsSh {
                owner, repo, slug, ..
            } => {
                assert_eq!(owner, "example");
                assert_eq!(repo, "repo");
                assert_eq!(slug, "skill");
            }
            _ => panic!("expected skills.sh source"),
        }
    }

    #[test]
    fn parse_source_supports_github_tree_urls() {
        let parsed =
            parse_source("https://github.com/example/repo/tree/main/skills/foo").expect("parse");
        match parsed {
            SkillSource::GitHubPath {
                owner,
                repo,
                ref_name,
                path,
                ..
            } => {
                assert_eq!(owner, "example");
                assert_eq!(repo, "repo");
                assert_eq!(ref_name.as_deref(), Some("main"));
                assert_eq!(path, "skills/foo");
            }
            _ => panic!("expected github path source"),
        }
    }

    #[test]
    fn prompt_context_includes_skill_name() {
        let skill = LoadedSkill {
            meta: ImportedSkillMeta {
                name: "demo".to_string(),
                title: "Demo Skill".to_string(),
                description: "Useful for tests".to_string(),
                source_url: "https://skills.sh/example/repo/demo".to_string(),
                repository: "https://github.com/example/repo".to_string(),
                repository_ref: "main".to_string(),
                repository_path: "skills/demo".to_string(),
                installed_at_utc: "2026-02-08T00:00:00Z".to_string(),
            },
            content: "# Demo\nDo the thing.".to_string(),
        };

        let context = build_prompt_context(&[skill], "please use demo");
        assert!(context.contains("demo"));
        assert!(context.contains("Instructions"));
    }
}
