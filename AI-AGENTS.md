# AI Agents Guide for Finna

This document provides context for AI agents working with finna.

## What is Finna?

Finna is a multi-model debate and implementation tool. It orchestrates multiple
LLMs (Claude, Codex, Gemini) to debate ideas, create roadmaps, generate specs,
and implement features through collaborative AI consensus.

## Architecture

**Three-stage workflow:**
1. `finna debate <idea>` - Multiple models debate and create implementation roadmap
2. `finna spec [--step <name>]` - Generate detailed specs for each roadmap step
3. `finna implement [--step <name>]` - Implement specs via model collaboration

**File structure:**
```
.finna/
├── roadmap.arf          # TOML roadmap with ordered steps
├── consensus.json       # JSON synthesis of debate
└── specs/
    ├── 01-step-name/
    │   └── spec.arf     # Detailed implementation spec
    ├── 02-next-step/
    │   └── spec.arf
    └── ...
```

## How It Works

### Debate Stage
- Takes idea as input
- Queries Claude, Codex, Gemini in parallel
- Each model proposes approach, key decisions, files to modify
- Synthesizes consensus into unified approach
- Generates TOML roadmap with ordered, dependency-aware steps

### Spec Stage
- For each step in roadmap:
  - Queries models for detailed implementation plan
  - Generates spec.arf with: what, why, how, backup, context
  - Includes step-by-step instructions, commands, file paths
  - Adds fallback plans if main approach fails

### Implement Stage
- Reads spec for step
- Queries models in parallel for implementation approach
- Synthesizes into concrete file edits
- Parses JSON output with edit instructions
- Applies edits to filesystem (create/modify files)
- **Does NOT auto-commit** - manual git workflow required

## LLM Backend Integration

**CLI commands invoked:**
- `claude <prompt>` - Claude Code CLI
- `codex exec --json -s read-only <prompt>` - OpenAI Codex
- `npx @google/gemini-cli <prompt>` - Google Gemini

**Environment setup:**
Requires nix-shell with Node.js 22+ and npm to install CLI tools.
Set `unset CLAUDECODE` to allow Claude CLI in Claude Code sessions.

## Spec Format (.arf files)

TOML structure:
```toml
order = 1
what = "Brief objective"
why = "Justification and context"
how = """
Step-by-step implementation:
1. Command or action
2. Next command
...
"""
backup = "Alternative approach if main plan fails"

[context]
files = ["list", "of", "files.rb"]
dependencies = ["gem", "service", "etc"]
```

## Implementation Output Format

Models generate JSON:
```json
{
  "edits": [
    {
      "path": "app/models/foo.rb",
      "action": "create|modify",
      "content": "file contents"
    }
  ]
}
```

## Usage Patterns

**Full workflow:**
```bash
cd ~/dev/project
finna debate "Build a federated blog platform"
finna spec                    # Generate all specs
finna implement --step rails-app-setup
git add -A && git commit -m "Implement rails-app-setup"
finna implement --step next-step
git add -A && git commit -m "Implement next-step"
```

**Single step:**
```bash
finna spec --step authentication-system
finna implement --step authentication-system
```

## Key Implementation Details

**Location:** `~/dev/finna/src/main.rs`

**Query functions:**
- `query_claude(prompt)` - Runs `claude` CLI, waits for output
- `query_codex(prompt)` - Runs `codex exec --json`, parses JSON response
- `query_gemini(prompt)` - Runs `npx @google/gemini-cli`
- `query_parallel(prompt)` - Runs all three in parallel, collects results

**Prompt templates:**
- `DEBATE_PROMPT` - Initial idea analysis
- `CONSENSUS_PROMPT` - Synthesize debate into consensus
- `ROADMAP_PROMPT` - Break consensus into steps
- `SPEC_PROMPT` - Generate detailed spec for step
- `IMPLEMENT_PROMPT` - Generate implementation edits
- `SYNTH_IMPL_PROMPT` - Synthesize implementation approaches

**Error handling:**
- "All models failed" if all three LLMs fail to respond
- Continues with partial results if at least one model succeeds
- Skips steps without specs during implementation

## Development

**Build:** `cargo build`
**Run:** `cargo run -- debate "idea"`
**Test:** Models must be available in PATH

**Shell.nix provides:**
- Rust toolchain (rustc, cargo, clippy)
- Node.js 22 and npm for CLI tools
- Auto-installs codex and gemini CLIs if missing
- Unsets CLAUDECODE for nested Claude sessions

## Known Limitations

1. **No auto-commit** - Manual git workflow required after implement
2. **Stderr suppressed** - Error messages from LLM CLIs may be hidden
3. **Sequential specs** - Specs generated one at a time (slow for 20+ steps)
4. **No rollback** - Failed implementations need manual cleanup
5. **JSON parsing** - LLMs sometimes output markdown instead of raw JSON

## Future Enhancements

- Add `--commit` flag to auto-commit after each step
- Parallel spec generation for faster workflows
- Dry-run mode to preview edits before applying
- Interactive approval for each edit
- Better error messages from LLM failures
- Resume partial implementations
- Track which steps are completed (state file)

## Example Project

See `~/dev/webstead` for a real example:
- Architecture doc: `ARCHITECTURE.md`
- Roadmap: `.finna/roadmap.arf` (22 steps)
- Specs: `.finna/specs/01-rails-app-setup/spec.arf` through `22-mvp-deploy/`
- Project: Federated personal site platform (Rails, ActivityPub, multi-tenant)

Generated via: `finna debate "Webstead: federated personal site platform..."`
