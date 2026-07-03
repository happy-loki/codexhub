# CodexHub Agent Manager 实施计划

## 项目概述

### 目标

为 CodexHub 用户提供可视化的 Codex Agent（角色）管理能力，让个人用户无需手写 TOML 就能创建、编辑、管理自定义 agent role，并在此基础上逐步加入 agent 之间协作的可视化。

### 优先级原则

本计划分两个阶段推进，顺序不可颠倒：

1. 阶段一（先做）：Agent 集成与管理。把 agent 的读取、展示、编辑、模板落地，让用户真正能在 GUI 里管理自己的 agent。
2. 阶段二（后做）：Agent 交互可视化。在阶段一稳定之后，再做调用链追踪与树形可视化。

先把「能用、好用」的管理能力做扎实，再考虑「看得见」的协作可视化。

### 目标用户

个人 Codex 用户（VSCode / Desktop / CLI），尤其是希望定制 agent 行为但不熟悉 TOML 配置的开发者。

---

## 背景：Codex 的 Agent Role 机制（必读）

在动手之前，必须理解 Codex 真实的 agent 机制。以下结论来自对 `references/codex-main` 源码的核对，而非假设。

### 核心事实

1. Codex 里 agent 的正式概念是 **agent role（角色）**，由多 agent 工具在 spawn 子 agent 时选用。相关代码见 `codex-rs/core/src/agent/role.rs` 与 `codex-rs/core/src/config/agent_roles.rs`。
2. Role 有两种来源：
   - 内置 role：编译进二进制，如 `explorer`、`awaiter`（见 `codex-rs/core/src/agent/builtins/`）。
   - 用户 role：在配置目录下发现和加载。
3. 用户 role 的存放位置是 `<config_folder>/agents/` 目录，递归扫描其中所有 `.toml` 文件（见 `agent_roles.rs` 的 `discover_agent_roles_in_dir` 与 `collect_agent_role_files`）。`config_folder` 即 `$CODEX_HOME`（默认 `~/.codex`）。
4. Role 也可以在 `config.toml` 里通过 `[agents.<role_name>]` 声明，并用 `config_file` 指向一个独立 role 文件。
5. Role 本质是「config.toml 的一个高优先级配置层」：spawn 时把 role 的配置叠加到当前会话配置上，但调用方当前的 `model_provider` 和 `service_tier` 保持粘性，除非 role 显式覆盖。

### Role 文件的真实格式

独立 role 文件的顶层字段（见 `agent_roles.rs` 的 `RawAgentRoleFileToml`）：

- `name`：role 名称。独立文件必须提供非空 `name`（或由声明处的 hint 提供）。
- `description`：role 描述。
- `nickname_candidates`：候选昵称列表（可选）。
- 其余所有字段：直接扁平（serde flatten）成一个 `ConfigToml` 层，也就是和 `config.toml` 同构的配置项。

换句话说，role 文件里除了 `name`/`description`/`nickname_candidates` 之外，写的都是标准的 Codex 配置键，例如 `model`、`model_reasoning_effort`、`developer_instructions`、`approval_policy` 等，而不是自定义的 `[model]`/`[system]`/`[tools]` 段。

一个内置 role（`awaiter.toml`）的真实片段：

```toml
background_terminal_max_timeout = 3600000
model_reasoning_effort = "low"
developer_instructions = """You are an awaiter.
Your role is to await the completion of a specific command or task ...
"""
```

一个典型的用户自定义 role 文件应长这样：

```toml
name = "code-reviewer"
description = "专业代码审查，聚焦缺陷、性能与安全"
nickname_candidates = ["reviewer", "cr"]

model = "gpt-5-codex"
model_reasoning_effort = "high"
approval_policy = "on-request"
developer_instructions = """
你是一个专业的代码审查员，聚焦：
1. 潜在缺陷与边界条件
2. 性能瓶颈
3. 安全风险
审查时给出可执行的修改建议，并引用具体文件与行号。
"""
```

### 对本计划的直接影响

- Agent Manager 管理的对象就是 `$CODEX_HOME/agents/` 下的 role 文件。
- 编辑器的表单字段必须对齐 Codex 真实配置键（`model`、`model_reasoning_effort`、`developer_instructions` 等），不能凭空发明字段结构。
- 生成的 TOML 必须能被 Codex 的 `parse_agent_role_file_contents` 成功解析，这是硬性验收标准。

---

## 阶段一：Agent 集成与管理（先做）

这是本计划的重心。目标是让用户在 CodexHub GUI 里完整管理自己的 agent role。

### 功能 1：Agent 列表

