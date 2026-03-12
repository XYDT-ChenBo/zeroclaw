## `/response` 多轮对话设计方案（草案）

### 1. 目标与约束

- **目标**
  - 让 `/response` 接口具备多轮对话能力：
    - 同一业务会话下，多次调用 `/response` 能共享上下文。
    - 在每次推理时能携带部分历史对话，加速并稳定模型推理效果。
- **约束**
  - 尽量复用 ZeroClaw 现有能力（`agent/loop_.rs`、`memory` 等）。
  - 初期实现可以不做“超长历史的高级压缩”，先支持基本多轮即可。
  - 方案不强制落地代码，仅作为未来实现参考。

### 2. 会话标识（Session ID）设计

- **会话 key 来源**
  - 请求方（前端 / 上游服务）为每条会话提供一个稳定的业务 key，例如：
    - 工单号：`ticket_12345`
    - 项目/任务 ID：`project_abc`
    - 用户会话：`user_{user_id}_date_{yyyyMMdd}`
- **Session ID 生成规则**
  - 最简单方案：**直接用业务 key 作为 `session_id`**。
  - 如需隐藏或压缩，可在内部做一次变换（例如 hash / UUID，但对外仍叫 `session_id`）。
- **一致性要求**
  - 同一条业务会话整个生命周期内，都必须使用相同的 `session_id` 调 `/response`。
  - 新会话时，由上游生成一个新的 `session_id`。

### 3. `/response` 请求与响应协议扩展（概念层）

- **请求体增加字段（示意）**
  - `session_id: string`（可选，但开启多轮对话时建议必填）。
- **语义**
  - 当 `session_id` 存在时：
    - 这一次 `/response` 属于某个持续会话。
    - 服务端需要在内部按 `session_id` 存取历史消息。
  - 当 `session_id` 缺失时：
    - `/response` 按当前实现执行，视为单轮调用，不做跨调用会话串联。

### 4. 会话历史的存储策略

#### 4.1 进程内存储（简单版）

- **结构**
  - 在网关 / `_nodes/response.rs` 所在节点维护一个：
    - `HashMap<String, Vec<ChatMessage>>`
    - key 为 `session_id`，value 为该会话下的历史轮次（`ChatMessage` 序列）。
- **写入流程**
  - 收到 `/response` 请求：
    1. 从 `session_id` 查出已有 `Vec<ChatMessage>`，不存在则新建空向量。
    2. 将本次用户输入作为 `ChatMessage::user(...)` 追加进该向量。
    3. 调用 agent / provider 时，将该向量作为历史上下文一并传入。
    4. 得到模型回复后，将 `ChatMessage::assistant(...)` 也 append 回该向量。
- **特点**
  - 实现简单，无需修改持久化层。
  - 进程重启则会话历史丢失，适合 Demo 或对跨重启不敏感的场景。

#### 4.2 利用 Memory 后端存储（持久化版）

- **利用现有抽象**
  - `MemoryEntry` 已包含：
    - `category: MemoryCategory::Conversation`
    - `session_id: Option<String>`
  - `Memory` trait 已支持：
    - `store(..., session_id: Option<&str>)`
    - `recall(..., session_id: Option<&str>)`
    - `list(..., session_id: Option<&str>)`
- **写入策略**
  - 每一轮 `/response` 至少写入两类记录：
    - 当前用户消息：`user` turn
    - 当前模型回复：`assistant` turn（可选，可视需求）
  - 字段建议：
    - `category`：`MemoryCategory::Conversation`
    - `session_id`：`Some(session_id)`（关键）
    - `key`：结构化命名，方便后续操作，例如：
      - `"{session_id}:user:{turn_index}"`
      - `"{session_id}:assistant:{turn_index}"`
      - 或带时间戳：`"{session_id}:user:{timestamp}"`。
