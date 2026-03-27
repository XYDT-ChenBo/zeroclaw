# ZeroClaw Skills

Each subdirectory is a skill. Create a `SKILL.toml` or `SKILL.md` file inside.

## SKILL.toml format

```toml
[skill]
name = "my-skill"
description = "What this skill does"
version = "0.1.0"
author = "your-name"
tags = ["productivity", "automation"]

[[tools]]
name = "my_tool"
description = "What this tool does"
kind = "shell"
command = "echo hello"
```

## SKILL.md format (simpler)

Just write a markdown file with instructions for the agent.
The agent will read it and follow the instructions.

## Installing community skills

```bash
zeroclaw skills install <source>
zeroclaw skills list
```