需求：
- 扫描 `$CODEX_HOME/agents/` 下的所有 `.toml`（递归），解析为 role 列表。
- 同时读取 `config.toml` 中 `[agents.<name>]` 声明的 role，合并展示（标注来源）。
- 展示字段：name、description、来源（文件 / config 声明 / 内置）、文件路径、最近修改时间。
- 支持搜索、排序、删除、打开所在目录。
- 对解析失败的文件不崩溃，单独列出并显示错误原因。

数据模型：

```rust
// src/agent_manager/model.rs
pub struct AgentRole {
    pub name: String,
    pub description: Option<String>,
    pub nickname_candidates: Vec<String>,
    pub source: AgentSource,          // File | ConfigDeclared | BuiltIn
    pub file_path: Option<PathBuf>,   // 独立文件才有
    pub modified_at: Option<DateTime<Utc>>,
    pub raw_config: toml::Value,      // 扁平的配置层，保留未知字段
    pub parse_error: Option<String>,  // 解析失败时填充
}

pub enum AgentSource {
    File,
    ConfigDeclared,
    BuiltIn,
}
```

关键点：`raw_config` 保留原始配置层，避免编辑时丢失我们没建模的字段。

### 功能 2：Agent 可视化编辑器

需求：
- 表单化编辑 role 的常用字段，右侧实时预览生成的 TOML。
- 保存时写入 `$CODEX_HOME/agents/<name>.toml`。
- 支持从现有 role 克隆。
- 表单只暴露常用字段，其余不认识的字段保留在 `raw_config` 中原样写回（round-trip 安全）。

表单字段（对齐 Codex 真实配置键）：

| 字段 | TOML 键 | 类型 | 必填 | 说明 |
|------|---------|------|------|------|
| 名称 | `name` | 文本 | 是 | role 名称，同时作为文件名 |
| 描述 | `description` | 多行文本 | 建议 | role 用途 |
| 昵称 | `nickname_candidates` | 标签输入 | 否 | 候选昵称 |
| 模型 | `model` | 下拉/文本 | 否 | 模型 slug |
| 推理强度 | `model_reasoning_effort` | 下拉 | 否 | low / medium / high |
| 审批策略 | `approval_policy` | 下拉 | 否 | untrusted / on-request / on-failure / never |
| 开发者指令 | `developer_instructions` | 多行文本 | 建议 | role 的系统级指令 |

说明：字段清单以 Codex `ConfigToml` 实际支持项为准，随版本可能增减。表单未覆盖的键通过「高级（原始 TOML）」编辑区兜底。

TOML 生成策略：
- 用 `toml` crate 从结构化数据序列化，而不是字符串拼接，避免转义与格式问题。
- 以 `raw_config` 为基底，用表单值覆盖已知键，未知键原样保留。
- 保存前先在内存里跑一遍与 Codex 等价的解析校验（复用同一套 `toml` 解析 + 必填校验）。

### 功能 3：官方 Agent 模板库

需求：
- 内置一组精品 role 模板，一键复制到 `$CODEX_HOME/agents/`。
- 复制时让用户确认 role 名称（即文件名），避免重名覆盖。

初始模板（每个都是合法的 role 文件）：

1. Code Reviewer：代码审查，`model_reasoning_effort = "high"`。
2. Doc Writer：文档撰写，强调结构化输出。
3. Test Engineer：测试编写，覆盖率优先。
4. Bug Fixer：缺陷定位与修复，谨慎改动。
5. Explorer（增强版）：代码库探索与检索。

实现：

```rust
// src/agent_manager/templates.rs
pub struct AgentTemplate {
    pub id: &'static str,
    pub display_name: &'static str,
    pub summary: &'static str,
    pub toml_content: &'static str, // include_str! 引入，本身即合法 role 文件
}
```

模板的 `toml_content` 必须能通过和 Codex 一致的解析校验，作为单元测试固定下来。

### 阶段一验收标准

- 能递归扫描并展示 `$CODEX_HOME/agents/` 下所有 role，含 config 声明的 role。
- 单个文件解析失败不影响其余 role 展示，错误可见。
- 能通过表单创建 role，生成的 TOML 能被 Codex 解析逻辑接受。
- 能编辑现有 role 且不丢失表单未覆盖的字段（round-trip 安全）。
- 能克隆 role、删除 role（删除有二次确认）。
- 能从至少 5 个内置模板一键创建 role。
- 列表加载 100 个 role 在 500ms 内完成。

---

## 阶段二：Agent 交互可视化（后做）

仅在阶段一稳定后启动。目标是让用户看见 agent 之间的协作过程。

### 功能 4：Agent 调用链追踪