- **读取策略**
  - 下次同一 `session_id` 调用 `/response` 时：
    - 使用 `list(Some(&MemoryCategory::Conversation), Some(session_id))` 拉取该会话下所有历史条目，
      - 按时间或 turn_index 排序，
      - 只取最近 N 条（滑动窗口）拼接为历史。
    - 或根据当前用户 query 使用：
      - `recall(query, limit, Some(session_id))`
      - 做“会话内语义检索”，只选出与当前问题最相关的若干条。

- **优点**
  - 会话历史可跨进程重启保留。
  - 不同 session_id 完全隔离，易于做归档和清理。

### 5. 历史注入推理上下文的方式

无论选择 4.1 还是 4.2，核心是**把选出来的历史对话变成模型的上下文**，可以分两层：

- **层 1：对话式 history（ChatMessage 序列）**
  - 历史记录按顺序转换为 `Vec<ChatMessage>`：
    - `role = "user"` / `"assistant"`。
  - 作为 `history` 传入底层 agent / provider（类似现有 `agent/loop_.rs` 对 channel 的处理）。
  - 通常只取最近 N 轮，以控制 token 大小。

- **层 2：抽象记忆 context（可选增强）**
  - 当会话很长时，可以为老的对话做总结：
    - 使用单独的一次模型调用，把 session 早期消息压缩成一条/几条 summary。
    - 以 `MemoryCategory::Daily` 或 `Custom("session_summary")` 储存，仍关联同一个 `session_id`。
  - 每次 `/response` 时：
    - 先查询该 `session_id` 最新的 summary（少量几条），
    - 组成类似：
      - `[Session context]\n- ...`
    - 作为 system prompt 或用户 prompt 之前的一段额外文本注入。

> 初次实现可先只做“最近 N 轮原文对话”，后续再迭代引入“summary + 最近若干轮”的混合策略。

### 6. 历史长度与清理策略

- **简单限长策略**
  - 为每个 `session_id` 维护一个最大历史条数（例如 N = 10 或 20）：
    - 超出时只保留最新 N 条。
  - Memory 后端场景下，可在写入前或后台任务里清理旧条目。
- **会话结束/归档**
  - 可以在协议层提供一个“结束会话”的指令（例如某个特殊 action 或额外 API）：
    - 删除该 `session_id` 下的 `Conversation` 条目，
    - 或把摘要迁移到长期记忆（`Core`）后再清理。
- **自动过期**
  - 定期扫描 Memory 或内存中的 `session_id`：
    - 若某个会话长时间未访问，则清除其历史，避免无限增长。

### 7. 与现有实现的集成思路（概念级）

- **入口层（`_nodes/response.rs` 或 Gateway API）**
  - 从 HTTP 请求中解析 `session_id`。
  - 构造内部调用 context 时，把 `session_id` 传递到 agent / memory 相关逻辑中。
- **Agent 层（`agent/loop_.rs` 及相关）**
  - 在构建 `history` 时，额外合并：
    - 当前 session 对应的历史对话（见第 5 节）。
  - 在本轮完成后，负责：
    - 将本轮 user / assistant 消息写入对应的会话存储（内存或 Memory 后端），带上 `session_id`。
- **内存/持久化层（`memory/*`）**
  - 不需要改接口（`session_id` 已存在），只需在使用时填入合适的 `Some(session_id)`，并约定好 `key` 命名规则。

### 8. 实施优先级建议

- **第一阶段（最小可用）**
  - 在 `/response` 协议中增加 `session_id` 字段。
  - 在进程内维护 `HashMap<session_id, Vec<ChatMessage>>`。
  - 每次带上最近 N 条历史进行对话。
- **第二阶段（持久化 + 清理）**
  - 将 per-session 历史迁移到 Memory 后端：
    - 写入时使用 `session_id`。
    - 读取时通过 `list/recall + session_id` 获取最近 N 条。
  - 增加历史清理和过期策略。
- **第三阶段（高级总结与语义检索，可选）**
  - 对长会话定期生成 summary，并在上下文中使用：
    - “summary + 最近若干轮原文 + 当前用户输入”。
  - 为复杂场景（如任务协作、长期项目）提供更鲁棒的上下文管理。

