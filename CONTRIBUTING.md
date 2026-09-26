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

## 构建产物清理

`target/` 是生成物，不提交。磁盘紧张时按以下入口清理：

- 彻底清理（下次构建全量重编）：

  ```bash
  cargo clean
  ```

- 定向清理陈旧测试二进制（免全量重链接）：

  ```bash
  find target/debug/deps -maxdepth 1 -type f -name '*_tests*' ! -newer target/debug/test262-runner -delete
  ```

  以 runner 二进制 mtime 为界，只删早于最近一次构建的测试二进制；删除须先于任何构建动作，否则 runner 刷新后陈旧面会扩大。
