# Anthropic Prompt Caching（cache_control）读写机制说明

状态：基础说明。本文记录 Anthropic Messages API `cache_control` 的读写机制、断点规则、与 Claude Code 的实际行为对照，并给出 AI Gateway（Responses ↔ Messages 转译）在多 `system` / 多 `messages` block 场景下的对齐策略。后续 `request.rs` 的缓存断点实现以本文为准。

相关文档：

- [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md)
- [`ai-gateway-anthropic-first-roadmap.zh-CN.md`](ai-gateway-anthropic-first-roadmap.zh-CN.md)
- Anthropic Prompt Caching: <https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- Anthropic Prompt Caching（镜像）: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching>

## 1. 核心模型：前缀缓存

`cache_control` 标记的语义不是「缓存这一个 block」，而是 **「缓存从请求开头到这个 block 为止的整个前缀」**。

前缀的拼接顺序固定为：

```
tools  →  system  →  messages
```

也就是说，一个断点缓存的内容 = 它前面的所有 tools + 所有 system + 截至该 block 的所有 messages。这条顺序是理解后续所有规则的基础：排在越前面的内容（tools）一旦变化，会让它后面所有缓存前缀全部失效。

## 2. 读与写是两套独立机制

这是最容易被官方「一个断点就够了」这句话误导的地方。读和写必须分开理解。

### 2.1 写（创建缓存）：只发生在显式断点上

- 缓存**只在你显式标了 `cache_control` 的 block 上被创建**。
- 没有标断点的位置，系统**不会主动写缓存**。
- 写入的是「截至该 block 的整个前缀」的一份缓存条目。

### 2.2 读（命中缓存）：才有自动回溯

- 请求带着断点进来时，系统从断点位置**向前回溯**（大约每 20 个 block 一个检查点），寻找**此前已经写过的**最长匹配前缀。
- 匹配到就部分命中，按命中长度计费为 `cache_read`。
- 回溯**不能凭空创建缓存**，它只能找回已经存在的缓存。

### 2.3 一句话总结

> **断点（写）负责把前缀「存下来」；回溯（读）负责把已存下来的前缀「找回来」。**
> 官方说「末尾一个断点就够」，前提是更靠前的前缀此前已经被写过缓存——回溯只是替你省去重复标注，不替你创建缓存。

## 3. 断点数量与限制

- 每个请求**最多 4 个 `cache_control` 断点**，超过直接报 400 错误。
- 这 4 个名额是 **tools + system + messages 全局共享**，不是每段各 4 个。
- 每段缓存前缀有**最小 token 门槛**：多数模型 1024 token，部分小模型 2048 token。不够长不会真正写入缓存（断点会被静默忽略）。
- TTL：默认 5 分钟（`{"type":"ephemeral"}`），每次命中刷新；可选 1 小时（`{"type":"ephemeral","ttl":"1h"}`）。

## 4. Claude Code 实测行为

抓包结果（以实测为准）：

| 维度 | Claude Code 行为 |
|------|------------------|
| `tools` | **不加** `cache_control` |
| `system` | **每个 block 都加** |
| `messages` | **只加 1 个断点**，落在会话尾部的一条消息上，role 不限（`user`/`assistant`），block 类型不限（`text`/`tool_result`/`tool_use` 都算） |

> 抓包实证（2026-07，claude-cli/2.1.185）：
>
> - 以 `tool_result` 结尾的请求（`anthropic2.json`，300 条消息）里，`cache_control` 直接打在末尾 `tool_result` block 上（与 `type`/`content`/`tool_use_id` 同级）。总断点数 = 2 条 system + 1 个末尾 tool_result = 3。
> - 以纯文本 user 结尾的请求（`anthropic.json`，302 条消息）里，断点没有落在最后一条 user（`msg[301]`），而是落在**倒数第二条 `role==assistant` 的 text block**（`msg[300]`）上。总断点数 = 2 条 system + 1 个 assistant = 3。
>
> 两条实证合起来说明：Claude Code 的消息断点是**单个、跟着会话尾巴走**的，承载它的既可以是 user/tool_result，也可以是 assistant/tool_use；messages 数组中间即便混入 `role:system`（mid-conversation-system）也不加。这也推翻了早期「assistant 消息永不标记」的假设。

### 4.1 为什么 system 每条都加？

