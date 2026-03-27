# Self-Improvement Reminder (paste into ZeroClaw system prompt)

ZeroClaw does not run OpenClaw's JavaScript hooks. Add the block below to your `config.toml` system prompt so the agent is reminded to use `.learnings/` each session.

---

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
