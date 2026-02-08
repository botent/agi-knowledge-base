//! `/skills` command handlers — import/list/reload installed skills.

use super::super::App;
use super::super::log_src;
use super::super::logging::LogLevel;

impl App {
    pub(crate) fn handle_skills_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.list_imported_skills();
            return;
        }

        match args[0] {
            "list" => self.list_imported_skills(),
            "reload" | "refresh" => self.reload_imported_skills_cmd(),
            "import" => {
                if let Some(source) = args.get(1) {
                    self.import_skill_cmd(source);
                } else {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        "Usage: /skills import <skills.sh-url | github-url>".to_string()
                    );
                }
            }
            other => {
                log_src!(
                    self,
                    LogLevel::Warn,
                    format!("Unknown /skills command: {other}")
                );
                self.log(
                    LogLevel::Info,
                    "Try: /skills, /skills list, /skills import <url>, /skills reload".to_string(),
                );
            }
        }
    }

    fn list_imported_skills(&mut self) {
        if self.imported_skills.is_empty() {
            self.log(
                LogLevel::Info,
                "No imported skills yet. Use /skills import <skills.sh-url>.".to_string(),
            );
            return;
        }

        let skills = self.imported_skills.clone();
        self.log(
            LogLevel::Info,
            format!("Imported skills ({}):", skills.len()),
        );
        for skill in skills {
            let description = if skill.meta.description.trim().is_empty() {
                "(no description)".to_string()
            } else {
                skill.meta.description.trim().to_string()
            };
            self.log(
                LogLevel::Info,
                format!(
                    "  {} -- {} [{}]",
                    skill.meta.name, description, skill.meta.source_url
                ),
            );
        }
    }

    fn reload_imported_skills_cmd(&mut self) {
        match self.reload_imported_skills() {
            Ok(()) => {
                self.log(
                    LogLevel::Info,
                    format!("Reloaded {} imported skill(s).", self.imported_skills.len()),
                );
            }
            Err(err) => {
                log_src!(
                    self,
                    LogLevel::Warn,
                    format!("Failed to reload imported skills: {err:#}")
                );
            }
        }
    }

    fn import_skill_cmd(&mut self, source: &str) {
        self.log(LogLevel::Info, format!("Importing skill from {source} ..."));

        let result = self.runtime.block_on(crate::skills::import_skill(source));
        match result {
            Ok(outcome) => {
                if let Err(err) = self.reload_imported_skills() {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        format!("Skill imported but reload failed: {err:#}")
                    );
                }

                self.log(
                    LogLevel::Info,
                    format!(
                        "✓ Imported skill '{}' ({} file(s)) to {}",
                        outcome.meta.name,
                        outcome.file_count,
                        outcome.destination.display()
                    ),
                );
                if !outcome.meta.description.trim().is_empty() {
                    self.log(
                        LogLevel::Info,
                        format!("  {}", outcome.meta.description.trim()),
                    );
                }
            }
            Err(err) => {
                log_src!(
                    self,
                    LogLevel::Warn,
                    format!("Skill import failed: {err:#}")
                );
            }
        }
    }
}