Claude Code 的请求结构里 `system` 是**固定且少**的（通常 2 条）：

```
system[0]: "You are Claude Code, Anthropic's official CLI for Claude."   ← 极短、固定
system[1]: 一大段环境与指令                                               ← 大、基本不变
```

真正承重的是 **system 最后一条上的断点**——它把 `tools + system` 整段前缀写成一份独立缓存，边界正好卡在 `system` 与 `messages` 之间，于是 messages 怎么增长，这份 system 缓存都稳定复用。

system[0] 上那个断点其实是**冗余的廉价保险**：它太短（远不到 1024 token），自身不会真正落缓存，但标着也无害——因为 Claude Code 总断点数（2 条 system + 末尾 1~2 个会话断点）不超过 4，名额富裕，索性每个稳定边界都标。

### 4.2 为什么 tools 不加？

因为没必要。`tools` 排在前缀最前面，只要后面（`system` 或 `messages`）有任意一个断点，tools 会作为前缀的一部分**自动被缓存进去**。Claude Code 把宝贵的 4 个名额留给 system 末尾和会话末尾，靠前缀机制覆盖 tools，无需为它单独花一个断点。

### 4.3 用户自定义 / MCP tool 加不加？

不用单独加。MCP tool 与内置 tool 同在 `tools` 数组里，属于前缀最前段，同样被 system 末尾的断点覆盖。

需要注意的固有行为：**tools 列表一旦变化（含新增/删除 MCP 工具），整个缓存前缀失效**——因为 tools 在最前面，它变了，后面 system、messages 的缓存全部作废。这与是否在 tools 上加断点无关。

## 5. Gateway 对齐策略（Responses ↔ Messages）

Claude Code 敢「system 每条都加」，前提是它的 system block 数量固定且少。而 Gateway 是 Responses → Messages 转译，**Codex 可能塞进来多条 system block**。若照搬「每条都加」，光 system 就可能吃掉 5~8 个断点，**直接撞穿 4 个上限，整个请求被拒**。

因此 Gateway 采用受控的三断点策略：tools 尾部固定一个，system 只在承重断点打一个，messages 段再用单个断点跟着会话尾巴走。Claude Code 抓包中的 tools 本身不带断点；Gateway 在这里有意多加一个，以便 tools 前缀可以独立复用，同时仍为 system 和会话尾部保留足够名额。

| 维度 | Gateway 策略 | 理由 |
|------|--------------|------|
| `tools` | 剥离入站标记后，只在**最后一个工具定义**加 1 个断点 | 缓存完整 tools 前缀；工具列表不变时可独立复用，且只占 1 个名额 |
| `system` | **只在最后一条 text block 加 1 个断点** | 承重断点；靠自动回溯覆盖前面所有 system block；规避 4 上限 |
| `messages` | 只在**会话尾部一条**（最后一条 `role==user`/`assistant` 消息）加 1 个断点；落点优先该消息最后一个 `text` block，无 text 则最后一个 content block（覆盖 tool_result-only / tool_use-only） | 与 CC 原生一致；单断点靠 20-block 回溯窗口即可命中。详见第 6 节 |
| 顶层 `cache_control` | 不生成 | 非法字段 |
| `ttl` | 不生成 | 使用 Anthropic `ephemeral` 的默认时长；当前不映射 `prompt_cache_retention` |

断点预算：tools 尾部 1 个 + system 末尾 1 个 + messages 尾部 1 个，共 ≤3，稳在 4 以内。

## 6. messages 段：单断点跟会话尾巴

### 6.1 落点规则

Anthropic 的读机制是从断点**向前回溯，窗口约 20 个 block**，寻找此前已写过的缓存。Codex agent 循环每轮尾部只追加几条消息（典型：assistant text → assistant tool_use → user tool_result），远小于 20，单断点足以回溯命中上一轮写入。

- 认 `role=="user"` 与 `role=="assistant"` 的消息（tool_result 是 role=user、tool_use 是 role=assistant，agent 循环尾部天然覆盖）；跳过 mid-conversation `system`（动态提示，不适合作缓存锚）。
- 用 `rposition` 从尾部找最后一条合格消息；无合格消息时不打断点。
- 落点优先该消息**最后一个 `type=="text"` 的 block**；无 text 则**最后一个 content block**（覆盖 tool_result-only / tool_use-only 消息）。这与 Claude Code 抓包一致——它把断点打在尾块的 text 上。
- 幂等：目标 block 已有 `cache_control` 则不重复写。

