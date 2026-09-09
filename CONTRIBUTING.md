# 贡献指南

## 提交信息风格

所有 commit 消息遵循统一格式：

```
type: 中文描述
```

- type 限定：`feat` / `fix` / `perf` / `refactor` / `test` / `chore` / `docs` / `style` / `revert`
- 不带 scope
- 描述写"改了什么、为什么"，不写实现步骤
- 不引用内部编号、任务号或平台特定信息
- 合并提交使用 git 默认格式（`Merge ...`）

示例：

```
feat: Temporal.PlainTime round 舍入（真因子校验 + 跨午夜取模）
fix: 闭包链式捕获越界改编译错误早暴露防静默错值
perf: 字符串拼接消除冗余拷贝
```
