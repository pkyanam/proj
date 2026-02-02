# proj

**Project-scoped developer environment manager**

Unify your terminals, ports, browsers, and processes under a single "project" namespace. Inspired by [Theo's "Agentic Code Problem"](https://x.com/t3dotgg/status/1885410873638170984) - the chaos of managing multiple dev projects with AI agents running everywhere.

## The Problem

When running multiple AI coding agents (Claude Code, Cursor, etc.), you quickly lose track of:
- Which terminal tab belongs to which project
- Which browser window has the right auth session
- Why localhost:3000 is broken (another project took it)
- Where that agent notification came from

**proj** makes "project" a first-class routing primitive across your dev environment.

## Installation

### Quick Install (macOS/Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/pkyanam/proj/main/install.sh | bash
```

### Install from Source

```bash
git clone https://github.com/pkyanam/proj
cd proj
cargo build --release
cp target/release/proj target/release/proj-daemon ~/.local/bin/
```

### Homebrew (coming soon)

```bash
brew install proj
```

## Quick Start

```bash
# Create a project (in your project's root directory)
proj new my-app

# Run your dev server
proj my-app run npm run dev

# Access it via a stable URL (no more port conflicts!)
# → http://my-app.localhost:8080

# Open browser with isolated profile (separate cookies, auth, localStorage)
proj my-app open

# Check what's running
proj ls

# Stop a project
proj my-app stop
```

## Commands

| Command | Description |
|---------|-------------|
| `proj new <name>` | Create a new project |
| `proj <name> run <cmd>` | Run command in project context |
| `proj <name> <cmd>` | Shorthand for run |
| `proj <name> open` | Open browser with isolated Chrome profile |
| `proj <name> stop` | Stop project's processes |
| `proj <name>` | Show project info |
| `proj ls` | List all projects with status |
| `proj` | Show daemon status |
| `proj daemon` | Start daemon (usually auto-starts) |
| `proj daemon -f` | Start daemon in foreground (for debugging) |

## Features

### Automatic Port Routing

Every project gets a stable URL: `<project>.localhost:8080`

No more:
- Hunting for which port your app is on
- Broken OAuth redirects because the port changed
- Port conflicts between projects

```bash
proj my-app run npm run dev    # Might use port 3000, 3001, or whatever
curl http://my-app.localhost:8080  # Always works
```

### Browser Profile Isolation

Each project gets its own Chrome profile with separate:
- Cookies
- localStorage
- Auth sessions
- Extensions

```bash
proj my-app open      # Opens Chrome with my-app's isolated profile
proj other-app open   # Opens with other-app's profile (different auth!)
```

### Auto-Detect Project

When you're in a project directory, proj automatically knows which project you're working on:

```bash
cd ~/code/my-app
proj run npm test     # Knows it's my-app
proj open            # Opens my-app's browser
```

### Process Supervision

Processes are monitored with stdout/stderr capture. Port detection happens automatically.

```bash
proj ls
# ● my-app:3000
#     /Users/you/code/my-app
# ○ other-app
#     /Users/you/code/other-app
```

## How It Works

```
┌─────────────────────────────────────────────────────┐
│                    proj CLI                         │
│  (new, run, open, ls, stop)                        │
└─────────────────┬───────────────────────────────────┘
                  │ Unix socket (auto-starts daemon)
                  ▼
┌─────────────────────────────────────────────────────┐
│              proj-daemon                            │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  │
│  │  Project    │  │   Reverse   │  │  Process    │  │
│  │  Registry   │  │   Proxy     │  │  Manager    │  │
│  └─────────────┘  └─────────────┘  └─────────────┘  │
└─────────────────────────────────────────────────────┘
```

1. **DNS**: Modern browsers resolve `*.localhost` to `127.0.0.1` (RFC 6761)
2. **Port Detection**: `lsof` monitors spawned processes for port bindings
3. **Proxy Routing**: Reverse proxy routes `Host: my-app.localhost` → actual port
4. **Browser Isolation**: Chrome's `--user-data-dir` flag creates isolated profiles

## Storage

All data is stored in `~/.proj/`:

```
~/.proj/
├── daemon.sock           # IPC socket
├── daemon.pid            # Daemon PID
└── projects/
    └── <project-name>/
        ├── project.json  # Project metadata
        └── chrome/       # Isolated Chrome profile
```

## Environment Variables

When running commands with `proj <name> run`, these are set:

- `PROJECT_ID` - The project name
- `PROJECT_HOST` - The project hostname (e.g., `my-app.localhost`)

## FAQ

**Q: Why port 8080?**
A: Port 80 requires root. 8080 is the standard unprivileged HTTP port. All project URLs are `<project>.localhost:8080`.

**Q: Does this work with any dev server?**
A: Yes! It wraps any command and auto-detects the port it binds to.

**Q: How does browser isolation work?**
A: Each project gets a `--user-data-dir` in `~/.proj/projects/<name>/chrome/`. Chrome treats it as a completely separate browser instance.

**Q: What about Firefox?**
A: Currently Chrome/Chromium only. Firefox support could be added using `-profile`.

## License

MIT
