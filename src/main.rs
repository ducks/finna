mod config;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

const DEBATE_PROMPT: &str = r#"## What
Analyze and propose an approach for this idea.

## Idea
{idea}

## How
1. Consider the key requirements
2. Identify trade-offs
3. Propose a concrete approach

## Output (JSON only, no markdown)
{{"approach": "your approach", "key_decisions": ["decision 1"], "files": ["paths to create/modify"]}}"#;

const CONSENSUS_PROMPT: &str = r#"## What
Synthesize these proposals into consensus.

## Proposals
{proposals}

## Output (JSON only, no markdown)
{{"approach": "final approach", "key_decisions": ["decisions"], "files": ["paths"]}}"#;

const ROADMAP_PROMPT: &str = r#"## What
Break this approach into ordered implementation steps.

## Consensus
{consensus}

## How
Create a roadmap with discrete, implementable steps. Each step should be small enough to implement in one pass.

## Output (TOML only, no markdown)
what = "one line description of the whole project"
why = "motivation and context"

[[steps]]
order = 1
name = "short-kebab-name"
summary = "what this step accomplishes"
depends_on = []

[[steps]]
order = 2
name = "another-step"
summary = "what this step accomplishes"
depends_on = ["short-kebab-name"]"#;

const SPEC_PROMPT: &str = r#"## What
Write a detailed spec for this step.

## Step
{step}

## Context
{context}

## Output (TOML only, no markdown)
order = {order}
what = "one sentence description"
why = "context and motivation for this specific step"
how = """
Step-by-step implementation plan:
1. First do this
2. Then do that
3. Finally do this
"""
backup = "fallback approach if primary fails"

[context]
files = ["paths/to/relevant/files"]
dependencies = ["external deps if any"]"#;

const IMPLEMENT_PROMPT: &str = r#"## What
Implement this spec.

## Spec
{spec}

## How
Generate exact code edits AND shell commands to run.

For NEW files: set "old" to empty string ""
For EXISTING files: provide exact text to find and replace

## Output (JSON only, no markdown)
{{
  "edits": [
    {{"path": "new_file.rs", "old": "", "new": "full file content"}},
    {{"path": "existing.rs", "old": "exact text to find", "new": "replacement text"}}
  ],
  "commands": [{{"command": "rails new app", "description": "Create Rails app", "working_dir": null}}]
}}"#;

const SYNTH_IMPL_PROMPT: &str = r#"## What
Synthesize implementation proposals into final edits and commands.

## Proposals
{proposals}

## How
For NEW files: set "old" to empty string ""
For EXISTING files: provide exact text to find and replace

## Output (JSON only, no markdown)
{{
  "edits": [
    {{"path": "new_file", "old": "", "new": "full content"}},
    {{"path": "existing", "old": "exact", "new": "replacement"}}
  ],
  "commands": [{{"command": "bundle install", "description": "Install gems", "working_dir": null}}]
}}"#;

#[derive(Parser)]
#[command(name = "finna")]
#[command(about = "Multi-model debate, spec, and implement tool")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Run all stages with this idea
    #[arg(trailing_var_arg = true)]
    idea: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Debate an idea and create a roadmap
    Debate {
        /// The idea to debate
        #[arg(trailing_var_arg = true)]
        idea: Vec<String>,
    },
    /// Create specs from existing roadmap
    Spec {
        /// Specific step to spec (default: all)
        #[arg(short, long)]
        step: Option<String>,
    },
    /// Implement from existing specs
    Implement {
        /// Specific step to implement (default: all)
        #[arg(short, long)]
        step: Option<String>,
        /// Force re-implementation of completed steps
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Debug, serde::Deserialize)]
struct Roadmap {
    what: String,
    why: String,
    steps: Vec<Step>,
}