### 6.2 关于历史 cache_control 被摘除

单断点跟尾巴时，断点每轮移动，上一轮标过的历史块这一轮会被摘除 `cache_control`。实测（第 9 节 3741→3742）显示这在某些时段会破坏前缀哈希、导致 miss（read 从 ~90k 崩到 ~13k），但也有大量时段不受影响（Anthropic 剔除 `cache_control` 后再算哈希）。这与 Claude Code 自身行为一致——它也是单断点跟尾巴。偶发 miss 的主因指向 Anthropic 服务端分片最终一致性（见第 9 节），非本地策略可根治，故与 Claude Code 保持一致的单断点是最简且风险最低的默认。

## 7. 响应侧用量统计

`convert_anthropic_response` 解析 usage 时拆分缓存 token：

```text
input_tokens
output_tokens
cache_creation_input_tokens   → cache_creation_tokens（本次写入缓存）
cache_read_input_tokens       → cached_tokens（本次命中缓存）
```

并支持 `cache_creation` 细分（`ephemeral_5m_input_tokens` / `ephemeral_1h_input_tokens`）。读取侧已兼容 1h，即便写入侧当前固定走 5m。

## 8. 实现状态

已落地（`request.rs`，与本文对齐）：

- `insert_tools_cache_control`：剥离入站工具标记后，只在**最后一个工具定义**统一生成断点，缓存完整 tools 前缀。
- `insert_system_cache_control`：数组分支只在**最后一条** text block 加断点，规避多 system block 撞 4 上限。
- `insert_message_cache_control`：`rposition` 找最后一条 `role=="user"`/`assistant` 消息（跳过 mid-conversation system）；`mark_message_breakpoint` 落在其最后一个 text block（无 text 则最后一个 content block），幂等。
- 主请求注入 `metadata.user_id`（`insert_metadata_user_id`，与 Claude Code 一致，跟会话稳定）。
- 测试：`builds_anthropic_text_request`、`marks_only_the_last_tool_cache_breakpoint`、`caches_conversation_tail_only`、`caches_trailing_assistant_message`、`marks_last_text_block_of_tail_message`、`marks_last_block_when_tail_has_no_text`、`marks_exactly_one_message_breakpoint` 覆盖三个分段断点及其落点。

> 注：请求 **headers / anthropic-beta / auth 保留 Claude Code 指纹**（user-agent、x-app、x-stainless-*、x-claude-code-session-id、Bearer、beta 列表含 `context-1m-2025-08-07` 等），是获取 1M 上下文等能力的前提。内部 web-search 独立请求（`internal_web_search_body`）整体仍模拟 Claude Code（含 `metadata.user_id`）。整体方向是**模仿 Claude Code**，唯一相对其表象的偏差是 messages 落点优先 text block（更贴近 CC 实际行为）。

未做（本次范围外）：`prompt_cache_retention = "1h"` → `cache_control.ttl` 透传，仍固定 `{"type":"ephemeral"}`（5m）。

## 9. 历史生产验证：偶发缓存 miss 根因分析

> 本节数据采集于旧版“双滚动消息断点”实现，用于保留偶发 miss 的排查证据。当前实现已经收敛为 tools、system、messages 各一个断点，其中 messages 仅保留单个会话尾部滚动点；不要把本节的旧断点数量当作当前请求形态。

### 9.1 验证数据

基于 SQLite `ai-gateway-request-logs` 的两组真实数据：

**正常区间（3542–3548，7 轮）**：
- System/tools 哈希：连续 7 轮 100% 一致
- Messages 每轮增长 3 条（assistant text → assistant tool_use → user tool_result）
- 当时的双滚动断点：每轮在最后两条消息上（如 msg[69-70] → msg[72-73] → msg[75-76]）
- Cache read：稳定在 21 万+ tokens，占总输入 92-94%
- Cache write：每轮 1-5k tokens（新增消息）
- **当时结论：旧版双滚动策略在该区间按预期工作，命中率稳定**

**异常区间（3521–3530，10 轮）**：
- System/tools 哈希：连续 10 轮 100% 一致
- 两次完全 cache miss：
  - **Request 3525**（23:15:45）：read=0, write=145437，距前一轮 3524 仅 9 秒
  - **Request 3529**（23:19:09）：read=0, write=168372，距前一轮 3528 共 183 秒（3 分钟）
