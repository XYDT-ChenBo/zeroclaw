
description: 通用节点控制技能,向已配对的安卓设备/手机的控制节点下发控制指令，如灯光控制、音量控制、投屏、语音播报等功能。

# 通用节点控制

## 执行方式（必读）

- 必须使用 **nodes** 工具的 **action: "invoke"** 下发指令。

- 调用时传入参数：
  - **action**: 固定为 `"invoke"`
  - **node**: 节点 ID、显示名称或 IP（必填）
  - **invokeCommand**: 下表对应的指令名（必填）
  - **invokeParamsJson**: 仅当指令需要参数时传入，为 **字符串形式的 JSON**（如 `"{\"percentage\": 75}"`）；无参数指令可不传或传 `"{}"`。
  
- **强约束** :如果判断没有在线节点，要使用**nodes** 工具 **action: "status"** 查看已配对节点状态；若缓存有在线节点可不查询。

## 工具调用示例

**无参数指令（如打开灯光）：**

```json
{
  "action": "invoke",
  "node": "Redmi",
  "invokeCommand": "flashlight.turnOn"
}
```

**有参数指令（如设置音量）：**

```json
{
  "action": "invoke",
  "node": "Redmi",
  "invokeCommand": "volume.set",
  "invokeParamsJson": "{\"percentage\": 75}"
}
```

**有参数指令（如投屏到指定设备）：**

```json
{
  "action": "invoke",
  "node": "Redmi",
  "invokeCommand": "cast.autoCast",
  "invokeParamsJson": "{\"devicename\": \"客厅电视\"}"
}
```

**有参数指令（如语音播报）：**

```json
{
  "action": "invoke",
  "node": "Redmi",
  "invokeCommand": "ztellm.speakTTS",
  "invokeParamsJson": "{\"msg\": \"TTS文本\"}"
}
```
可先用 **nodes** 工具 **action: "status"** 查看已配对节点，再用 **action: "invoke"** 下发指令。

## 前置条件

- 目标节点已与 Gateway 配对且在线（可用 nodes 工具 action status 确认）。
- 若为安卓节点，需在网关配置中允许相应命令（如 `gateway.nodes.allowCommands` 包含 `flashlight.turnOn`、`volume.set`、`cast.autoCast`、`cast.previousPhoto`、`cast.nextPhoto`、`ztellm.speakTTS` 等），否则 invoke 可能被拒绝。

## 指令一览

| 分类 | 意图         | invokeCommand           | 参数 | 说明 |
|------|--------------|-------------------------|------|------|
| 灯光 | 打开灯光     | `flashlight.turnOn`     | 无   | 不传 invokeParamsJson 或传 `"{}"` |
| 灯光 | 关闭灯光     | `flashlight.turnOff`    | 无   | 同上 |
| 灯光 | 切换灯光     | `flashlight.toggle`     | 无   | 同上 |
| 音量 | 设置音量     | `volume.set`            | 必填 | invokeParamsJson 为 `"{\"percentage\": <0-100>}"` |
| 投屏 | 开始投屏     | `cast.autoCast`         | 可选 | invokeParamsJson 可为 `"{\"devicename\": \"投屏设备名称\"}"`，不传则仅 invokeCommand |
| 投屏 | 停止投屏     | `cast.stopCast`         | 无   | 同上 |
| 投屏 | 查看上一张照片 | `cast.previousPhoto`    | 无   | 仅 invokeCommand |
| 投屏 | 查看下一张照片 | `cast.nextPhoto`        | 无   | 同上 |
| 投屏 | 删除当前照片并刷新 | `cast.deleteAndRefresh` | 无   | 同上 |


### 音量参数说明

- `volume.set` 的 **invokeParamsJson** 必须为 JSON 字符串，且包含 **percentage**（0–100 的整数）。
- 示例：`"{\"percentage\": 75}"`（75%）、`"{\"percentage\": 0}"`（静音）、`"{\"percentage\": 100}"`（满音量）。

### 投屏参数说明

- `cast.autoCast` 可选传入 **invokeParamsJson**，为 JSON 字符串，包含 **devicename**（投屏设备名称）。指定设备时传入，不指定可不传或传 `"{}"`。
- 示例：`"{\"devicename\": \"客厅电视\"}"`、`"{\"devicename\": \"会议室投影\"}"`。

### 语音播报参数说明

- `ztellm.speakTTS` 的 **invokeParamsJson** 必须为 JSON 字符串，且包含 **msg**（要播报的 TTS 文本）。
- 示例：`"{\"msg\": \"你好，这是语音播报\"}"`。

## 使用流程

1. **确认节点**：从用户输入或先调 nodes(status) 获取目标 node（ID、名称或 IP）。
2. **识别意图**：根据用户表述选择上表中的 invokeCommand（灯光开/关/切换、音量百分比、投屏开始/停止/上一张/下一张/删除刷新、语音播报）。
3. **调用工具**：使用 **nodes** 工具，**action 设为 "invoke"**，传入 **node**、**invokeCommand**；若为 `volume.set` 再传 **invokeParamsJson**（如 `"{\"percentage\": 75}"`）；若为 `cast.autoCast` 且用户指定了投屏设备，传 **invokeParamsJson**（如 `"{\"devicename\": \"投屏设备名称\"}"`）；若为 `ztellm.speakTTS` 再传 **invokeParamsJson**（如 `"{\"msg\": \"要播报的文本\"}"`）。
4. **回复结果**：根据返回简要告知用户是否成功。

## 成功后的回复

- 打开灯光：**灯光已打开**
- 关闭灯光：**灯光已关闭**
- 切换灯光：**已切换灯光状态**
- 设置音量：**音量已设置为 XX%**（XX 为实际 percentage）
- 开始投屏：**投屏已开始**（若指定了设备可补充“已投到 XXX”）
- 停止投屏：**投屏已停止**
- 查看上一张照片：**已切换到上一张照片**
- 查看下一张照片：**已切换到下一张照片**
- 删除并刷新：**当前照片已删除并刷新**
- 语音播报：**已播报** 或 **语音已播报：XXX**（XXX 为播报内容摘要）

## 指令与触发对照

| 指令 | 触发场景示例 |
|------|----------------|
| `flashlight.turnOn` | “打开灯光”“开手电筒”“打开手电”“亮灯” |
| `flashlight.turnOff` | “关闭灯光”“关手电筒”“关闭手电”“关灯” |
| `flashlight.toggle` | “闪光”“切换灯光”“手电筒闪一下” |
| `volume.set` | “音量调到 75”“设置音量为 50%”“静音”“音量调满” |
| `cast.autoCast` | “开始投屏”“手机投屏”“打开投屏”“投到客厅电视”等（可带投屏设备名称） |
| `cast.stopCast` | “停止投屏”“结束投屏”“关闭投屏” |
| `cast.previousPhoto` | “查看上一张照片”“上一张”“上一张图” |
| `cast.nextPhoto` | “查看下一张照片”“下一张”“下一张图” |
| `cast.deleteAndRefresh` | “删除当前照片”“删掉当前这张图”“删除并刷新” |