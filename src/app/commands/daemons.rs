//! `/daemon` (`/auto`) and `/spawn` command handlers — background task
//! management and live agent window creation.

use std::collections::HashSet;

use super::super::App;
use super::super::agent_recipes;
use super::super::daemon;
use super::super::log_src;
use super::super::logging::LogLevel;

// ── /daemon ──────────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_daemon_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.list_daemons();
            return;
        }

        match args[0] {
            "list" => self.list_daemons(),
            "dir" => self.show_daemon_recipe_dir(),
            "reload" | "refresh" => self.reload_daemon_recipes_cmd(),
            "templates" => self.list_daemon_recipe_templates(),
            "scaffold" => {
                if let Some(template_id) = args.get(1) {
                    let name = args.get(2).copied();
                    self.scaffold_daemon_recipe(template_id, name);
                } else {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        "Usage: /daemon scaffold <template-id> [name]".to_string()
                    );
                }
            }
            "run" => {
                if let Some(name) = args.get(1) {
                    self.run_daemon_now(name);
                } else {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        "Usage: /daemon run <name>".to_string()
                    );
                }
            }
            "start" => {
                if let Some(name) = args.get(1) {
                    self.start_daemon(name);
                } else {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        "Usage: /daemon start <name>".to_string()
                    );
                }
            }
            "stop" => {
                if let Some(name) = args.get(1) {
                    self.stop_daemon(name);
                } else {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        "Usage: /daemon stop <name>".to_string()
                    );
                }
            }
            "create" | "add" => {
                // /daemon create <name> <interval_secs> <instructions...>
                if args.len() >= 4 {
                    let name = args[1].to_string();
                    let interval: u64 = args[2]
                        .parse()
                        .unwrap_or(crate::constants::DEFAULT_AGENT_INTERVAL_SECS);
                    let prompt = args[3..].join(" ");
                    self.create_daemon_recipe_task(&name, interval, &prompt);
                } else {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        "Usage: /daemon create <name> <interval_secs> <instructions>".to_string()
                    );
                }
            }
            "remove" => {
                if let Some(name) = args.get(1) {
                    self.remove_daemon_task(name);
                } else {
                    log_src!(
                        self,
                        LogLevel::Warn,
                        "Usage: /daemon remove <name>".to_string()
                    );
                }
            }
            "results" => {
                let filter = args.get(1).copied();
                self.show_daemon_results(filter);
            }
            other => {
                log_src!(
                    self,
                    LogLevel::Warn,
                    format!("Unknown /daemon command: {other}")
                );
            }
        }
    }

    fn show_daemon_recipe_dir(&mut self) {
        match agent_recipes::ensure_agents_dir() {
            Ok(dir) => {
                self.log(
                    LogLevel::Info,
                    format!("Agent recipe directory: {}", dir.display()),
                );
                self.log(
                    LogLevel::Info,
                    "Put .md files with front matter here to define reusable auto-agents."
                        .to_string(),
                );
                self.log(
                    LogLevel::Info,
                    "Optional trigger fields: trigger_events: VariableUpdate, trigger_variables: foo,bar.*"
                        .to_string(),
                );
            }
            Err(err) => {
                log_src!(
                    self,
                    LogLevel::Error,
                    format!("Failed to prepare recipe directory: {err:#}")
                );
            }
        }
    }

    fn reload_daemon_recipes_cmd(&mut self) {
        let recipes = self.load_daemon_recipes();
        self.log(
            LogLevel::Info,
            format!("Loaded {} recipe-based auto-agent(s).", recipes.len()),
        );
    }

    fn list_daemon_recipe_templates(&mut self) {
        self.log(
            LogLevel::Info,
            "Available auto-agent templates:".to_string(),
        );
        for template in agent_recipes::recipe_templates() {
            self.log(
                LogLevel::Info,
                format!(
                    "  {} -- {} [{}s default]",
                    template.id, template.description, template.interval_secs
                ),
            );
        }
        self.log(
            LogLevel::Info,
            "Use /daemon scaffold <template-id> [name] to create and start one.".to_string(),
        );
    }

    fn scaffold_daemon_recipe(&mut self, template_id: &str, requested_name: Option<&str>) {
        let Some(template) = agent_recipes::recipe_template_by_id(template_id) else {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Unknown template '{template_id}'. Use /daemon templates.")
            );
            return;
        };

        let name = requested_name.unwrap_or(template.id);
        let spec = agent_recipes::NewAgentRecipe {
            name: name.to_string(),
            description: template.description.to_string(),
            interval_secs: template.interval_secs,
            auto_start: true,
            tools: template
                .tools
                .iter()
                .map(|tool| (*tool).to_string())
                .collect(),
            persona: template.persona.to_string(),
            instructions: template.instructions.to_string(),
        };

        match agent_recipes::write_recipe_file(&spec) {
            Ok(path) => {
                let task_name = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or(name)
                    .to_string();
                self.log(
                    LogLevel::Info,
                    format!(
                        "Created recipe '{}' from template '{}' at {}.",
                        task_name,
                        template.id,
                        path.display()
                    ),
                );
                let def = daemon::DaemonTaskDef {
                    name: task_name,
                    persona: spec.persona,
                    prompt: spec.instructions,
                    interval_secs: spec.interval_secs,
                    trigger_events: Vec::new(),
                    trigger_variables: Vec::new(),
                    tools: spec.tools,
                    paused: false,
                };
                self.spawn_daemon_task(def);
            }
            Err(err) => {
                log_src!(
                    self,
                    LogLevel::Error,
                    format!("Failed to scaffold recipe '{name}': {err:#}")
                );
            }
        }
    }

    fn load_daemon_recipes(&mut self) -> Vec<agent_recipes::AgentRecipe> {
        match agent_recipes::load_agent_recipes() {
            Ok(recipes) => recipes,
            Err(err) => {
                log_src!(
                    self,
                    LogLevel::Warn,
                    format!("Failed to load recipe-based agents: {err:#}")
                );
                Vec::new()
            }
        }
    }

    fn daemon_def_from_recipe(
        recipe: &agent_recipes::AgentRecipe,
        paused: bool,
    ) -> daemon::DaemonTaskDef {
        daemon::DaemonTaskDef {
            name: recipe.name.clone(),
            persona: recipe.persona.clone(),
            prompt: recipe.instructions.clone(),
            interval_secs: recipe.interval_secs,
            trigger_events: recipe.trigger_events.clone(),
            trigger_variables: recipe.trigger_variables.clone(),
            tools: recipe.tools.clone(),
            paused,
        }
    }

    pub(crate) fn autostart_daemon_recipes(&mut self) {
        let builtins = daemon::builtin_tasks();
        let recipes = self.load_daemon_recipes();
        let mut started = 0usize;

        for recipe in recipes {
            if !recipe.auto_start {
                continue;
            }
            if builtins
                .iter()
                .any(|task| task.name.eq_ignore_ascii_case(&recipe.name))
            {
                self.log(
                    LogLevel::Warn,
                    format!(
                        "Skipped recipe '{}' because it conflicts with a built-in task name.",
                        recipe.name
                    ),
                );
                continue;
            }
            if self
                .daemon_handles
                .iter()
                .any(|handle| handle.def.name.eq_ignore_ascii_case(&recipe.name))
            {
                continue;
            }
            self.spawn_daemon_task(Self::daemon_def_from_recipe(&recipe, false));
            started += 1;
        }

        if started > 0 {
            self.log(
                LogLevel::Info,
                format!("Auto-started {} recipe-based task(s).", started),
            );
        }
    }

    fn list_daemons(&mut self) {
        let builtins = daemon::builtin_tasks();
        let recipes = self.load_daemon_recipes();

        if self.daemon_handles.is_empty() && builtins.is_empty() && recipes.is_empty() {
            self.log(
                LogLevel::Info,
                "No auto-agent tasks configured.".to_string(),
            );
            return;
        }

        self.log(LogLevel::Info, "Background tasks:".to_string());

        let mut known_names = HashSet::new();

        for builtin in &builtins {
            known_names.insert(builtin.name.to_ascii_lowercase());
            let running = self
                .daemon_handles
                .iter()
                .any(|h| h.def.name.eq_ignore_ascii_case(&builtin.name));
            let status = if running { "running" } else { "available" };
            self.log(
                LogLevel::Info,
                format!(
                    "  {} -- {} [{}s, {}, builtin]",
                    builtin.name, builtin.prompt, builtin.interval_secs, status
                ),
            );
        }

        for recipe in &recipes {
            if known_names.contains(&recipe.name.to_ascii_lowercase()) {
                self.log(
                    LogLevel::Warn,
                    format!(
                        "Skipping duplicate recipe '{}' from {} (conflicts with built-in).",
                        recipe.name,
                        recipe.path.display()
                    ),
                );
                continue;
            }
            known_names.insert(recipe.name.to_ascii_lowercase());
            let running = self
                .daemon_handles
                .iter()
                .any(|h| h.def.name.eq_ignore_ascii_case(&recipe.name));
            let status = if running { "running" } else { "available" };
            let preview: String = if recipe.description.trim().is_empty() {
                recipe.instructions.chars().take(72).collect()
            } else {
                recipe.description.clone()
            };
            let trigger_info = recipe
                .trigger_summary()
                .map(|summary| format!(", trigger:{summary}"))
                .unwrap_or_default();
            self.log(
                LogLevel::Info,
                format!(
                    "  {} -- {} [{}s, {}, file:{}{}]",
                    recipe.name,
                    preview,
                    recipe.interval_secs,
                    status,
                    recipe.path.display(),
                    trigger_info
                ),
            );
        }

        let runtime_only: Vec<(String, String, u64)> = self
            .daemon_handles
            .iter()
            .filter(|handle| !known_names.contains(&handle.def.name.to_ascii_lowercase()))
            .map(|handle| {
                (
                    handle.def.name.clone(),
                    handle.def.prompt.clone(),
                    handle.def.interval_secs,
                )
            })
            .collect();
        for (name, prompt, interval) in runtime_only {
            self.log(
                LogLevel::Info,
                format!("  {name} -- {prompt} [{interval}s, running, runtime-only]"),
            );
        }
    }

    fn run_daemon_now(&mut self, name: &str) {
        for handle in &self.daemon_handles {
            if handle.def.name.eq_ignore_ascii_case(name) {
                handle.wake.notify_one();
                self.log(
                    LogLevel::Info,
                    format!("Woke task '{}' for immediate run.", handle.def.name),
                );
                return;
            }
        }

        let builtins = daemon::builtin_tasks();
        if let Some(def) = builtins
            .into_iter()
            .find(|task| task.name.eq_ignore_ascii_case(name))
        {
            self.run_daemon_oneshot(def);
            return;
        }

        let recipes = self.load_daemon_recipes();
        if let Some(recipe) = recipes
            .into_iter()
            .find(|recipe| recipe.name.eq_ignore_ascii_case(name))
        {
            let def = Self::daemon_def_from_recipe(&recipe, true);
            self.run_daemon_oneshot(def);
            return;
        }

        log_src!(self, LogLevel::Warn, format!("Unknown daemon task: {name}"));
    }

    fn start_daemon(&mut self, name: &str) {
        if self
            .daemon_handles
            .iter()
            .any(|handle| handle.def.name.eq_ignore_ascii_case(name))
        {
            self.log(
                LogLevel::Info,
                format!("Daemon '{name}' is already running."),
            );
            return;
        }

        let builtins = daemon::builtin_tasks();
        if let Some(mut def) = builtins
            .into_iter()
            .find(|task| task.name.eq_ignore_ascii_case(name))
        {
            def.paused = false;
            self.spawn_daemon_task(def);
            return;
        }

        let recipes = self.load_daemon_recipes();
        if let Some(recipe) = recipes
            .into_iter()
            .find(|recipe| recipe.name.eq_ignore_ascii_case(name))
        {
            self.spawn_daemon_task(Self::daemon_def_from_recipe(&recipe, false));
            return;
        }

        log_src!(self, LogLevel::Warn, format!("Unknown daemon task: {name}"));
    }

    fn stop_daemon(&mut self, name: &str) {
        if let Some(pos) = self
            .daemon_handles
            .iter()
            .position(|handle| handle.def.name.eq_ignore_ascii_case(name))
        {
            let handle = self.daemon_handles.remove(pos);
            handle.abort.abort();
            self.log(
                LogLevel::Info,
                format!("Task '{}' stopped.", handle.def.name),
            );
        } else {
            log_src!(
                self,
                LogLevel::Warn,
                format!("No running daemon named '{name}'.")
            );
        }
    }

    fn create_daemon_recipe_task(&mut self, name: &str, interval: u64, prompt: &str) {
        let builtins = daemon::builtin_tasks();
        if builtins
            .iter()
            .any(|task| task.name.eq_ignore_ascii_case(name))
            || self
                .daemon_handles
                .iter()
                .any(|handle| handle.def.name.eq_ignore_ascii_case(name))
        {
            log_src!(
                self,
                LogLevel::Warn,
                format!("A daemon named '{name}' already exists.")
            );
            return;
        }

        let spec = agent_recipes::NewAgentRecipe {
            name: name.to_string(),
            description: "CLI-created auto-agent recipe".to_string(),
            interval_secs: interval,
            auto_start: true,
            tools: vec!["local".to_string()],
            persona: format!(
                "You are a background autonomous agent named '{}'. \
                 You can use local workspace tools to inspect files, edit files, and run commands. \
                 Be concise and action-oriented.",
                name
            ),
            instructions: prompt.to_string(),
        };

        match agent_recipes::write_recipe_file(&spec) {
            Ok(path) => {
                self.log(
                    LogLevel::Info,
                    format!(
                        "Created auto-agent recipe '{}' at {}.",
                        name,
                        path.display()
                    ),
                );
                let task_name = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or(name)
                    .to_string();
                let def = daemon::DaemonTaskDef {
                    name: task_name.clone(),
                    persona: spec.persona,
                    prompt: spec.instructions,
                    interval_secs: spec.interval_secs,
                    trigger_events: Vec::new(),
                    trigger_variables: Vec::new(),
                    tools: spec.tools,
                    paused: false,
                };
                self.spawn_daemon_task(def);
                self.log(
                    LogLevel::Info,
                    format!(
                        "Task '{}' created and started (every {}s).",
                        task_name, interval
                    ),
                );
            }
            Err(err) => {
                log_src!(
                    self,
                    LogLevel::Error,
                    format!("Failed to create auto-agent recipe '{name}': {err:#}")
                );
            }
        }
    }

    fn remove_daemon_task(&mut self, name: &str) {
        let mut stopped = false;
        if let Some(pos) = self
            .daemon_handles
            .iter()
            .position(|handle| handle.def.name.eq_ignore_ascii_case(name))
        {
            let handle = self.daemon_handles.remove(pos);
            handle.abort.abort();
            stopped = true;
        }

        let is_builtin = daemon::builtin_tasks()
            .iter()
            .any(|task| task.name.eq_ignore_ascii_case(name));
        if is_builtin {
            if stopped {
                self.log(
                    LogLevel::Info,
                    format!("Stopped built-in task '{name}'. Built-ins cannot be deleted."),
                );
            } else {
                log_src!(
                    self,
                    LogLevel::Warn,
                    format!("'{name}' is built-in. Use /daemon stop {name} to stop it.")
                );
            }
            return;
        }

        match agent_recipes::remove_recipe_file(name) {
            Ok(Some(path)) => {
                self.log(
                    LogLevel::Info,
                    format!("Removed recipe file: {}", path.display()),
                );
                if stopped {
                    self.log(
                        LogLevel::Info,
                        format!("Stopped task '{name}' before removal."),
                    );
                }
            }
            Ok(None) => {
                if stopped {
                    self.log(
                        LogLevel::Info,
                        format!("Stopped runtime-only task '{name}'."),
                    );
                } else {
                    log_src!(self, LogLevel::Warn, format!("Unknown daemon task: {name}"));
                }
            }
            Err(err) => {
                log_src!(
                    self,
                    LogLevel::Error,
                    format!("Failed to remove recipe '{name}': {err:#}")
                );
            }
        }
    }

    fn show_daemon_results(&mut self, filter: Option<&str>) {
        let results: Vec<_> = self
            .daemon_results
            .iter()
            .filter(|r| filter.is_none() || Some(r.0.as_str()) == filter)
            .map(|r| (r.2.clone(), r.0.clone(), r.1.clone()))
            .collect();

        if results.is_empty() {
            self.log(
                LogLevel::Info,
                "No daemon results yet. Run /daemon run <name> to trigger one.".to_string(),
            );
            return;
        }

        self.log(
            LogLevel::Info,
            format!("Recent daemon results ({}):", results.len()),
        );
        for (ts, name, msg) in results.iter().rev().take(10) {
            self.log(LogLevel::Info, format!("  [{ts}] {name} -- {msg}"));
        }
    }
}

