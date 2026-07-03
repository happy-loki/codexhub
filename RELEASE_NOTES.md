# CodexHub v0.3.19

## 改进内容

- Anthropic / GLM 主请求现在也注入 `metadata.user_id`，与 Claude Code 对齐。此前只有内部 web-search 辅助请求带该字段，主对话请求完全没有 metadata。
  - 值沿用会话标识（`session_id`，缺失时回退 `prompt_cache_key`），同一会话每轮保持不变，形如 Claude Code 的 `{"device_id":...,"account_uuid":"","session_id":...}`。
  - Anthropic 用这个不透明标识做滥用检测，实践中也作为稳定性提示，帮助同一会话的连续请求落到缓存温热的容量上，减少「前缀一致却偶发 cache_read=0」的抖动。
  - 调用方若已自带 `metadata.user_id` 则保留不覆盖。

## 验证

- `cargo fmt`
- `cargo test`（356 项通过，新增主请求 metadata 注入与保留既有 user_id 两项断言）

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
