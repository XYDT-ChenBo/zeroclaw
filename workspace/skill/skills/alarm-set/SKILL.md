---
name: alarm
description: >
  通过 `shell` 工具发送广播，与 `com.haobai.clock` 对接，实现闹钟的增、删、停、查。只要用户表达“闹钟”的需求，就应优先使用本 Skill。
  必须使用 `shell`，禁止使用 `nodes`、`cron_add`、`schedule` 等其他工具。
  必须完整读取本 Skill 的全部规则后才能执行；禁止凭经验直接拼装广播命令。
---

# 闹钟 Skill

本 Skill 用来把用户的自然语言闹钟需求，转成符合 `com.haobai.clock` 协议的 JSON，
再通过`shell` 工具下发广播。

- **强约束**
所有闹钟 payload 都通过
`com.haobai.clock.SET_ALARM` 广播的 `remindInfo` extra 传递，其值是一段 JSON 字符串，严格参考格式：
### 固定命令模板（严禁自行发挥）

你只能发送下面这种形态的广播（action 与 extra key 都是固定不变量）：

```bash
am broadcast -a com.haobai.clock.SET_ALARM --es remindInfo '<JSON_PAYLOAD>'
```

其中 `<JSON_PAYLOAD>` 必须是 **紧凑 JSON 字符串（单行）**，例如（一次性闹钟 `timingInfo`）：

```bash
am broadcast -a com.haobai.clock.SET_ALARM --es remindInfo '{"skill":"reminderSkill","intense":"timingInfo","entity":{"remidObject":"提醒对象","event":"起床提醒","timeParams":[{"remindDate":"2026-03-16","remindTime":"14:44:00","aftertime":0,"isFuture":true}]}}'
```


## 总体流程（从语义到 payload）

处理任何“闹钟”相关请求时，请按照下面步骤：

1. 理解用户意图：是**创建**、**更新**、**删除**、**停止响铃**还是**查询闹钟列表**。
2. 在用户所在时区，把日期时间规范化为：
   - 日期：`yyyy-MM-dd`
   - 时间：`HH:mm:ss`
3. 选择合适的 `intense` 值（见下文各小节）。
4. 根据《闹钟对接文档》构造 `entity` 对象。
5. 最终组合成：
   ```json
   { "skill": "...", "intense": "...", "entity": {  } }
   ```
6. 将该对象序列化为紧凑 JSON 字符串（单行）。
7. 把该 JSON 字符串作为 `--es remindInfo` 的值拼接到固定命令，再调用 `shell`。

## 附加参数 JSON 构建规则（必须执行）

处理任何闹钟请求时必须执行以下硬流程，不得跳步：

1. **意图判定**：先确定 `intense`。
2. **实体构建**：再构建 `entity`。
3. **顶层封装**：`payload = { skill, intense, entity }`。
4. **序列化**：把 `payload` 转成紧凑 JSON 字符串（单行）。
5. **命令拼接**：使用固定前缀 + `--es remindInfo 'JSON_PAYLOAD'`。
6. **发送**：仅使用 `shell` 工具执行广播。

只要缺失第 3-5 任一步，就视为未完成任务，必须补齐后再发送命令。

### 发送前自检（逐项检查）

- 是否已生成 `payload`（而不是只生成局部字段）。
- `payload` 顶层是否包含 `skill`、`intense`、`entity`。
- `remindInfo` 是否为“字符串化 JSON”，而不是对象字面量。
- 命令中是否出现固定参数：`-a com.haobai.clock.SET_ALARM --es remindInfo`。
- 引号是否闭合，参数是否为单个完整字符串。
- `--es remindInfo` 是否只出现一次，且拼写/大小写完全一致。


## 常见意图与 `intense` 映射

### 一次性闹钟（timingInfo）

- `intense = "timingInfo"`
- 适用：用户说“明天早上 7 点叫我起床”“3 月 20 日 8:30 提醒我开会”等一次性时间点。
- 字段约定：
  - `entity.event`：闹钟标题。
  - `entity.timeParams[0].remindDate`：`yyyy-MM-dd`。
  - `entity.timeParams[0].remindTime`：`HH:mm:ss`。
  - `entity.timeParams[0].isFuture = true`。
  - `entity.timeParams[0].aftertime = 0`（当前实现未使用，固定 0）。

### 时间段类一次性提醒（periodTimeInfo）

- `intense = "periodTimeInfo"`
- 适用：文档中说明与 `timingInfo` 结构一致，可视为另一种“一次性”入口。
- 字段结构与 `timingInfo` 完全相同，仅 `intense` 不同。