需求：
- 监听 remote-control 通道里与 sub-agent 相关的消息，构建父子调用关系。
- 以树形结构实时展示调用链，标注状态（运行中 / 完成 / 失败）与耗时。
- 点击节点查看详情（输入/输出摘要、token 消耗、错误信息）。

前置调研（阶段二开工第一件事）：
- 核对 `src/remote_control_backend/` 中 `observe_app_server_message` 能观测到哪些 sub-agent 事件；确认字段是否足以还原父子关系。
- 若现有消息不足以还原调用树，需要先评估补齐事件的成本，再决定是否降级为「按会话平铺展示」。

这一阶段的技术选型（事件驱动、TreeCtrl）详见文末「技术可行性评估」。

---

## 技术架构

### 模块划分

```
src/
├── agent_manager/
│   ├── mod.rs            # 对外接口
│   ├── model.rs          # AgentRole 等数据模型
│   ├── scanner.rs        # 扫描 $CODEX_HOME/agents + config 声明
│   ├── writer.rs         # 表单 -> TOML 序列化 + 校验
│   └── templates.rs      # 内置模板
├── agent_tracker.rs      # 阶段二：调用链追踪
└── gui/
    ├── agent_list.rs     # 列表面板
    ├── agent_editor.rs   # 编辑对话框
    ├── agent_templates.rs# 模板选择
    └── agent_tree.rs     # 阶段二：调用链树形视图
```

### 复用现有能力

- TOML 读写复用项目已依赖的 `toml` crate。
- GUI 复用 `src/gui/` 既有模式：`DataViewCtrl` 列表、`card_section` 布局、`GuiMessage` + `on_idle` 事件循环。
- 配置目录定位复用项目里既有的 `$CODEX_HOME` 解析逻辑（见 `codex_app_config.rs`）。

### 数据流（阶段一）

```
扫描 agents/ 目录 + 读取 config.toml [agents.*]
        ↓
解析为 Vec<AgentRole>（保留 raw_config 与 parse_error）
        ↓
agent_list 面板展示
        ↓
编辑/克隆 -> agent_editor 表单
        ↓
writer 序列化 + 本地校验
        ↓
写回 $CODEX_HOME/agents/<name>.toml -> 刷新列表
```

---

## 实施时间表

时间为估算，按个人开发者节奏，可按实际调整。阶段一务必先完成再进入阶段二。

### 阶段一（约 3-4 周）

周次 1：扫描与列表
- 定位 `$CODEX_HOME` 与 `agents/` 目录，实现递归扫描。
- 实现 role 文件解析（复用与 Codex 一致的 `toml` 解析路径），保留 `raw_config`。
- 合并 `config.toml` 的 `[agents.*]` 声明。
- 单元测试：覆盖合法文件、损坏文件、重名、空目录。

周次 2：列表 GUI
- 新增「Agent」标签页与列表面板。
- 展示 name/description/来源/路径/修改时间；实现搜索、排序。
- 详情面板 + 删除（二次确认）+ 打开目录。

周次 3：编辑器与 TOML 生成
- 表单对话框 + 右侧实时 TOML 预览。
- writer：以 `raw_config` 为基底做 round-trip 序列化。
- 保存前本地校验；错误提示；克隆功能。

周次 4：模板库与打磨
- 5 个内置模板（含解析校验单测）。
- 模板选择对话框 + 一键复制 + 命名确认。
- 集成测试与边界打磨。

### 阶段二（约 2 周，阶段一完成后启动）

周次 5：调用链数据
- 核对 remote-control 可观测事件，确认能否还原父子关系。
- 实现 `agent_tracker`：接收事件、维护调用树、记录状态与耗时。
- 验证 `DataViewTreeCtrl` 可用性（详见技术评估）。

周次 6：调用链可视化
- 树形视图 + 实时更新（事件驱动）+ 节点详情。
- 状态颜色编码；集成测试；性能与稳定性打磨。

---

## 风险与缓解

风险 1：对 Codex role 格式理解偏差。
- 缓解：以 `references/codex-main` 源码为准（已核对 `agent_roles.rs`、`role.rs`）；生成的 TOML 必须通过与 Codex 一致的解析校验；保留 `raw_config` 做 round-trip，避免丢字段。

风险 2：Codex 未来调整 role 格式或配置键。
- 缓解：表单只覆盖稳定的常用键，其余走「原始 TOML」兜底；解析失败降级为只读展示而非报错崩溃。

风险 3：remote-control 未暴露足够的 sub-agent 事件（影响阶段二）。
- 缓解：阶段二开工先做事件调研；若不足，降级为「按会话平铺」展示，把完整调用树作为后续迭代。

