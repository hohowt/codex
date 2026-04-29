# 应用（连接器）工具发现

使用 BM25 搜索应用/连接器的工具元数据，并将匹配的工具暴露给下一次模型调用。

你可以使用以下应用/连接器的所有工具：
{{app_descriptions}}
部分工具可能未提前提供给你，你应该使用此工具（`tool_search`）搜索所需的工具并为上述应用加载它们。对于上述应用，始终使用 `tool_search` 而非 `list_mcp_resources` 或 `list_mcp_resource_templates` 进行工具发现。
