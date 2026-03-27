# ZeroClaw Integration

Setup and usage for the self-improvement skill on ZeroClaw. ZeroClaw supports OpenClaw's SKILL.md format; hooks (TypeScript) do not run natively—use the system prompt snippet below for session reminders.

## Overview

- **Skill**: Copy this repo (or just `SKILL.md`) into ZeroClaw's skills directory; the skill content is loaded from SKILL.md.
- **Reminder**: ZeroClaw has no JS hooks. Add the reminder block to your config's system prompt so the agent is prompted to use `.learnings/` every session.

## Workspace & config paths

- Config: `~/.config/zeroclaw/config.toml` or override with `--config` / `ZEROCLAW_CONFIG_PATH`.
- Workspace: Override with `--workspace` or `ZEROCLAW_WORKSPACE`; otherwise ZeroClaw uses its default workspace.

## Quick setup

### 1. Install the skill

Clone or copy the skill into your ZeroClaw skills directory (path may vary by install; common pattern):

```bash
# Example: clone into a skills directory ZeroClaw loads
git clone https://github.com/peterskoett/self-improving-agent.git ~/.zeroclaw/skills/self-improving-agent
```

If ZeroClaw uses a different skills path, copy the repo (or at least `SKILL.md`) there. ZeroClaw natively reads OpenClaw-format SKILL.md.

### 2. Create learning files

In your **workspace** (the directory ZeroClaw uses as cwd / project root):

```bash
mkdir -p .learnings
```

Create (or copy from this repo’s `assets/`):

- `.learnings/LEARNINGS.md` — corrections, knowledge gaps, best practices  
- `.learnings/ERRORS.md` — command failures, exceptions  
- `.learnings/FEATURE_REQUESTS.md` — user-requested capabilities  

Optional headers/templates are in `assets/LEARNINGS.md`, `assets/ERRORS.md`, `assets/FEATURE_REQUESTS.md`.

### 3. Add the reminder to the system prompt

ZeroClaw does not run OpenClaw’s TypeScript hooks. To get the same “check learnings” behavior at session start, append the following to your **system prompt** in `config.toml` (exact key may be `system_prompt`, `systemPrompt`, or under `[agent]` / `[ai]`—see ZeroClaw docs):

```markdown
## Self-Improvement Reminder

After completing tasks, evaluate if any learnings should be captured:

**Log when:**
- User corrects you → `.learnings/LEARNINGS.md`
- Command/operation fails → `.learnings/ERRORS.md`
- User wants missing capability → `.learnings/FEATURE_REQUESTS.md`
- You discover your knowledge was wrong → `.learnings/LEARNINGS.md`
- You find a better approach → `.learnings/LEARNINGS.md`

**Promote when pattern is proven:**
- Behavioral patterns → `SOUL.md`
- Workflow improvements → `AGENTS.md`
- Tool gotchas → `TOOLS.md`

Keep entries simple: date, title, what happened, what to do differently.
```

Example TOML (adjust key names to match your ZeroClaw version):

```toml
[agent]
system_prompt = """
You are a helpful assistant.
... your existing instructions ...

## Self-Improvement Reminder
After completing tasks, evaluate if any learnings should be captured:
- User corrects you → .learnings/LEARNINGS.md
- Command/operation fails → .learnings/ERRORS.md
- User wants missing capability → .learnings/FEATURE_REQUESTS.md
- Knowledge was wrong / better approach → .learnings/LEARNINGS.md
Promote when proven: behavioral → SOUL.md, workflow → AGENTS.md, tool gotchas → TOOLS.md.
"""
```

The same text is available as `hooks/zeroclaw/REMINDER.md` in this repo for copy-paste.

## Workspace layout (recommended)

```
<workspace>/
├── .learnings/
│   ├── LEARNINGS.md
│   ├── ERRORS.md
│   └── FEATURE_REQUESTS.md
├── AGENTS.md    # optional: workflow/delegation
├── SOUL.md      # optional: behavior/principles
└── TOOLS.md     # optional: tool gotchas
```

Promotion targets (AGENTS.md, SOUL.md, TOOLS.md) are optional; create them when you start promoting learnings.

## Verification

- Ensure the skill directory contains `SKILL.md` and ZeroClaw is configured to load skills from that directory.
- Send a message that should trigger a learning (e.g. correct the model); then check `.learnings/` for new entries.

## Differences from OpenClaw

| Feature            | OpenClaw                    | ZeroClaw                          |
|--------------------|-----------------------------|------------------------------------|
| Skill format       | SKILL.md                    | Same (native)                      |
| Hooks (bootstrap)  | TypeScript hook in hooks/   | Not supported → use system prompt  |
| Workspace path     | ~/.openclaw/workspace/       | Config / ZEROCLAW_WORKSPACE         |
| Session tools      | sessions_list, sessions_send| Use ZeroClaw’s built-in tools      |

For full skill behavior (when to log, where, promotion rules), see the root `SKILL.md`.