风险 4：DataViewTreeCtrl 稳定性未知（影响阶段二）。
- 缓解：阶段二先做 POC 验证；失败则降级为缩进列表方案（见技术评估）。

风险 5：写入 `$CODEX_HOME` 的副作用。
- 缓解：写入前校验；重名给出覆盖确认；删除有二次确认；不触碰内置 role 与 `config.toml` 主体（仅在明确操作时才改动声明）。

---

## 未来迭代方向

- 阶段三：role 版本管理与回滚、role 使用统计。
- 阶段四：role 分享 / 导入导出、团队共享（Git 集成）。
- 更晚：调用链的成本分析、prompt A/B 对比。

企业级能力（集中管理、审计、配额、SSO）不在个人用户范围内，暂不规划。

---

## 技术可行性评估（wxdragon）

这一节主要服务阶段二，但结论也影响阶段一的实时刷新体验。

### 事件驱动：已验证可用

wxdragon 提供完整的事件系统（`vendor/wxdragon/rust/wxdragon/src/event/`），且 CodexHub **已经在用事件驱动架构**，不是纯轮询。核心模式见 `src/gui.rs`：

```rust
// 后台线程发消息 + 唤醒 GUI
let _ = gui_tx.send(GuiMessage::SomeAction(result));
wxdragon::wake_up_idle();

// GUI 主线程在 on_idle 里批量消费
frame.on_idle(move |event| {
    let mut processed = 0;
    while let Ok(message) = gui_rx.try_recv() {
        // 处理消息，更新 UI
        processed += 1;
        if processed >= 20 { break; }
    }
    if let WindowEventData::Idle(idle) = event {
        idle.request_more(processed >= 20);
    }
});
```

特性：`wake_up_idle()` 单帧内唤醒（约 16ms 级），空闲时 CPU 约 0%，批量消费避免阻塞 UI。

### 阶段二调用链的推荐做法

复用现有 `GuiMessage` 机制，新增一类消息即可，无需新架构：

```rust
enum GuiMessage {
    // ... 现有类型 ...
    AgentTracking(AgentTrackingEvent),
}

enum AgentTrackingEvent {
    Spawned { session_id: String, agent_id: String, parent_id: Option<String> },
    Completed { session_id: String, input_tokens: u32, output_tokens: u32 },
    Failed { session_id: String, error: String },
}
```

remote-control 侧在观测到 sub-agent 事件时 `send + wake_up_idle()`，GUI 侧在 `on_idle` 里更新树。延迟可做到 100ms 级。

关于 Timer：项目里的 `Timer`（`update.rs`、`session_history.rs`）用于定期拉取外部数据和超时控制，属于非关键定期任务；关键交互仍应走事件驱动。

### TreeCtrl：需 POC 验证，有备选

`DataViewTreeCtrl` 在 wxdragon 中存在（`vendor/wxdragon/rust/wxdragon/src/widgets/dataview/tree_ctrl.rs`），提供 `append_container` / `append_item` 等 API，但项目中尚无实际使用先例。阶段二开工先跑一个 POC（`tests/tree_ctrl_poc/` 已备好骨架）验证：创建节点、展开/折叠、选择、动态添加、状态更新。

备选方案（若 TreeCtrl 不达标）：用 `DataViewCtrl` + 缩进字符模拟树形，对 2-3 层调用链足够：

```
▼ Main Agent (2m34s)      [完成]
  ├─ Code Reviewer (45s)  [完成]
  │  └─ Test Engineer     [完成]
  └─ Doc Writer           [运行中]
```

### 结论

- 阶段一（管理）：全部落在已验证能力内，风险低。
- 阶段二（可视化）：实时性可达 100ms 级（事件驱动），唯一待验证项是 TreeCtrl，且有备选兜底。

---

## 附录

### 关键源码参考

- Agent role 应用逻辑：`references/codex-main/codex-rs/core/src/agent/role.rs`
- Role 加载与解析：`references/codex-main/codex-rs/core/src/config/agent_roles.rs`
- 内置 role 示例：`references/codex-main/codex-rs/core/src/agent/builtins/`（`awaiter.toml`、`explorer.toml`）
- 现有 GUI 事件循环：`src/gui.rs`
- remote-control 消息观测：`src/remote_control_backend/server_messages.rs`
- wxdragon 事件系统：`vendor/wxdragon/rust/wxdragon/src/event/`
- DataViewTreeCtrl：`vendor/wxdragon/rust/wxdragon/src/widgets/dataview/tree_ctrl.rs`

### 文档状态

- 版本：v2.0（重写并重排优先级：先集成管理，后交互可视化）
- 更新日期：2026-07-03
- 重要修订：基于源码核对，纠正了对 Codex agent role 机制与 role 文件格式的描述。