#[derive(Debug, serde::Deserialize, Clone)]
struct Step {
    order: u32,
    name: String,
    summary: String,
    #[serde(default)]
    #[allow(dead_code)]
    depends_on: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ImplOutput {
    #[serde(default)]
    edits: Vec<Edit>,
    #[serde(default)]
    commands: Vec<ShellCommand>,
}

#[derive(Debug, serde::Deserialize)]
struct Edit {
    path: String,
    old: String,
    new: String,
}

#[derive(Debug, serde::Deserialize)]
struct ShellCommand {
    command: String,
    description: String,
    #[serde(default)]
    working_dir: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;

    match cli.command {
        Some(Commands::Debate { idea }) => {
            let idea = idea.join(" ");
            if idea.is_empty() {
                anyhow::bail!("Usage: finna debate <idea>");
            }
            cmd_debate(&config, &idea).await
        }
        Some(Commands::Spec { step }) => cmd_spec(&config, step).await,
        Some(Commands::Implement { step, force }) => cmd_implement(&config, step, force).await,
        None => {
            let idea = cli.idea.join(" ");
            if idea.is_empty() {
                eprintln!("Usage: finna <idea>");
                eprintln!("       finna debate <idea>");
                eprintln!("       finna spec [--step NAME]");
                eprintln!("       finna implement [--step NAME]");
                std::process::exit(1);
            }
            cmd_all(&config, &idea).await
        }
    }
}

async fn cmd_debate(config: &Config, idea: &str) -> Result<()> {
    println!("finna debate: {}", idea);

    println!("\n[1/3] Debating...");
    let debate_prompt = DEBATE_PROMPT.replace("{idea}", idea);
    let proposals = query_parallel(config, &config.default_debate_providers, &debate_prompt).await?;

    println!("\n[2/3] Reaching consensus...");
    let consensus_prompt = CONSENSUS_PROMPT.replace("{proposals}", &proposals.join("\n\n---\n\n"));
    let consensus = query_provider(&config.default_spec_provider, config.get_provider(&config.default_spec_provider).unwrap(), &consensus_prompt).await?;
    println!("Consensus: {}", truncate(&consensus, 200));

    println!("\n[3/3] Creating roadmap...");
    let roadmap_prompt = ROADMAP_PROMPT.replace("{consensus}", &consensus);
    let roadmap_toml = query_provider(&config.default_spec_provider, config.get_provider(&config.default_spec_provider).unwrap(), &roadmap_prompt).await?;
    let roadmap_toml = extract_toml(&roadmap_toml);

    fs::create_dir_all(".finna/specs")?;
    fs::write(".finna/roadmap.arf", &roadmap_toml)?;
    fs::write(".finna/consensus.json", &consensus)?;

    let roadmap: Roadmap = toml::from_str(&roadmap_toml).context("Failed to parse roadmap")?;
    println!(
        "\nRoadmap: .finna/roadmap.arf ({} steps)",
        roadmap.steps.len()
    );
    for step in &roadmap.steps {
        println!("  {}. {} - {}", step.order, step.name, step.summary);
    }

    println!("\nNext: finna spec");
    Ok(())
}

async fn cmd_spec(config: &Config, step_filter: Option<String>) -> Result<()> {
    let roadmap_path = ".finna/roadmap.arf";
    if !Path::new(roadmap_path).exists() {
        anyhow::bail!("No roadmap found. Run 'finna debate <idea>' first.");
    }

    let roadmap_toml = fs::read_to_string(roadmap_path)?;
    let roadmap: Roadmap = toml::from_str(&roadmap_toml).context("Failed to parse roadmap")?;

    let steps: Vec<&Step> = match &step_filter {
        Some(name) => roadmap.steps.iter().filter(|s| s.name == *name).collect(),
        None => roadmap.steps.iter().collect(),
    };

    if steps.is_empty() {
        anyhow::bail!("No matching steps found");
    }

    println!("finna spec: {} step(s)", steps.len());

    for step in steps {
        let step_context = format!(
            "Project: {}\nWhy: {}\nThis step: {} - {}",
            roadmap.what, roadmap.why, step.name, step.summary
        );
        let spec_prompt = SPEC_PROMPT
            .replace("{step}", &step.summary)
            .replace("{context}", &step_context)
            .replace("{order}", &step.order.to_string());

        println!("\n  Spec: {}", step.name);
        let spec_toml = query_provider(&config.default_spec_provider, config.get_provider(&config.default_spec_provider).unwrap(), &spec_prompt).await?;
        let spec_toml = extract_toml(&spec_toml);

        let spec_dir = format!(".finna/specs/{:02}-{}", step.order, step.name);
        fs::create_dir_all(&spec_dir)?;
        fs::write(format!("{}/spec.arf", spec_dir), &spec_toml)?;
        println!("    {}/spec.arf", spec_dir);
    }

    println!("\nNext: finna implement");
    Ok(())
}

async fn cmd_implement(config: &Config, step_filter: Option<String>, force: bool) -> Result<()> {
    let roadmap_path = ".finna/roadmap.arf";
    if !Path::new(roadmap_path).exists() {
        anyhow::bail!("No roadmap found. Run 'finna debate <idea>' first.");
    }

    let roadmap_toml = fs::read_to_string(roadmap_path)?;
    let roadmap: Roadmap = toml::from_str(&roadmap_toml).context("Failed to parse roadmap")?;

    let steps: Vec<&Step> = match &step_filter {
        Some(name) => roadmap.steps.iter().filter(|s| s.name == *name).collect(),
        None => roadmap.steps.iter().collect(),
    };

    if steps.is_empty() {
        anyhow::bail!("No matching steps found");
    }

    println!("finna implement: {} step(s)", steps.len());

    // If single step, run sequentially (original behavior)
    if steps.len() == 1 {
        let step = steps[0];
        implement_step(config, step, force).await?;
        println!("\nDone!");
        return Ok(());
    }

    // Multiple steps: implement in parallel respecting dependencies
    implement_parallel(config, &steps, force).await?;

    println!("\nDone!");
    Ok(())
}

async fn implement_step(config: &Config, step: &Step, force: bool) -> Result<()> {
    let spec_path = format!(".finna/specs/{:02}-{}/spec.arf", step.order, step.name);
    implement_step_by_path(config, &step.name, &spec_path, force).await
}

fn is_step_completed(spec_path: &str) -> bool {
    if let Ok(contents) = fs::read_to_string(spec_path) {
        contents.contains("outcome = \"success\"")
    } else {
        false
    }
}

fn mark_step_completed(spec_path: &str) -> Result<()> {
    let mut contents = fs::read_to_string(spec_path)?;
    if !contents.contains("outcome =") {
        contents.push_str("\noutcome = \"success\"\n");
        fs::write(spec_path, contents)?;
    }
    Ok(())
}

fn git_ensure_main_and_pull() -> Result<()> {
    // Check current branch
    let output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .output()?;
    let current_branch = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Switch to main if not already there
    if current_branch != "main" {
        println!("    (switching from {} to main)", current_branch);
        std::process::Command::new("git")
            .args(["checkout", "main"])
            .status()?;
    }

    // Pull latest
    println!("    (pulling latest from main)");
    std::process::Command::new("git")
        .args(["pull"])
        .status()?;

    Ok(())
}

fn git_create_feature_branch(step_name: &str) -> Result<String> {
    let branch_name = format!("feature/step-{}", step_name);
    println!("    (creating branch: {})", branch_name);
    std::process::Command::new("git")
        .args(["checkout", "-b", &branch_name])
        .status()?;
    Ok(branch_name)
}

fn git_merge_to_main(branch_name: &str, step_name: &str) -> Result<()> {
    // Switch back to main
    println!("    (merging {} to main)", branch_name);
    std::process::Command::new("git")
        .args(["checkout", "main"])
        .status()?;

    // Merge with --no-ff to create merge commit
    let merge_msg = format!("Merge branch '{}' - implemented {}", branch_name, step_name);
    std::process::Command::new("git")
        .args(["merge", "--no-ff", "-m", &merge_msg, &branch_name])
        .status()?;

    // Delete the feature branch
    std::process::Command::new("git")
        .args(["branch", "-d", &branch_name])
        .status()?;

    Ok(())
}

async fn implement_step_by_path(config: &Config, step_name: &str, spec_path: &str, force: bool) -> Result<()> {
    if !Path::new(spec_path).exists() {
        eprintln!("  Skipping {}: no spec found", step_name);
        return Ok(());
    }

    // Check if already completed
    if !force && is_step_completed(spec_path) {
        println!("  ⊙ Skipping {}: already completed (use --force to re-run)", step_name);
        return Ok(());
    }

    // Git workflow: ensure on main and pull
    git_ensure_main_and_pull()?;

    // Create feature branch for this step
    let branch_name = git_create_feature_branch(step_name)?;

    let spec = fs::read_to_string(spec_path)?;

    println!("\n  Implementing: {}", step_name);
    let impl_prompt = IMPLEMENT_PROMPT.replace("{spec}", &spec);
    let impl_proposals = query_parallel(config, &config.default_debate_providers, &impl_prompt).await?;

    let synth_prompt =
        SYNTH_IMPL_PROMPT.replace("{proposals}", &impl_proposals.join("\n\n---\n\n"));
    let impl_result = query_provider(&config.default_spec_provider, config.get_provider(&config.default_spec_provider).unwrap(), &synth_prompt).await?;

    // Try to parse JSON, but if it fails, ask Claude to convert the response to JSON
    let output: ImplOutput = match parse_json(&impl_result) {
        Ok(output) => output,
        Err(_) => {
            println!("    (non-JSON response, asking Claude to convert...)");
            let convert_prompt = format!(
                "Convert the following response to JSON format {{\"edits\": [], \"commands\": []}}.\n\
                 If the response indicates no work is needed or the spec is already implemented, return empty arrays.\n\
                 Otherwise, extract any mentioned file edits or shell commands.\n\
                 Return ONLY valid JSON, no explanation.\n\n\
                 Response to convert:\n{}",
                impl_result
            );
            let json_result = query_provider(
                &config.default_spec_provider,
                config.get_provider(&config.default_spec_provider).unwrap(),
                &convert_prompt
            ).await?;
            parse_json(&json_result)?
        }
    };
    apply_edits(&output.edits)?;
    execute_commands(&output.commands).await?;

    // Mark as completed
    mark_step_completed(spec_path)?;

    // Git workflow: merge back to main
    git_merge_to_main(&branch_name, step_name)?;

    Ok(())
}

async fn implement_parallel(config: &Config, steps: &[&Step], force: bool) -> Result<()> {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // Track completed steps
    let completed = Arc::new(Mutex::new(HashSet::new()));

    // Pre-populate with already-completed steps (if not forcing re-run)
    if !force {
        for step in steps {
            let spec_path = format!(".finna/specs/{:02}-{}/spec.arf", step.order, step.name);
            if is_step_completed(&spec_path) {
                completed.lock().await.insert(step.name.clone());
            }
        }
    }

    // Build map of step name -> step for quick lookup (for future use)
    let _step_map: HashMap<String, &Step> = steps.iter().map(|s| (s.name.clone(), *s)).collect();

    // Clone config for use in spawned tasks
    let config = Arc::new(config.clone());
    let force = Arc::new(force);

    // Keep implementing until all steps are done
    while completed.lock().await.len() < steps.len() {
        // Find steps ready to implement (all dependencies completed)
        let ready_steps: Vec<&Step> = {
            let comp = completed.lock().await;
            steps
                .iter()
                .filter(|s| !comp.contains(&s.name))
                .filter(|s| s.depends_on.iter().all(|dep| comp.contains(dep)))
                .copied()
                .collect()
        };

        if ready_steps.is_empty() {
            // Check for circular dependencies or missing dependencies
            let remaining: Vec<String> = {
                let comp = completed.lock().await;
                steps
                    .iter()
                    .filter(|s| !comp.contains(&s.name))
                    .map(|s| s.name.clone())
                    .collect()
            };
            anyhow::bail!("No steps ready to implement. Remaining: {:?}. Possible circular dependency or missing step.", remaining);
        }

        println!("\n  Running {} step(s) in parallel...", ready_steps.len());

        // Launch all ready steps in parallel
        let mut handles = vec![];
        for step in ready_steps {
            let step_name = step.name.clone();
            let step_order = step.order;
            let completed_clone = Arc::clone(&completed);
            let config_clone = Arc::clone(&config);
            let force_clone = Arc::clone(&force);

            let handle = tokio::spawn(async move {
                // Reconstruct step from stored data
                let spec_path = format!(".finna/specs/{:02}-{}/spec.arf", step_order, step_name);
                let result = implement_step_by_path(&config_clone, &step_name, &spec_path, *force_clone).await;
                if result.is_ok() {
                    completed_clone.lock().await.insert(step_name.clone());
                    println!("  ✓ Completed: {}", step_name);
                } else {
                    eprintln!("  ✗ Failed: {}", step_name);
                }
                result
            });
            handles.push(handle);
        }

        // Wait for all parallel steps to complete
        for handle in handles {
            handle.await??;
        }
    }

    Ok(())
}

async fn cmd_all(config: &Config, idea: &str) -> Result<()> {
    println!("finna: {}", idea);

    // Stage 1: Debate
    println!("\n[1/5] Debating...");
    let debate_prompt = DEBATE_PROMPT.replace("{idea}", idea);
    let proposals = query_parallel(config, &config.default_debate_providers, &debate_prompt).await?;

    println!("\n[2/5] Reaching consensus...");
    let consensus_prompt = CONSENSUS_PROMPT.replace("{proposals}", &proposals.join("\n\n---\n\n"));
    let consensus = query_provider(&config.default_spec_provider, config.get_provider(&config.default_spec_provider).unwrap(), &consensus_prompt).await?;
    println!("Consensus: {}", truncate(&consensus, 200));

    // Stage 2: Roadmap
    println!("\n[3/5] Creating roadmap...");
    let roadmap_prompt = ROADMAP_PROMPT.replace("{consensus}", &consensus);
    let roadmap_toml = query_provider(&config.default_spec_provider, config.get_provider(&config.default_spec_provider).unwrap(), &roadmap_prompt).await?;
    let roadmap_toml = extract_toml(&roadmap_toml);

    fs::create_dir_all(".finna/specs")?;
    fs::write(".finna/roadmap.arf", &roadmap_toml)?;
    println!("Roadmap: .finna/roadmap.arf");

    let roadmap: Roadmap = toml::from_str(&roadmap_toml).context("Failed to parse roadmap")?;
    println!("  {} steps planned", roadmap.steps.len());

    // Stage 3: Spec each step
    println!("\n[4/5] Writing specs...");
    for step in &roadmap.steps {
        let step_context = format!(
            "Project: {}\nWhy: {}\nThis step: {} - {}",
            roadmap.what, roadmap.why, step.name, step.summary
        );
        let spec_prompt = SPEC_PROMPT
            .replace("{step}", &step.summary)
            .replace("{context}", &step_context)
            .replace("{order}", &step.order.to_string());

        let spec_toml = query_provider(&config.default_spec_provider, config.get_provider(&config.default_spec_provider).unwrap(), &spec_prompt).await?;
        let spec_toml = extract_toml(&spec_toml);

        let spec_dir = format!(".finna/specs/{:02}-{}", step.order, step.name);
        fs::create_dir_all(&spec_dir)?;
        fs::write(format!("{}/spec.arf", spec_dir), &spec_toml)?;
        println!("  {}/spec.arf", spec_dir);
    }

    // Stage 4: Implement each spec
    println!("\n[5/5] Implementing...");
    for step in &roadmap.steps {
        let spec_path = format!(".finna/specs/{:02}-{}/spec.arf", step.order, step.name);
        let spec = fs::read_to_string(&spec_path)?;

        println!("  Implementing: {}", step.name);
        let impl_prompt = IMPLEMENT_PROMPT.replace("{spec}", &spec);
        let impl_proposals = query_parallel(config, &config.default_debate_providers, &impl_prompt).await?;

        let synth_prompt =
            SYNTH_IMPL_PROMPT.replace("{proposals}", &impl_proposals.join("\n\n---\n\n"));
        let impl_result = query_provider(&config.default_spec_provider, config.get_provider(&config.default_spec_provider).unwrap(), &synth_prompt).await?;

        let output: ImplOutput = parse_json(&impl_result)?;
        apply_edits(&output.edits)?;
        execute_commands(&output.commands).await?;
    }

    println!("\nDone!");
    println!("  Roadmap: .finna/roadmap.arf");
    println!("  Specs:   .finna/specs/");
    Ok(())
}

async fn query_parallel(config: &Config, provider_names: &[String], prompt: &str) -> Result<Vec<String>> {
    let mut tasks = Vec::new();

    for name in provider_names {
        let name_clone = name.clone();
        let prompt_clone = prompt.to_string();
        let provider_config = config.get_provider(name).cloned();

        let task = tokio::spawn(async move {
            if let Some(provider_config) = provider_config {
                query_provider(&name_clone, &provider_config, &prompt_clone).await
            } else {
                Err(anyhow::anyhow!("Provider {} not configured", name_clone))
            }
        });

        tasks.push((name.clone(), task));
    }

    let mut results = Vec::new();
    for (name, task) in tasks {
        match task.await {
            Ok(Ok(response)) => {
                results.push(format!("[{}]\n{}", name, response));
            }
            Ok(Err(e)) => {
                eprintln!("Provider {} failed: {}", name, e);
            }
            Err(e) => {
                eprintln!("Provider {} task failed: {}", name, e);
            }
        }
    }

    if results.is_empty() {
        anyhow::bail!("All models failed");
    }

    Ok(results)
}

async fn query_provider(name: &str, provider_config: &config::ProviderConfig, prompt: &str) -> Result<String> {
    // Replace {prompt} placeholder in args
    let args: Vec<String> = provider_config.args.iter()
        .map(|arg| arg.replace("{prompt}", prompt))
        .collect();

    let output = Command::new(&provider_config.command)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .context(format!("Failed to run {}", name))?;

    if !output.status.success() {
        anyhow::bail!("{} failed with status: {}", name, output.status);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Special handling for codex JSON stream
    if name == "codex" || provider_config.provider_type == "codex" {
        for line in stdout.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if v.get("type").and_then(|t| t.as_str()) == Some("item.completed") {
                    if let Some(text) = v.pointer("/item/content/0/text").and_then(|t| t.as_str()) {
                        return Ok(text.to_string());
                    }
                }
            }
        }
        anyhow::bail!("No output from {}", name)
    } else if name == "claude" || provider_config.provider_type == "claude" {
        // Special handling for Claude CLI JSON output format
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
            if let Some(result) = v.get("result").and_then(|r| r.as_str()) {
                return Ok(result.to_string());
            }
        }
        // If parsing as JSON failed or no result field, return raw output
        Ok(stdout.trim().to_string())
    } else {
        Ok(stdout.trim().to_string())
    }
}

