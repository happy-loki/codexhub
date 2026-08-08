# Telegram 集成计划

本文档定义 `codexhub` 接入 Telegram 的落地边界。目标是先支持一个可用的 Telegram Bot 通道，而不是把飞书和 Telegram 抽象成同一种 UI。

## 目标

- 支持 Telegram Bot 作为新的 IM 通道。
- Telegram MVP 默认面向 BotFather 创建的私人 bot，由用户直接私聊这个 bot。
- 复用 Codex remote-control、thread 绑定、approval 队列、turn 状态、消息去重等平台无关逻辑。
- 保留飞书现有体验，不因为 Telegram 引入大规模重写。
- Telegram MVP 优先支持纯文本、默认新建 thread、审批确认和中断/退出命令。

## 非目标

- 不做跨平台卡片 DSL。
- 不尝试复用飞书 CardKit/交互卡片 renderer。
- 不在 MVP 阶段追平飞书的完整复杂卡片和流式卡片体验。
- 不要求 Telegram 支持飞书专有的用户授权、审批卡片样式或多维交互布局。

## 架构原则

抽象业务意图，不抽象 UI 结构。

```text
Codex notification / inbound message
        |
platform-neutral bridge state
        |
   Feishu adapter          Telegram adapter
   Feishu renderer         Telegram renderer
```

平台无关层应该只理解这些概念：

- conversation：一个 IM 平台上的聊天目标。
- thread route：conversation 与 Codex thread 的绑定关系。
- inbound message：用户文本、附件和平台回调动作。
- approval：Codex 请求用户确认的业务事件。
- turn origin：某个 turn 来自哪个 IM 平台，用于避免把用户自己的输入再次回显。

平台层负责这些事情：

- 监听平台事件。
- 把平台事件转成 `InboundMessage` / `InboundAction`。
- 把业务意图渲染成平台自己的消息。
- 处理平台消息更新、按钮回调、文件上传和格式转义。

## 技术决策

### Bot API 封装

Telegram 使用 BotFather 创建的 Bot API，不使用 MTProto/userbot。

当前阶段继续使用 `reqwest` 自己封装少量 Bot API，不引入 `teloxide`：

- 现有链路已经是 `bridge -> InboundMessage -> Codex turn -> Adapter`，`teloxide` 的 dispatcher/dialogue 模型会与当前架构重复。
- 我们需要控制的是协议细节：`getUpdates` 409 冲突、`retry_after`、4096 字符切块、typing、callback approval。
- 当前 API 面很小：`getUpdates`、`sendMessage`、`getMe`、`sendChatAction`，后续再加 `answerCallbackQuery`、`editMessageText`、`deleteMessage`、`sendDocument`。

### Channel 抽象

不照搬 ZeroClaw 的完整 `Channel` trait。ZeroClaw 的接口覆盖草稿流式、附件、reaction、pin/delete、choice、approval 等大量能力，适合完整多 IM 框架，但对当前项目过重。

本项目后续如果继续接入更多 IM，可以抽一个轻量 `ImChannel`，只表达业务意图：

- `listen`
- `send_text`
- `send_approval`
- `send_status`
- `typing` / `health` 可选

飞书卡片、Telegram inline keyboard、消息编辑、文件发送等平台表现仍留在各自 adapter 内。

## ZeroClaw 参考结论

参考目录：`references/zeroclaw-master`。

值得借鉴的 Telegram 协议处理：

- `getUpdates` 启动前先 `timeout=0` probe，避免旧 long-poll 连接残留后持续 409。
- 主 polling 遇到 409 backoff 到大于 long-poll timeout 的时间。
- `allowed_updates` 限定为 `message` / `callback_query`。
- `getMe` 获取 bot 展示名用于 GUI；当前阶段不接入群聊。
- 文本按 Telegram 4096 字符限制切块，优先按换行/空格断开，并给多段消息加 continuation 标记。
- 多段消息之间加短延迟，降低触发限流的概率。
- 收到用户消息后发 `sendChatAction=typing`，降低“没响应”的体感。
- 审批后续可用 inline keyboard + callback_query，收到按钮回调后调用 `answerCallbackQuery`。
- 流式体验后续可用 `sendMessage` + `editMessageText`，最终失败时谨慎 delete+send，避免重复消息。

