use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
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
Generate exact code edits.

## Output (JSON only, no markdown)
{{"edits": [{{"path": "file.rs", "old": "exact text to find", "new": "replacement text"}}]}}"#;

const SYNTH_IMPL_PROMPT: &str = r#"## What
Synthesize implementation proposals into final edits.

## Proposals
{proposals}

## Output (JSON only, no markdown)
{{"edits": [{{"path": "file", "old": "exact", "new": "replacement"}}]}}"#;

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
    edits: Vec<Edit>,
}

#[derive(Debug, serde::Deserialize)]
struct Edit {
    path: String,
    old: String,
    new: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Debate { idea }) => {
            let idea = idea.join(" ");
            if idea.is_empty() {
                anyhow::bail!("Usage: finna debate <idea>");
            }
            cmd_debate(&idea).await
        }
        Some(Commands::Spec { step }) => cmd_spec(step).await,
        Some(Commands::Implement { step }) => cmd_implement(step).await,
        None => {
            let idea = cli.idea.join(" ");
            if idea.is_empty() {
                eprintln!("Usage: finna <idea>");
                eprintln!("       finna debate <idea>");
                eprintln!("       finna spec [--step NAME]");
                eprintln!("       finna implement [--step NAME]");
                std::process::exit(1);
            }
            cmd_all(&idea).await
        }
    }
}

async fn cmd_debate(idea: &str) -> Result<()> {
    println!("finna debate: {}", idea);

    println!("\n[1/3] Debating...");
    let debate_prompt = DEBATE_PROMPT.replace("{idea}", idea);
    let proposals = query_parallel(&debate_prompt).await?;

    println!("\n[2/3] Reaching consensus...");
    let consensus_prompt = CONSENSUS_PROMPT.replace("{proposals}", &proposals.join("\n\n---\n\n"));
    let consensus = query_claude(&consensus_prompt).await?;
    println!("Consensus: {}", truncate(&consensus, 200));

    println!("\n[3/3] Creating roadmap...");
    let roadmap_prompt = ROADMAP_PROMPT.replace("{consensus}", &consensus);
    let roadmap_toml = query_claude(&roadmap_prompt).await?;
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

async fn cmd_spec(step_filter: Option<String>) -> Result<()> {
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
        let spec_toml = query_claude(&spec_prompt).await?;
        let spec_toml = extract_toml(&spec_toml);

        let spec_dir = format!(".finna/specs/{:02}-{}", step.order, step.name);
        fs::create_dir_all(&spec_dir)?;
        fs::write(format!("{}/spec.arf", spec_dir), &spec_toml)?;
        println!("    {}/spec.arf", spec_dir);
    }

    println!("\nNext: finna implement");
    Ok(())
}

async fn cmd_implement(step_filter: Option<String>) -> Result<()> {
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

    for step in steps {
        let spec_path = format!(".finna/specs/{:02}-{}/spec.arf", step.order, step.name);
        if !Path::new(&spec_path).exists() {
            eprintln!("  Skipping {}: no spec found", step.name);
            continue;
        }

        let spec = fs::read_to_string(&spec_path)?;

        println!("\n  Implementing: {}", step.name);
        let impl_prompt = IMPLEMENT_PROMPT.replace("{spec}", &spec);
        let impl_proposals = query_parallel(&impl_prompt).await?;

        let synth_prompt =
            SYNTH_IMPL_PROMPT.replace("{proposals}", &impl_proposals.join("\n\n---\n\n"));
        let impl_result = query_claude(&synth_prompt).await?;

        let output: ImplOutput = parse_json(&impl_result)?;
        apply_edits(&output.edits)?;
    }

    println!("\nDone!");
    Ok(())
}

async fn cmd_all(idea: &str) -> Result<()> {
    println!("finna: {}", idea);

    // Stage 1: Debate
    println!("\n[1/5] Debating...");
    let debate_prompt = DEBATE_PROMPT.replace("{idea}", idea);
    let proposals = query_parallel(&debate_prompt).await?;

    println!("\n[2/5] Reaching consensus...");
    let consensus_prompt = CONSENSUS_PROMPT.replace("{proposals}", &proposals.join("\n\n---\n\n"));
    let consensus = query_claude(&consensus_prompt).await?;
    println!("Consensus: {}", truncate(&consensus, 200));

    // Stage 2: Roadmap
    println!("\n[3/5] Creating roadmap...");
    let roadmap_prompt = ROADMAP_PROMPT.replace("{consensus}", &consensus);
    let roadmap_toml = query_claude(&roadmap_prompt).await?;
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

        let spec_toml = query_claude(&spec_prompt).await?;
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
        let impl_proposals = query_parallel(&impl_prompt).await?;

        let synth_prompt =
            SYNTH_IMPL_PROMPT.replace("{proposals}", &impl_proposals.join("\n\n---\n\n"));
        let impl_result = query_claude(&synth_prompt).await?;

        let output: ImplOutput = parse_json(&impl_result)?;
        apply_edits(&output.edits)?;
    }

    println!("\nDone!");
    println!("  Roadmap: .finna/roadmap.arf");
    println!("  Specs:   .finna/specs/");
    Ok(())
}

async fn query_parallel(prompt: &str) -> Result<Vec<String>> {
    let (claude, codex, gemini) = tokio::join!(
        query_claude(prompt),
        query_codex(prompt),
        query_gemini(prompt),
    );

    let mut results = Vec::new();
    if let Ok(r) = claude {
        results.push(format!("[Claude]\n{}", r));
    }
    if let Ok(r) = codex {
        results.push(format!("[Codex]\n{}", r));
    }
    if let Ok(r) = gemini {
        results.push(format!("[Gemini]\n{}", r));
    }

    if results.is_empty() {
        anyhow::bail!("All models failed");
    }

    Ok(results)
}

async fn query_claude(prompt: &str) -> Result<String> {
    let output = Command::new("claude")
        .args(["-p", prompt])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .context("Failed to run claude")?;

    if !output.status.success() {
        anyhow::bail!("Claude failed");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn query_codex(prompt: &str) -> Result<String> {
    let output = Command::new("codex")
        .args(["exec", "--json", "-s", "read-only", prompt])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .context("Failed to run codex")?;

    if !output.status.success() {
        anyhow::bail!("Codex failed");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("item.completed") {
                if let Some(text) = v.pointer("/item/content/0/text").and_then(|t| t.as_str()) {
                    return Ok(text.to_string());
                }
            }
        }
    }

    anyhow::bail!("No output from codex")
}

async fn query_gemini(prompt: &str) -> Result<String> {
    let output = Command::new("npx")
        .args(["@google/gemini-cli", prompt])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .context("Failed to run gemini")?;

    if !output.status.success() {
        anyhow::bail!("Gemini failed");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

    serde_json::from_str(json_str.trim()).context("Failed to parse JSON")
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
