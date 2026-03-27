---
name: cn-news-briefing
description: 用 web_search_tool（DuckDuckGo）与 web_fetch 汇总「AI 最新动态」与「苏超」相关新闻简报。可选加财经、综合体育。用于「AI 新闻」「苏超」「今日快讯」等请求。
约束：必须完整读取本 Skill 的全部规则后才能执行。
---

# AI 与苏超新闻速报（DuckDuckGo）

## 做什么

默认输出两块：**AI 领域最新动态**、**苏超（江苏省城市足球联赛等，以检索到的权威表述为准）**。  
流程：`web_search_tool` 多轮检索 → 关键条目 `web_fetch` 核对 → 中文简报。不编造；不确定标「待核实」。

## 约束

- 时间：默认「今天」；不足则标「近24小时」。
- 每次查询**只写一个** `site:域名`；多站则分多次搜。
- 用户额外要「财经 / 其他体育」时，在两块主内容之后追加小节。

## 站点与查询（按主题）

### AI 最新（中文站轮换）

站：`36kr.com`、`ithome.com`、`geekpark.net`、`leiphone.com`、`jiqizhixin.com`、`wallstreetcn.com`（科技财经交叉时）

查询示例（各换 2～3 个站）：

- `今天 人工智能 大模型 site:36kr.com`
- `AI 开源 动态 site:ithome.com`
- `今天 AIGC 应用 site:geekpark.net`

无结果时加英文：`LLM AI news today site:36kr.com`

### 苏超

站：`sports.sina.com.cn`、`sports.qq.com`、`thepaper.cn`、`dongqiudi.com`、`cctv.com`

查询示例：

- `苏超 赛果 site:sports.sina.com.cn`
- `江苏 城市足球 联赛 site:thepaper.cn`
- `苏超 今日 site:sports.qq.com`

无结果时去掉「今日」，或试：`江苏男足 业余联赛 site:thepaper.cn`

### 财经、综合体育（常查不到时加强）

财经：必须**具体词** + `site:`，少用「经济新闻资讯」。

- `今天 央行 利率 site:stcn.com` / `CPI 宏观 site:caixin.com` / `A股 收盘 site:cls.cn`

体育（非苏超）：`今天 足球 战报 site:sports.sina.com.cn`、`NBA site:espn.com`

仍 `No results found`：换下一个站 → 缩短查询 → 中英各一条。

## 工具还是改 Skill？

- **优先按本 Skill 做多轮站点与关键词**；经济与体育多数情况是查询太泛或单站无索引，不是工具坏了。
- 若已严格轮换仍**长期**整类为空，再考虑改**工具侧**：`web_search` 换 `brave` / 自建 `searxng`，或在代码里给 DuckDuckGo 加重试与更多端点。

## 输出（默认）

```markdown
# 简报（YYYY-MM-DD）

## 一、AI 最新
1. **[标题]** — 摘要 — 来源

## 二、苏超
1. **[标题]** — 摘要 — 来源

## 可选：财经 / 其他体育
（用户要时再加）

## 一句话观察
```

短版：每块 1～2 条 bullet 即可。

## 触发示例

- 「今天 AI 和苏超有什么新闻」
- 「最新 AI 大模型动态 + 苏超赛况」
