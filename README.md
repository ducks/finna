# finna

Multi-model debate, spec, and implement tool. Takes an idea, debates it across
Claude/Codex/Gemini, creates a roadmap, writes specs, and implements.

## Usage

```bash
# Run all stages
finna "Add JWT authentication to the API"

# Or run stages separately
finna debate "Add JWT authentication"  # debate → roadmap
finna spec                              # roadmap → specs
finna implement                         # specs → code

# Target specific steps
finna spec --step auth-middleware
finna implement --step auth-middleware
```

## How it works

1. **Debate**: Three models (Claude, Codex, Gemini) independently analyze the idea
2. **Consensus**: Claude synthesizes proposals into a unified approach
3. **Roadmap**: Break consensus into ordered, dependency-aware steps
4. **Spec**: Write detailed ARF specs for each step
5. **Implement**: Models propose edits, Claude synthesizes, edits applied

## Output

```
.finna/
├── consensus.json      # Synthesized approach
├── roadmap.arf         # Ordered steps with dependencies
└── specs/
    ├── 01-step-name/
    │   └── spec.arf    # Detailed implementation spec
    ├── 02-next-step/
    │   └── spec.arf
    └── ...
```

## ARF spec format

```toml
order = 1
what = "one sentence description"
why = "context and motivation"
how = """
Step-by-step implementation plan:
1. First do this
2. Then do that
"""
backup = "fallback approach"

[context]
files = ["paths/to/files"]
dependencies = ["external deps"]
```

## Requirements

- `claude` CLI
- `codex` CLI
- `npx @google/gemini-cli`

## Build

```bash
nix-shell
cargo build --release
```

## License

MIT