fn extract_toml(text: &str) -> String {
    if text.contains("```toml") {
        text.split("```toml")
            .nth(1)
            .and_then(|s| s.split("```").next())
            .unwrap_or(text)
            .trim()
            .to_string()
    } else if text.contains("```") {
        text.split("```")
            .nth(1)
            .and_then(|s| s.split("```").next())
            .unwrap_or(text)
            .trim()
            .to_string()
    } else {
        text.trim().to_string()
    }
}

fn parse_json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T> {
    let json_str = if text.contains("```json") {
        text.split("```json")
            .nth(1)
            .and_then(|s| s.split("```").next())
            .unwrap_or(text)
    } else if text.contains("```") {
        text.split("```")
            .nth(1)
            .and_then(|s| s.split("```").next())
            .unwrap_or(text)
    } else {
        text
    };

    serde_json::from_str(json_str.trim()).map_err(|e| {
        eprintln!("\n=== JSON Parse Error ===");
        eprintln!("Error: {}", e);
        eprintln!("\n=== Raw Response (first 1000 chars) ===");
        eprintln!("{}", &text.chars().take(1000).collect::<String>());
        eprintln!("\n=== Extracted JSON (first 1000 chars) ===");
        eprintln!(
            "{}",
            &json_str.trim().chars().take(1000).collect::<String>()
        );
        eprintln!("========================\n");
        anyhow::anyhow!("Failed to parse JSON: {}", e)
    })
}