不直接搬运的部分：

- 完整 `Channel` trait。
- 语音、TTS、附件转写、复杂 bot command 注册。
- 多平台通用 renderer 或 UI DSL。

## 当前代码耦合点

历史耦合点如下，当前已经处理了平台身份和 Telegram MVP 的主要链路：

- `InboundMessage::conversation_key()` 已改为 `{platform}:{account_id}:{chat_id}`。
- `TurnOrigin` 已支持 `Feishu` / `Telegram`。
- `route_from_conversation_key()` 已支持 `feishu` / `telegram`。
- `PendingApproval` 已使用平台中性的 `message_id`。
- `config.rs` 已新增 `[telegram]`。
- `bridge.rs` 仍保留较多业务编排逻辑，后续再考虑小型 `ImChannel` 抽象。
- GUI 已在“聊天工具接入”页管理 Telegram Bot Token，并明确 Telegram 仅支持私聊。

这些点需要分阶段处理，避免一次性重构影响飞书稳定性。

## 第一阶段：平台身份和路由抽象

状态：已完成。

目标：不改变飞书行为，只把平台身份从写死字符串中拆出来。

- 增加 `ImPlatformKind`，初始支持 `Feishu`，预留 `Telegram`。
- `InboundMessage` 增加 `platform` 字段，并保持 serde 默认值为 `Feishu`，兼容旧测试/旧输入。
- `conversation_key()` 改为 `{platform}:{account_id}:{chat_id}`。
- `RouteTarget` 增加 `platform` 字段。
- `route_from_conversation_key()` 支持 `feishu` 和 `telegram`。
- `TurnOrigin` 增加 `Telegram`。
- 将 `PendingApproval.feishu_message_id` 改为平台中性的 `message_id`。

这一阶段不引入 Telegram Bot，也不改飞书 renderer。

## 第二阶段：拆出平台发送边界

状态：已完成飞书 adapter 和 Telegram adapter 的基础边界。

目标：把 `bridge.rs` 中的核心业务动作和飞书发送动作隔开。

不做 UI DSL，只定义业务动作接口，例如：

- `send_text`
- `send_thread_routing_choice`
- `send_thread_routing_list`
- `send_approval`
- `send_next_approval`
- `send_turn_completed`
- `send_item_update`

飞书实现继续调用现有 renderer。Telegram 未来实现自己的 renderer。

这一阶段可以先只把接口包在飞书 adapter 上，行为保持不变。

## 第三阶段：Telegram MVP

状态：基础链路已完成。

配置新增：

```toml
[telegram]
botToken = ""
allowedChatIds = []
```

MVP 功能：

- 通过 long polling 接收 Telegram Bot 消息。
- 私聊文本消息转为 `InboundMessage`；群聊消息和群聊按钮回调直接忽略。
- 全局命令只保留 `/s` 中断当前任务、`/q` 退出当前会话。
- 没有绑定 thread 时，先让用户选择新建会话或恢复历史会话，不自动创建。
- approval 先用文本命令回复，后续再补 inline keyboard。
- Codex 最终回复用普通文本发送。

MVP 之后已补齐：

- 图片、文件、音视频等附件入站下载；无说明的纯图片会暂存并等待下一条文字描述。
- 菜单、分页、创建进度和审批结果优先原位更新，旧按钮随状态清理。
- 同一 turn 的多个 `Ran` 命令步骤合并到一条进度消息，步骤完成后原位更新；完成或中断时收口，过长历史只保留关键步骤和最近步骤。
- 同一 turn 的子代理活动合并到另一条协作进度消息；只显示名称、状态、耗时和简短结果，`wait` 轮询、内部 thread/call id、空数组及原始 JSON 不发送到聊天。
- Agent 回复通过 Bot API 原生消息草稿流式展示，最终仍发送普通消息固化结果。

