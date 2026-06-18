<div align="center">
  <img src="docs/assets/oxidejs-logo.svg" width="250" alt="OxideJS Logo" />

  <h1>OxideJS：面向 AI Agent 的轻量级 JavaScript 执行引擎</h1>

  <p>
    <img alt="Rust" src="https://img.shields.io/badge/Rust-1.80%2B-orange?style=for-the-badge&logo=rust" />
    <img alt="Platform" src="https://img.shields.io/badge/Platform-Linux%20%7C%20macOS%20%7C%20Windows-blue?style=for-the-badge" />
    <img alt="Engine" src="https://img.shields.io/badge/JS%20Engine-Non--Wrapper-success?style=for-the-badge" />
    <img alt="test262" src="https://img.shields.io/badge/test262-Integrated-purple?style=for-the-badge" />
  </p>
</div>

## 1. 项目简介

OxideJS 是一个使用 Rust 编写的轻量级 JavaScript 执行引擎，面向短时、高频、即时执行的脚本运行场景，例如 Agent 工具调用、脚本沙箱、数据转换流水线和嵌入式运行时。

OxideJS 不是 V8、QuickJS、JavaScriptCore 或其他现有引擎的封装。项目实现了自己的编译流水线、字节码格式、虚拟机、值表示、对象模型和运行时核心。

当前项目聚焦于实用 ECMAScript 子集，并通过 [test262](https://github.com/tc39/test262) 持续验证兼容性。长期目标是提供一个小型、可检查、跨平台、启动成本可预测、benchmark 可复现的 JavaScript runtime。

## 2. 特性概览

- **Rust 实现**：使用 Cargo workspace 组织 parser、compiler、VM、runtime、CLI 和 test runner。
- **自研引擎**：自研字节码、寄存器式虚拟机、对象模型和运行时状态管理。
- **寄存器式 VM**：使用固定寄存器文件执行字节码，减少栈式 VM 中频繁 push/pop 的开销。
- **NaN-boxing 值表示**：用 64-bit 值统一表示 number、boolean、object、string、null 和 undefined。
- **Shape 对象布局**：使用隐藏类思想描述对象属性布局，便于缓存属性偏移。
- **Inline Cache 方向**：围绕 shape/offset 缓存设计属性访问路径。
- **共享运行时 Kernel**：统一管理字符串驻留、Shape、编译缓存、属性模板和内置对象。
- **test262 runner**：内置兼容性测试运行器，输出 pass / fail / skip 统计。

## 3. 架构

OxideJS 采用经典的 parse -> compile -> execute 流水线，同时使用 **单 Kernel、多 VM** 的运行时结构：一个 `OxideKernel` 保存可共享的字符串、Shape、编译缓存、属性模板和内置对象；多个 `oxide_vm` 实例面向不同执行请求独立运行，并共同引用同一个 Kernel。

```text
                         +----------------------+
                         |     oxide_kernel     |
                         |----------------------|
                         |  StringForge         |
                         |  ShapeForge          |
                         |  CodeForge           |
                         |  PropForge           |
                         |  BuiltinWorld        |
                         +----------+-----------+
                                    |
          shared Arc<OxideKernel>  |  shared Arc<OxideKernel>
                 +-----------------+-----------------+
                 |                 |                 |
                 v                 v                 v
        +----------------+ +----------------+ +----------------+
        |    oxide_vm    | |    oxide_vm    | |    oxide_vm    |
        |----------------| |----------------| |----------------|
        |  registers     | |  registers     | |  registers     |
        |  call frames   | |  call frames   | |  call frames   |
        |  local epoch   | |  local epoch   | |  local epoch   |
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
| oxide_compiler   |  AST 编译为寄存器式字节码；可命中 Kernel.CodeForge
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
3. **寄存器式 VM**：`oxide_vm` 读取字节码并使用固定寄存器文件执行；每个 VM 保持自己的寄存器、调用栈和本地 epoch。
4. **值系统**：`JsValue` 负责紧凑表示 JavaScript 运行时值。
5. **对象模型**：对象通过 Shape ID 描述属性布局。
6. **运行时 Kernel**：`OxideKernel` 保存可跨 VM 复用的共享状态；多个 VM 通过 `Arc<OxideKernel>` 引用同一个 Kernel，避免重复初始化字符串池、Shape 表、内置对象和编译缓存。
7. **内置对象层**：常用内置对象和方法由 Rust 原生实现。
8. **兼容性测试层**：`oxide_test262` 运行 test262 用例并输出统计结果。

## 4. 仓库结构

```text
project-root/
├── Cargo.toml
├── README.md
├── crates/
│   ├── oxide_parser/      # JavaScript parser 接入层
│   ├── oxide_compiler/    # AST -> bytecode 编译器
│   ├── oxide_types/       # JsValue、JsObject、Shape、内存基础类型
│   ├── oxide_kernel/      # 共享运行时状态和内置对象注册
│   ├── oxide_vm/          # 字节码 VM 和运行时执行逻辑
│   ├── oxide_api/         # 嵌入式 API 预留层
│   ├── oxide_cli/         # 命令行工具
│   └── oxide_test262/     # test262 兼容性测试运行器
└── tests/
    └── test262/           # 本地 test262 测试套件
```

## 5. 构建

### 构建全部 crate

```bash
cargo build --release
```

### 运行单元测试

```bash
cargo test
```

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

OxideJS 包含独立的 test262 runner：

```bash
cargo run --release -p oxide_test262
```

运行指定子目录：

```bash
cargo run --release -p oxide_test262 -- tests/test262/test language/expressions
```

runner 会输出：

- 发现的测试文件总数；
- pass / fail / skip 数量；
- 全量通过率；
- 实际执行样本通过率；
- 失败类别统计；
- 按目录拆分的结果（取决于 runner 版本）。

兼容性数字属于开发过程指标。发布正式 benchmark 或兼容性结论前，应基于当前 checkout 的 test262 版本重新生成结果。

## 8. Benchmark

Benchmark 工作围绕可复现脚本和可比较 baseline 展开。

计划中的 benchmark 分组：

| 分组 | 目的 | 状态 |
|------|------|------|
| 表达式 microbenchmark | 算术、比较、逻辑操作 | planned |
| 对象 / 属性访问 benchmark | 对象创建和属性访问路径 | planned |
| Array / String benchmark | 常用内置对象操作 | planned |
| 函数调用 benchmark | 字节码 call/return 与 native call 开销 | planned |
| Agent 风格 workload | 短时、重复结构脚本和数据转换 | planned |
| JetStream 2.0 子集 | 依赖兼容性的 JS benchmark 覆盖 | planned |

当前研究用 baseline：

- QuickJS
- Boa
- JerryScript

[TODO] 完工时放与其他 baseline 的对比表格

## 9. License

本项目采用 MIT License 开源协议。协议全文见项目根目录的 [LICENSE](LICENSE) 文件。