// ── /reply ───────────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_reply_command(&mut self, args: Vec<&str>) {
        if args.is_empty() || args[0] == "list" {
            self.list_waiting_agent_questions();
            return;
        }

        if args.len() < 2 {
            log_src!(
                self,
                LogLevel::Warn,
                "Usage: /reply <id|next> <message>  or  /reply list".to_string()
            );
            return;
        }

        let target = args[0];
        let reply = args[1..].join(" ");
        if reply.trim().is_empty() {
            log_src!(
                self,
                LogLevel::Warn,
                "Reply message cannot be empty.".to_string()
            );
            return;
        }

        let window_id = if target.eq_ignore_ascii_case("next") {
            self.first_waiting_window_id()
        } else {
            target.parse::<usize>().ok()
        };

        let Some(window_id) = window_id else {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Invalid target '{target}'. Use /reply list.")
            );
            return;
        };

        if !self.reply_to_agent_window(window_id, &reply) {
            log_src!(
                self,
                LogLevel::Warn,
                format!("Agent #{window_id} is not waiting for input.")
            );
        }
    }

    fn list_waiting_agent_questions(&mut self) {
        let waiting = self.waiting_window_summaries();
        if waiting.is_empty() {
            self.log(
                LogLevel::Info,
                "No agents are waiting for input.".to_string(),
            );
            return;
        }

        self.log(
            LogLevel::Info,
            format!("Agents waiting for input ({}):", waiting.len()),
        );
        for (id, label, question) in waiting {
            let preview: String = question.chars().take(140).collect();
            self.log(LogLevel::Info, format!("  #{id} {label} -- {preview}"));
        }
        self.log(
            LogLevel::Info,
            "Reply with /reply <id|next> <message> or inline #<id> <message>.".to_string(),
        );
    }
}