仍暂不做：

- 飞书 CardKit 等价的复杂富卡片。
- Telegram webhook 部署。

## 第四阶段：Telegram 协议层加固

状态：基础加固已完成。

目标：不引入新框架，把 Telegram Bot API 的易错细节收口到 `src/im/telegram` 内。

本阶段任务：

- `TelegramApi` 复用 `reqwest::Client`。已完成。
- `TelegramApi` 暴露结构化错误，保留 `error_code`、`description`、`retry_after`。已完成。
- `polling` 启动时执行 `timeout=0` probe，成功后再进入 long polling。已完成。
- `polling` 遇到 409 conflict 时 backoff，避免旧连接未释放时高频重试。已完成。
- `polling` 只接受 private chat，群聊消息和群聊 callback_query 不进入 Codex。已完成。
- 收到可处理消息后发送 `sendChatAction=typing`。已完成。
- `TelegramAdapter` 按 4096 字符限制智能切块，优先在换行/空格处分段。已完成。
- 多段消息加 continuation 标记，并在段之间短暂 sleep。已完成。

## 第五阶段：Telegram Thread 管理

状态：基础能力已完成，会话创建/恢复已改为未绑定时自动触发的按钮式流程。

Telegram 不适合照搬飞书表单。当前表达方式：

- 没有绑定 thread 时，自动发送 inline keyboard 主菜单，用户选择“创建新会话”或“恢复历史会话”。
- 恢复历史会话：发送历史 thread 文本列表，每个 thread 使用 `/1`、`/2` 这类短命令选择，底部只提供“上一页 / 下一页 / 创建新会话”等导航按钮。
- callback data 不直接塞长 thread id，而是使用 `request_id + page + index`，实际 thread id 存在 runtime 的 `ThreadRoutingRequestState` 里，避免 Telegram 64 字节 callback data 限制。
- 创建新会话：发送创建设置面板，用户通过按钮选择目录、模型、推理强度和权限，最后点“创建”。
- 目录/模型/推理强度/权限选择都使用 inline keyboard；callback data 只带 `request_id + field + page + index`，实际选项值存在 runtime，避免 Telegram 64 字节 callback data 限制。
- 目录支持“自定义或新建目录”：用户点按钮后直接发送绝对路径，不需要写 `cwd=` 参数。
- 不支持斜杠命令式新建参数入口，避免普通用户需要理解内部字段名。

后续可优化：

- 对加载列表增加搜索/filter。
- 聚合 Telegram media group 后再一次性提交整组附件。

## 第六阶段：GUI 和体验补齐

- GUI 已在“聊天工具接入”页增加 Telegram 接入区域。
- 状态概览已合并为 IM 通道状态；Telegram 已增加 polling 运行态，按真实连接状态展示“已接入 / 连接中 / 等待连接”。
- 配置页支持 token 保存，并通过全局 IM bridge 启停。
- About/README 已更新 Telegram 使用说明。

## 风险

- Telegram Bot API callback data 有长度限制，复杂 action 需要短 id 映射到本地 runtime。
- Telegram 消息编辑有频率和内容限制，流式输出需要节流。
- Telegram Markdown/HTML 转义容易出错，MVP 应先使用纯文本或最少格式。
- 多平台同时启用时，`last_route` 这类全局状态需要继续审查，避免平台互相覆盖。
- Telegram Bot API 同一个 token 只能有一个 active long polling 消费者，用户开多个进程时会出现 409。

## 推荐实施顺序

1. 完成第一阶段平台身份和路由抽象。已完成。
2. 保持飞书测试通过。已完成。
3. 拆出飞书 adapter，但不改渲染结果。已完成。
4. 新增 Telegram long polling + 纯文本发送。已完成。
5. 加固 Telegram 协议层：polling probe、409 backoff、getMe、typing、智能切块。已完成。
6. Telegram thread 管理：新建、参数创建、历史加载、分页。已完成基础能力。
7. 补 approval inline keyboard 与原消息状态更新。已完成。
8. 补原生流式草稿和文件/图片入站。已完成。
