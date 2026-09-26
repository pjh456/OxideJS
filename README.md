<div align="center">
  <img src="docs/assets/oxidejs-logo.svg" width="250" alt="OxideJS Logo" />

  <h1>OxideJS：面向 AI Agent 的轻量级 JavaScript 执行引擎</h1>

  <p>
    <img alt="Rust" src="https://img.shields.io/badge/Rust-1.80%2B-orange?style=for-the-badge&logo=rust" />
    <img alt="Platform" src="https://img.shields.io/badge/Platform-Linux%20%7C%20macOS%20%7C%20Windows%20And%20More-blue?style=for-the-badge" />
    <img alt="Engine" src="https://img.shields.io/badge/JS%20Engine-Non--Wrapper-success?style=for-the-badge" />
    <img alt="test262" src="https://img.shields.io/badge/test262-83.34%25-purple?style=for-the-badge" />
  </p>
</div>

## 1. 项目简介

OxideJS 是一个基于 Rust 的轻量级 JavaScript 执行引擎，擅长短时、高频、即时执行的脚本运行场景，主要面向 Agent 工具调用、脚本沙箱、数据转换流水线和嵌入式运行时。

当前项目聚焦于实用 ECMAScript 子集，并通过 [test262](https://github.com/tc39/test262) 持续验证兼容性。我们的最终目标是提供一个小型、可检查、跨平台、启动成本可预测、benchmark 可复现的 JavaScript runtime。

演示视频链接: https://pan.baidu.com/s/11gvBV5G_rLrNTS0sb863Qg?pwd=pr7n 提取码: pr7n

## 2. 特性概览

- **Rust 实现**：使用 Cargo workspace 组织 parser、compiler、VM、runtime、CLI 和 test runner。
- **自研引擎**：自研字节码、寄存器式虚拟机、对象模型和运行时状态管理。
- **寄存器式 VM**：使用固定寄存器文件执行字节码，减少栈式 VM 中频繁 push/pop 的开销。
- **NaN-boxing 值表示**：用 64-bit 值统一表示 number、boolean、object、string、null 和 undefined。
- **Shape 对象布局**：使用隐藏类思想描述对象属性布局，便于缓存属性偏移。
- **Inline Cache 方向**：围绕 shape/offset 缓存设计属性访问路径。
- **共享运行时 Kernel**：`KernelCore` 永久共享 key 驻留 / Shape / 编译缓存 / IC 模板, `KernelSession` 按会话管理 BuiltinWorld 与 global object。
- **多 pass IR 编译管线**：AST -> IR -> CFG/liveness/regalloc/DCE -> 32-bit bytecode，各分析 pass 拆分为独立 crate。
- **内置对象扩展**：完整 Temporal 族、DisposableStack/AsyncDisposableStack/SuppressedError、Iterator.prototype helpers、eval 支持。
- **test262 runner**：内置兼容性测试运行器，输出 pass / fail / skip 统计，支持监督式窗口运行与 FAIL 清单。

## 3. 架构

OxideJS 采用经典的 parse -> compile -> execute 流水线，同时使用 **单 Kernel、多 VM** 的运行时结构。Kernel 内部分为两层：`KernelCore` 永久共享 (PermInterner / Shape / Code / Prop)，`KernelSession` 按会话可重建 (BuiltinWorld / global object)；多个 `oxide_vm` 实例面向不同执行请求独立运行。

```text
                         +-----------------------+
                         |     oxide_kernel      |
                         |-----------------------|
                         | KernelCore            |
                         |  PermInterner         |  append-only key 驻留
                         |  ShapeForge           |  Shape / Hidden Class 表
                         |  CodeForge            |  bytecode LRU 缓存
                         |  PropForge            |  IC 模板
                         |-----------------------|
                         | KernelSession         |  可 full_reset() 重建
                         |  BuiltinWorld         |
                         |  Global object        |
                         |  BuiltinSnapshot      |
                         +----------+------------+
                                    |
          shared Arc<KernelCore>  |  + per-VM KernelSession
                 +-----------------+-----------------+
                 |                 |                 |
                 v                 v                 v
        +----------------+ +----------------+ +----------------+
        |    oxide_vm    | |    oxide_vm    | |    oxide_vm    |
        |----------------| |----------------| |----------------|
        |  registers     | |  registers     | |  registers     |
        |  call frames   | |  call frames   | |  call frames   |
        |  Epoch Arena   | |  Epoch Arena   | |  Epoch Arena   |
        |  JsString GC   | |  JsString GC   | |  JsString GC   |
        +-------+--------+ +-------+--------+ +-------+--------+
                |                  |                  |
                v                  v                  v
        JsValue Result     JsValue Result     JsValue Result

Per request pipeline:

 JavaScript Source
        |
        v
+------------------+
|   oxide_parser   |  解析源码，生成 AST
+------------------+
        |
        v
+------------------+
| oxide_compiler   |  AST -> IR -> 多 pass（CFG/liveness/regalloc/DCE）-> 字节码；可命中 KernelCore.CodeForge
+------------------+
        |
        v
+------------------+
|    oxide_vm      |  从 VM Pool 获取实例并执行字节码
+------------------+
```

核心组件：

1. **Parser 前端**：通过 `oxide_parser` 将源码解析为 AST。
2. **字节码编译器**：将 AST 降低为 OxideJS 字节码、常量池和寄存器布局。
3. **寄存器式 VM**：`oxide_vm` 读取字节码并使用固定寄存器文件执行；每个 VM 持有自己的寄存器、调用栈、Epoch Arena 与 JsString GC。
4. **值系统**：`JsValue` 通过 NaN-boxing 紧凑表示 JavaScript 运行时值；`JsString` 由 VM 内的标记清扫 GC 管理。
5. **对象模型**：对象通过 Shape ID 描述属性布局。
6. **运行时 Kernel**：`KernelCore` 保存进程级永久共享状态 (`PermInterner` 等), `KernelSession` 保存可重建的会话级状态 (`BuiltinWorld` / global object); 多个 VM 通过 `Arc<KernelCore>` 引用同一份永久共享数据。
7. **内置对象层**：常用内置对象和方法由 Rust 原生实现，通过 `BuiltinWorld` 注册。
8. **兼容性测试层**：`oxide_test262` 运行 test262 用例并输出统计结果，支持 `--supervise` 子进程窗口模式实现单测超时与断点续跑。

## 4. 仓库结构

```text
project-root/
├── Cargo.toml
├── README.md
├── crates/
│   ├── oxide_parser/      # JavaScript parser 接入层
│   ├── oxide_emit/        # AST -> IR 代码生成层
│   ├── oxide_ir/          # IR 中间表示与 bytecode 降级
│   ├── oxide_cfg/         # CFG 分析 pass
│   ├── oxide_liveness/    # liveness 分析 pass
│   ├── oxide_dce/         # 死代码消除 pass
│   ├── oxide_regalloc/    # 寄存器分配 pass
│   ├── oxide_bytecode/    # 共享字节码协议与编译产物 ABI
│   ├── oxide_code_cache/  # 编译结果缓存 (CodeForge)
│   ├── oxide_compiler/    # 编译入口，编排各 IR pass
│   ├── oxide_types/       # JsValue、JsObject、Shape、内存基础类型
│   ├── oxide_kernel/      # 共享运行时状态
│   ├── oxide_runtime_api/ # builtins 与 VM 的抽象接口 (VmHost)
│   ├── oxide_builtins/    # 内置对象实现
│   ├── oxide_vm/          # 字节码 VM 和运行时执行逻辑
│   ├── oxide_log/         # 基于 tracing 的统一日志
│   ├── oxide_cli/         # 命令行工具
│   └── oxide_test262/     # test262 兼容性测试运行器
└── tests/
    └── test262/           # 本地 test262 测试套件
```

## 5. 构建与日常验证

### 构建全部 crate

```bash
cargo build --workspace
```

### 按 crate 运行单元测试

```bash
cargo test -p <crate>
```

### 格式与 lint 自查

```bash
cargo fmt --all -- --check
cargo clippy --all-targets
```

验收口径：构建通过、单测通过、零新增 clippy 告警。test262 子集回测与全量回测命令见第 7 节。

## 6. CLI 使用

### 执行源码片段

```bash
cargo run --release -p oxide_cli -- eval "1 + 2"
```

### 执行文件

```bash
cargo run --release -p oxide_cli -- run examples/demo.js
```

### 打印编译后的字节码

```bash
cargo run --release -p oxide_cli -- compile -e "1 + 2"
```

### 启动 REPL

```bash
cargo run --release -p oxide_cli
```

## 7. test262 兼容性测试

OxideJS 包含独立的 test262 runner，套件位于 `tests/test262`（git 子模块）。

### 运行子集（日常）

```bash
# 按目录筛选
cargo run -p oxide_test262 -- tests/test262/test/language/expressions/addition

# 按路径关键词筛选
cargo run -p oxide_test262 -- tests/test262/test "literals/string"
```

### 全量运行（监督式）

```bash
OXIDE_TEST262_SUPERVISORS=16 OXIDE_TEST262_WINDOW=3300 \
  cargo run -p oxide_test262 -- --supervise tests/test262/test
```

- `--supervise` 为单 worker 子进程窗口模式，带单测超时与断点续跑；并发窗口数缺省 16（`OXIDE_TEST262_SUPERVISORS`）；
- `--supervise` 可与位置参数路径筛选组合（筛选在子进程窗口内逐测试生效）；
- 含 `built-ins/` 的回测必须加 `--supervise`：非监督模式无单测超时，单个测试可无界增长拖死宿主。

runner 会输出：

- 发现的测试文件总数；
- pass / fail / skip 数量；
- 全量通过率；
- 实际执行样本通过率；
- 失败类别统计与 FAIL 清单（按测试文件路径）；
- 按目录拆分的结果（取决于 runner 版本）。

兼容性数字属于开发过程指标。发布正式 benchmark 或兼容性结论前，应基于当前 checkout 的 test262 版本重新生成结果。

## 8. 主要使用的开源项目

- [oxc](https://github.com/oxc-project/oxc) — JavaScript 源码解析，因为这不是我们的工作中心，所以没有自己构建该系统
- [bumpalo](https://github.com/fitzgen/bumpalo) — Bump allocator，构成 `Epoch` arena 内存系统的底层分配器
- [dashmap](https://github.com/xacrimon/dashmap) — 并发 HashMap，用于 `CodeForge`、`ShapeForge`、`PropForge` 跨 VM 缓存共享

## 9. License

本项目采用 MIT License 开源协议。详见 [LICENSE](LICENSE)。
