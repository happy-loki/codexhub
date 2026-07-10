CodexHub v0.3.33

小幅收窄 Responses 工具调用兼容逻辑：

- 仅对 `custom_tool_call name=exec namespace=exec` 做兼容归一化，避免影响其他自定义工具命名空间。