- Miss 后立即恢复：
  - Request 3526（3525 后 5 秒）：read=129831，正好命中 3524 写入的量
  - Request 3530（3529 后 10 秒）：read=163266，命中 3528 写入的量

### 9.2 排除的可能原因

1. **前缀内容变化**：✗ System/tools 哈希在两组数据中全程一致
2. **`cache_control` 标记漂移**：✗ 对比发现正常区间（3542-3543）和异常区间（3524-3525）都存在历史消息 `cache_control` 被移除的情况，但前者不 miss，说明 Anthropic 服务端确实会过滤此字段后再计算哈希
3. **TTL 过期**：✗ 3525 距 3524 仅 9 秒，3529 距 3528 仅 183 秒，远小于 5 分钟 TTL
4. **20-block 回溯窗口溢出**：✗ 实测距离仅 4 blocks（3524 msg[19] block 37 → 3525 msg[21] block 40），远小于 20
5. **Gateway 代码逻辑错误**：✗ 正常区间证明当时的请求构造能够稳定命中

### 9.3 确认的根本原因：Anthropic API 分片不一致

关键证据：

1. **3524 写入的缓存确实存在**：3526（5 秒后）命中了 129831 tokens，正好是 3524 写入的量
2. **3525 在查询时未找到**：read=0，但前缀完全一致
3. **3526 又找到了**：说明缓存本身正常，只是 3525 那次查询没找到

**最可能的技术原因**：Anthropic API 后端的多服务器 / 缓存分片架构导致：

- **异步写入延迟**：3524 的 HTTP 响应返回时，缓存可能还在后台异步写入分片
- **跨分片同步延迟**：缓存写入分片 A，但 3525 请求被路由到分片 B，此时分片间尚未同步（典型延迟 5-60 秒）
- **负载均衡路由变化**：连续请求可能基于客户端 IP、时间窗口、服务器负载等因素被路由到不同节点

**为什么正常区间（3542-3548）没问题**：
- 请求间隔更短且连续（5-10 秒），可能被路由到同一服务器
- 缓存留在本地 L1 层，未经过跨分片同步
- 或者该时段负载低，分片同步速度快

### 9.4 影响评估

- **发生频率**：10 轮中 2 次 miss（20%），但连续稳定区间可达几十轮无 miss
- **恢复速度**：miss 后下一轮（5-10 秒）立即恢复正常
- **成本影响**：单次 miss 多付 ~14 万 tokens 的 write 费用（约 $0.009 for Opus-4.8），但下轮立即恢复 92%+ 命中率
- **延迟影响**：miss 轮 TTFT 会增加，但 Anthropic 的 cache write 本身是流式并行的，影响有限

### 9.5 应对策略

**当前建议：接受现状**
- 这是 Anthropic API 基础设施的最终一致性延迟，客户端无法控制
- 偶发 miss 不影响功能正确性，下一轮自动恢复
- 成本影响可控（20% miss 率时，整体缓存命中率仍在 75%+）

**可选优化方向**（视需求决定是否实现）：
1. **检测与重试**：在响应解析时检测 `cache_read_input_tokens=0` 且前缀理论应命中的情况，记录日志或触发 1-2 秒后的自动重试
2. **监控告警**：统计 miss 率，当连续 N 次或一定时间窗口内 miss 率超过阈值时告警
3. **向 Anthropic 反馈**：提供复现数据（请求 ID、时间戳），推动官方优化缓存分片同步机制

**不建议的方案**：
- ✗ 客户端侧缓存预热：无法解决服务端分片不一致问题
- ✗ 仅靠增加消息断点：历史双滚动样本同样出现过 miss，不能解决服务端分片同步延迟
- ✗ 增加断点数量：受 4 个上限限制，且无助于解决分片同步延迟

### 9.6 结论

历史样本说明，偶发缓存 miss 并非单纯增加消息断点就能消除，更可能来自 Anthropic API 服务端的分布式缓存一致性。当前 Gateway 使用 tools、system、messages 三个分段断点：tools 与 system 提供稳定前缀锚点，messages 使用一个滚动尾部断点；后续命中率应以新策略下的生产日志继续验证。
