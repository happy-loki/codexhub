# IM Remote-Control Stream 隔离方案

更新时间：2026-06-09

## 背景

Codex remote-control 官方实现支持多个独立逻辑 client。隔离键是：

```text
(client_id, stream_id)
```

`client_id` 可以相同，只要 `stream_id` 不同，Codex app-server 侧就会创建不同的 remote-control connection。官方源码里 `ClientTracker` 使用 `HashMap<(ClientId, StreamId), ClientState>`，并有测试覆盖“同一个 client_id 使用新 stream_id 会打开新 connection”。

当前 `codex-remote` 已有 `RemoteControlInner.clients: HashMap<String, RemoteControlClientState>`，非 default `client_key` 会派生独立 `stream_id`。问题是 IM 层很多入口仍然走 `default_remote_client_key()`，导致飞书、微信、Telegram 会共享同一条 remote-control stream。一条链路的 pending reset、reinitialize、recovery 可能影响另一条 IM 链路。

## 目标

1. 每个 IM conversation 绑定一个确定的 remote-control client key。
2. 飞书、微信、Telegram 之间不共享 default stream。
3. 同一 IM conversation 断链恢复后继续使用同一个 remote-control client key。
4. thread/turn/approval/recovery/list/config 相关请求都使用同一个 route key。
5. 不使用 current thread 或 default client 作为业务 fallback。

## 非目标

1. 不改变 Codex 官方 remote-control wire protocol。
2. 不给一个 IM conversation 同时广播到多个 Codex app-server。
3. 不在本阶段实现用户手动选择 remote-control 执行端。
4. 不把 IM conversation 直接等同于 Codex thread；两者仍通过 thread binding 绑定。

## Key 规则

每个 IM route 基于 platform、account_id、chat_id 生成稳定 key：

```text
im:<platform>:<sha256(platform:account_id:chat_id)[0..16]>
```

示例：

```text
im:feishu:2f8c...
im:wechat:ab31...
im:telegram:91d0...
```

使用 hash 而不是原始 chat_id，避免日志里泄露过多 IM 标识，同时保持确定性。

## 请求路径

创建 thread：

```text
InboundMessage
  -> RouteTarget
  -> route.remote_client_key
  -> thread/start on route key
  -> bind thread_id -> RouteTarget(remote_client_key)
```

恢复 thread：

```text
RouteTarget
  -> route.remote_client_key
  -> thread/resume on route key
  -> bind thread_id -> same RouteTarget(remote_client_key)
```

发送 turn：

```text
InboundMessage
  -> find bound RouteTarget by conversation_key
  -> bound route.remote_client_key
  -> turn/start on route key
```

approval response：

```text
PendingApproval.remote_client_key
  -> send_response_for_client(route key)
```

恢复订阅：

```text
remote-control recovery(client_key)
  -> only resubscribe route.remote_client_key == client_key
```

## 约束

IM route 创建时必须带上确定性 `remote_client_key`。后续 thread/turn/approval/recovery/list/config 请求只使用这个 key；缺失 key 视为状态错误，不回退到 default/current thread。

`default_remote_client_key()` 只保留给非 IM 或状态展示场景，不作为 IM thread/turn 的正常路径。

## 实施步骤

1. 在 `RouteTarget` 上实现确定性 remote-client-key helper。
2. `route_for_message()` 和 `route_from_conversation_key()` 创建 route 时直接携带 key。
3. `create_and_bind_thread()`、`resume_and_bind_thread()` 改用 route key。
4. `start_turn_for_route()`、approval response 改用 route key。
5. thread list、thread create defaults、form options 改用 route key。
6. 增加测试验证不同 IM conversation 生成不同 key。