### 循环闹钟（cycleTimeInfo）

统一使用 `intense = "cycleTimeInfo"`，通过 `entity.repeatRule.frequency` 区分频率：

- 公共字段：
  - `entity.event`：闹钟标题。
  - `entity.repeatRule.frequency`：`"daily" | "weekly" | "monthly" | "yearly"`。
  - `entity.repeatRule.timeParams[*]`：按频率填充不同字段。

#### 每天（daily）

- `entity.repeatRule.frequency = "daily"`
- 至少：
  - `entity.repeatRule.timeParams[0].timePoint`：`HH:mm:ss`。

#### 每周（weekly）

- `entity.repeatRule.frequency = "weekly"`
- 每个 timeParams：
  - `timePoint`：`HH:mm:ss`
  - `weekPoint`：`Monday` / `Tuesday` / `Wednesday` / `Thursday` / `Friday` / `Saturday` / `Sunday`
- 示例：每周一、三 8 点，可生成两个 timeParams，weekPoint 分别为 `Monday` / `Wednesday`。

#### 每月（monthly）

- `entity.repeatRule.frequency = "monthly"`
- 至少：
  - `timeParams[0].monthPoint`：几号，字符串，例如 `"15"`。
  - `timeParams[0].timePoint`：`HH:mm:ss`。

#### 每年（yearly）

- `entity.repeatRule.frequency = "yearly"`
- 字段含义按现有实现：
  - `timeParams[0].yearPoint`：月份（字符串，如 `"10"` 表示 10 月）。
  - `timeParams[0].monthPoint`：日期（字符串，如 `"1"` 表示 1 日）。
  - `timeParams[0].timePoint`：`HH:mm:ss`。

### 删除闹钟（deleteRecordInfo）

删除类操作统一使用 `intense = "deleteRecordInfo"`，通过 `entity.deletype` 决定删除方式。
`deletype` 为字符串数组，如 `["1"]`、`["2"]`，含义参见对接文档：

#### 按日期 + 时间删除（deletype = "1"）

- `entity.deletype = ["1"]`
- `entity.timeParams.remindDate`：
  - 支持 `yyyy-MM-dd` 或 `yyyy年M月d日`（可不补零）
- `entity.timeParams.remindTime`：`HH:mm:ss`

#### 按名称删除（deletype = "2"）

- `entity.deletype = ["2"]`
- `entity.event`：闹钟标题。

#### 按索引删除（deletype = "3"）

- `entity.deletype = ["3"]`
- `entity.index`：字符串数字，从 1 开始（`"2"` 表示列表中第 2 个）。

#### 删除循环闹钟（deletype = "4"）

- `entity.deletype = ["4"]`
- `entity.repeatRule` 结构参考创建循环闹钟时的填法：
  - `frequency`：`daily` / `weekly` / `monthly` / `yearly`
  - `timeParams[*]`：按对应频率填充 `timePoint`、`weekPoint`、`monthPoint`、`yearPoint`。

#### 删除全部闹钟（deletype = "5"）

- `entity.deletype = ["5"]` 即可，无需额外字段。

### 停止响铃与查询闹钟

#### 停止当前响铃/倒计时（killRecordInfo）

- `intense = "killRecordInfo"`
- `entity` 可省略或使用空对象 `{}`。

#### 查询并播报当前闹钟（checkRecordInfo）

- `intense = "checkRecordInfo"`
- `entity` 同样可省略或使用空对象 `{}`。

## 调用 `shell` 工具的方式（标准模板）

当你已经构造好完整 JSON 对象 `{ skill, intense, entity }` 后：

1. 先得到 `payload = { skill, intense, entity }`。
2. 序列化：`JSON_PAYLOAD = <payload 的紧凑 JSON 字符串>`。
3. 拼接并调用 `shell`：
   ```bash
   am broadcast -a com.haobai.clock.SET_ALARM --es remindInfo 'JSON_PAYLOAD'
   ```
4. 禁止使用：`nodes`（包括 action=run/notify 等）、`cron_add`、`schedule`。
5. `com.haobai.clock.SET_ALARM` 与 `remindInfo` 是固定值，不可修改。

示例：
```bash
am broadcast -a com.haobai.clock.SET_ALARM --es remindInfo '{"skill":"reminderSkill","intense":"timingInfo","entity":{"remidObject":"提醒对象","event":"起床提醒","timeParams":[{"remindDate":"2026-03-16","remindTime":"14:44:00","aftertime":0,"isFuture":true}]}}'
```