// ── /spawn ───────────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_spawn_command(&mut self, args: Vec<&str>) {
        if args.is_empty() {
            self.log(
                LogLevel::Info,
                "Usage: /spawn <prompt>  or  /spawn list".to_string(),
            );
            self.log(
                LogLevel::Info,
                "Spin up a live agent window. Watch it think in real time, and reply if it needs help.".to_string(),
            );
            self.log(
                LogLevel::Info,
                "Use Tab/Shift-Tab or PgUp/PgDn on dashboard to scroll live agents; Enter opens selected.".to_string(),
            );
            self.log(
                LogLevel::Info,
                "Use Alt+Enter (or Ctrl+J) to add a newline in the input composer.".to_string(),
            );
            self.log(
                LogLevel::Info,
                "Use Ctrl+1..9 to focus a window quickly, Ctrl+0 to unfocus.".to_string(),
            );
            return;
        }

        if args[0] == "list" {
            self.list_spawned_agents();
            return;
        }

        // Everything after /spawn is the prompt.
        let prompt = args.join(" ");
        self.spawn_agent_window_cmd(&prompt);
    }

    fn spawn_agent_window_cmd(&mut self, prompt: &str) {
        use std::sync::atomic::Ordering;
        let window_id = self.next_window_id.fetch_add(1, Ordering::SeqCst);
        let label = format!("Agent #{window_id}");

        // Create the window in Thinking state.
        let window = daemon::AgentWindow {
            id: window_id,
            label: label.clone(),
            prompt: prompt.to_string(),
            status: daemon::AgentWindowStatus::Thinking,
            output_lines: Vec::new(),
            pending_question: None,
            scroll: 0,
            persona: self.active_agent.persona.clone(),
            skill_context: self.skills_prompt_context(prompt),
            mcp_snapshots: Vec::new(),
            coordination_key: String::new(),
        };
        self.agent_windows.push(window);

        // Spawn the background task.
        let tx = self.daemon_tx.clone();
        let openai = self.openai.clone();
        let key = self.openai_key.clone();
        let rice_handle = self.runtime.spawn(crate::rice::RiceStore::connect());
        let persona = self.active_agent.persona.clone();
        let skill_context = self.skills_prompt_context(prompt);

        daemon::spawn_agent_window(
            window_id,
            persona,
            prompt.to_string(),
            skill_context,
            tx,
            openai,
            key,
            rice_handle,
            self.runtime.handle().clone(),
        );

        self.log(
            LogLevel::Info,
            format!("Spawned {label} — opening session."),
        );

        // Auto-navigate into the agent session.
        self.focused_window = Some(window_id);
        self.view_mode = super::super::ViewMode::AgentSession(window_id);
        // Also select this cell in the grid for when we come back.
        let idx = self.agent_windows.len().saturating_sub(1);
        self.grid_selected = idx;
    }

    fn list_spawned_agents(&mut self) {
        if self.agent_windows.is_empty() {
            self.log(
                LogLevel::Info,
                "No agent windows. Use /spawn <prompt> to create one.".to_string(),
            );
            return;
        }

        self.log(
            LogLevel::Info,
            format!("Agent windows ({}):", self.agent_windows.len()),
        );
        let windows: Vec<_> = self
            .agent_windows
            .iter()
            .map(|w| {
                let status = match w.status {
                    daemon::AgentWindowStatus::Thinking => "thinking",
                    daemon::AgentWindowStatus::Done => "done",
                    daemon::AgentWindowStatus::WaitingForInput => "WAITING FOR INPUT",
                };
                (w.id, w.label.clone(), w.prompt.clone(), status)
            })
            .collect();
        for (id, label, prompt, status) in &windows {
            let preview: String = prompt.chars().take(60).collect();
            self.log(
                LogLevel::Info,
                format!("  [{id}] {label} -- {preview}  [{status}]"),
            );
        }
    }
}
