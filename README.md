# kubedoc

Agentic Kubernetes cluster diagnostics, performance review, and manifest generation.

kubedoc is a CLI tool that uses an LLM-powered multi-agent architecture to inspect, diagnose, and manage Kubernetes clusters. All cluster interactions go through the Kubernetes API directly — no shell commands.

## Features

- **Cluster diagnosis** — inspect nodes, pods, events, deployments, services, and configmaps for issues
- **Performance review** — analyze resource utilization and identify bottlenecks (via Prometheus MCP)
- **Manifest generation** — generate valid Kubernetes YAML from natural language
- **MCP support** — expose kubedoc tools to external hosts (Claude Desktop, Cursor) and consume external MCP tools
- **Audit logging** — every session, prompt, and tool call is recorded for traceability
- **Interactive TUI** — terminal UI with conversation history and scrollback

## Prerequisites

- Rust 2024 edition (1.85+)
- Access to a Kubernetes cluster (via `~/.kube/config`)
- An LLM API key (OpenAI, Anthropic, Groq, or a local Ollama instance)

Or use Nix (see below) — no manual Rust or dependency setup required.

## Installation

### Nix

With [Nix](https://nixos.org/download/) installed:

```bash
# Enter the dev shell (provides Rust, cargo, and all build dependencies)
nix develop

# Or build the binary directly
nix build
./result/bin/kubedoc --version
```

The flake provides:
- `nix develop` — drops you into a shell with Rust, cargo, clippy, rustfmt, and all build deps
- `nix build` — builds the binary via crane, output at `./result/bin/kubedoc`
- `nix run` — builds and runs in one step

### From source

```bash
cargo install kubedoc
```

### Prebuilt binaries

Download from [GitHub Releases](../../releases). Available for:

| Platform | Architecture |
|----------|-------------|
| Linux    | x86_64, aarch64 |
| macOS    | Intel, Apple Silicon |
| Windows  | x86_64 |

## Quick start

```bash
# 1. Generate a config template
kubedoc config init

# 2. Edit ~/.kubedoc/config.toml — set your LLM provider and API key env var

# 3. Export your API key
export OPENAI_API_KEY=sk-...

# 4. Launch the interactive session
kubedoc
```

Once running, ask questions in natural language:

```
You> diagnose my cluster
You> why is pod X in CrashLoopBackOff
You> create a deployment for nginx with 3 replicas
You> review the performance of my cluster
```

## Usage

### Interactive session (default)

```bash
kubedoc                          # launch TUI with config defaults
kubedoc --provider anthropic     # override LLM provider
kubedoc --model gpt-4o-mini      # override model
kubedoc -c my-cluster            # override K8s context
```

**TUI controls:**

| Key | Action |
|-----|--------|
| Enter | Send message |
| Up/Down | Scroll output |
| PageUp/PageDown | Scroll faster |
| Ctrl+L | Clear conversation |
| Ctrl+C / Ctrl+D | Quit |

### MCP server

Expose kubedoc's K8s tools via the Model Context Protocol:

```bash
kubedoc mcp serve                # stdio transport (for Claude Desktop, etc.)
```

Example Claude Desktop config:

```json
{
  "mcpServers": {
    "kubedoc": {
      "command": "kubedoc",
      "args": ["mcp", "serve"]
    }
  }
}
```

### Audit log review

Replay a past session's audit trail:

```bash
kubedoc audit <session-id>
```

### Configuration

```bash
kubedoc config init    # generate config template at ~/.kubedoc/config.toml
kubedoc config show    # display resolved config (secrets redacted)
```

**Config file** (`~/.kubedoc/config.toml`):

```toml
[llm]
provider = "openai"              # openai | anthropic | groq | ollama
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"   # env var holding the API key
# base_url = ""                  # optional: custom endpoint

[kube]
# kubeconfig_path = ""           # defaults to ~/.kube/config
# context = ""                   # override current-context

# [[mcp_servers]]
# name = "prometheus"
# command = ["prometheus-mcp-server"]
```

**Environment variable overrides:**

| Variable | Overrides |
|----------|-----------|
| `KUBEDOC_LLM_PROVIDER` | LLM provider |
| `KUBEDOC_LLM_MODEL` | Model name |
| `KUBEDOC_KUBECONFIG` | Path to kubeconfig |
| `KUBEDOC_KUBE_CONTEXT` | K8s context |
| `KUBEDOC_DATA_DIR` | Data directory (default: `~/.kubedoc`) |
| `KUBEDOC_CONFIG` | Config file path |

Priority: CLI flags > env vars > config file > defaults.

## Architecture

```
                    ┌──────────────┐
                    │     CLI      │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │  Coordinator │
                    │  (routes to  │
                    │  sub-agents) │
                    └──┬───┬───┬───┘
                       │   │   │
              ┌────────┘   │   └────────┐
              ▼            ▼            ▼
        ┌──────────┐ ┌──────────┐ ┌──────────┐
        │ Diagnose │ │  Review  │ │Artifacts │
        │ (K8s API)│ │ (MCP)    │ │ (K8s +   │
        │          │ │          │ │  file)   │
        └──────────┘ └──────────┘ └──────────┘
```

- **Coordinator** — parses user intent, delegates to sub-agents, summarizes results
- **Diagnose** — the only agent with direct K8s API access (nodes, pods, events, etc.)
- **Review** — performance analyst with MCP tools for metrics (Prometheus)
- **Artifacts** — generates YAML manifests, writes files to disk

## Data layout

```
~/.kubedoc/
├── config.toml
├── sessions/
│   └── session_YYYY-MM-DD_HHMMSS.json
└── audit/
    └── session_YYYY-MM-DD_HHMMSS.jsonl
```

## Non-goals

- No shell/exec into pods (security risk, not auditable)
- No built-in cluster provisioning
- No applying manifests — users apply generated artifacts via kubectl, GitOps, etc.
- No GUI — CLI only

## License

[MIT](LICENSE)

---

*Parts of this project were generated using a coding agent.*
