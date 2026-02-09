//! `/share` command handlers — shared workspace (team memory) management.

use crate::rice::RiceStatus;

use super::super::App;
use super::super::log_src;
use super::super::logging::LogLevel;

impl App {
    pub(crate) fn handle_share_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.show_share_status();
            return;
        }
        match args[0] {
            "join" => {
                if let Some(name) = args.get(1) {
                    self.join_shared_workspace(name);
                } else {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        "Usage: /share join <workspace-name>".to_string()
                    );
                }
            }
            "leave" => self.leave_shared_workspace(),
            "status" => self.show_share_status(),
            other => {
                log_src!(
                    self,
                    LogLevel::Warn,
                    format!("Unknown /share command: {other}")
                );
            }
        }
    }

    fn show_share_status(&mut self) {
        match &self.rice.shared_run_id {
            Some(name) => {
                self.log(
                    LogLevel::Info,
                    format!(
                        "Shared workspace: {name} (all memory is shared with anyone on this workspace)"
                    ),
                );
                self.log(
                    LogLevel::Info,
                    "Use /share leave to return to your private memory.".to_string(),
                );
            }
            None => {
                self.log(
                    LogLevel::Info,
                    "You are in your private workspace. Memory is only visible to you.".to_string(),
                );
                self.log(
                    LogLevel::Info,
                    "Use /share join <name> to join a shared workspace with your team.".to_string(),
                );
            }
        }
    }

    fn join_shared_workspace(&mut self, name: &str) {
        if matches!(&self.rice.status, RiceStatus::Disabled(_)) {
            log_src!(
                self,
                LogLevel::Warn,
                "Rice is not connected -- shared workspaces require Rice.".to_string()
            );
            return;
        }

        // Clear the local conversation thread since it belongs to the
        // old workspace context.
        self.conversation_thread.clear();

        self.rice.join_workspace(name);

        // Persist the choice so it's restored on next launch.
        if let Err(err) = self.runtime.block_on(self.rice.save_shared_workspace()) {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Could not persist workspace choice: {err:#}")
            );
        }

        self.log(LogLevel::Info, format!("Joined shared workspace: {name}"));
        self.log(
            LogLevel::Info,
            "Memories you create are now visible to everyone in this workspace.".to_string(),
        );
        self.log(
            LogLevel::Info,
            "Memories from other members will appear when relevant to your questions.".to_string(),
        );

        // Try to load the shared conversation thread.
        match self.runtime.block_on(self.rice.load_thread()) {
            Ok(thread) if !thread.is_empty() => {
                let turns = thread.len() / 2;
                self.conversation_thread = thread;
                self.log(
                    LogLevel::Info,
                    format!("Picked up {turns} shared conversation turn(s)."),
                );
            }
            _ => {}
        }
        self.restart_rice_trigger_listener();
    }

    fn leave_shared_workspace(&mut self) {
        if self.rice.shared_run_id.is_none() {
            self.log(
                LogLevel::Info,
                "You are already in your private workspace.".to_string(),
            );
            return;
        }

        let old_name = self.rice.shared_run_id.clone().unwrap_or_default();
        self.conversation_thread.clear();
        self.rice.leave_workspace();

        if let Err(err) = self.runtime.block_on(self.rice.save_shared_workspace()) {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Could not persist workspace change: {err:#}")
            );
        }

        self.log(LogLevel::Info, format!("Left shared workspace: {old_name}"));
        self.log(
            LogLevel::Info,
            "Back to your private memory. Only you can see it.".to_string(),
        );

        // Restore personal conversation thread.
        match self.runtime.block_on(self.rice.load_thread()) {
            Ok(thread) if !thread.is_empty() => {
                let turns = thread.len() / 2;
                self.conversation_thread = thread;
                self.log(
                    LogLevel::Info,
                    format!("Restored {turns} personal conversation turn(s)."),
                );
            }
            _ => {}
        }
        self.restart_rice_trigger_listener();
    }
}