fn apply_edits(edits: &[Edit]) -> Result<()> {
    for edit in edits {
        let path = Path::new(&edit.path);

        if path.exists() {
            let content = fs::read_to_string(path)
                .with_context(|| format!("Failed to read {}", edit.path))?;

            if !content.contains(&edit.old) {
                eprintln!("Warning: Could not find text to replace in {}", edit.path);
                continue;
            }

            let new_content = content.replace(&edit.old, &edit.new);
            fs::write(path, new_content)
                .with_context(|| format!("Failed to write {}", edit.path))?;
            println!("    Updated: {}", edit.path);
        } else {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, &edit.new)
                .with_context(|| format!("Failed to create {}", edit.path))?;
            println!("    Created: {}", edit.path);
        }
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_step_deserialization() {
        let toml = r#"
            order = 1
            name = "test-step"
            summary = "Test step"
            depends_on = ["other-step"]
        "#;

        let step: Step = toml::from_str(toml).unwrap();
        assert_eq!(step.order, 1);
        assert_eq!(step.name, "test-step");
        assert_eq!(step.summary, "Test step");
        assert_eq!(step.depends_on, vec!["other-step"]);
    }

    #[test]
    fn test_step_deserialization_no_deps() {
        let toml = r#"
            order = 1
            name = "test-step"
            summary = "Test step"
        "#;

        let step: Step = toml::from_str(toml).unwrap();
        assert_eq!(step.depends_on.len(), 0);
    }

    #[test]
    fn test_roadmap_deserialization() {
        let toml = r#"
            what = "Test project"
            why = "Testing"
            
            [[steps]]
            order = 1
            name = "step-one"
            summary = "First step"
            depends_on = []
            
            [[steps]]
            order = 2
            name = "step-two"
            summary = "Second step"
            depends_on = ["step-one"]
        "#;

        let roadmap: Roadmap = toml::from_str(toml).unwrap();
        assert_eq!(roadmap.what, "Test project");
        assert_eq!(roadmap.why, "Testing");
        assert_eq!(roadmap.steps.len(), 2);
        assert_eq!(roadmap.steps[0].name, "step-one");
        assert_eq!(roadmap.steps[1].depends_on, vec!["step-one"]);
    }

    #[test]
    fn test_extract_toml_with_fence() {
        let text = r#"
Some explanation text

```toml
key = "value"
```

More text
        "#;

        let result = extract_toml(text);
        assert_eq!(result.trim(), "key = \"value\"");
    }

    #[test]
    fn test_extract_toml_without_fence() {
        let text = r#"
Some text
```
key = "value"
```
More text
        "#;

        let result = extract_toml(text);
        assert_eq!(result.trim(), "key = \"value\"");
    }

    #[test]
    fn test_extract_toml_plain() {
        let text = "key = \"value\"";
        let result = extract_toml(text);
        assert_eq!(result, text);
    }
}

async fn execute_commands(commands: &[ShellCommand]) -> Result<()> {
    use std::process::Stdio;

    for cmd in commands {
        println!("  Running: {}", cmd.description);
        println!("    $ {}", cmd.command);

        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(&cmd.command);

        if let Some(ref dir) = cmd.working_dir {
            command.current_dir(dir);
        }

        command.stdout(Stdio::inherit());
        command.stderr(Stdio::inherit());

        let status = command
            .status()
            .await
            .with_context(|| format!("Failed to execute: {}", cmd.command))?;

        if !status.success() {
            anyhow::bail!("Command failed: {}", cmd.command);
        }
    }

    Ok(())
}
