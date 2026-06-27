# OxideJS Benchmark Suite

自动编译 + 全量基准测试 + AI Agent 适配度评分（QuickJS = 80 分基准）。

## 目录结构

```
benchmark/
├── README.md
├── build.sh              # 一键构建 (Rust + QuickJS + 依赖安装)
├── run_benchmark.py      # 全量测试脚本
└── fetch_test262.sh      # 下载 test262 测试套件
```

脚本自动检测仓库内资源：
- `../` — OxideJS Rust 源码
- `../baseline-quickjs/` — QuickJS 源码（放这里自动编译）
- `../tests/stress/` — JS 压测脚本
- `../tests/test262/test/` — test262 套件（缺则运行 fetch_test262.sh）

## 快速开始

```bash
# 1. 确保 QuickJS 源码在 baseline-quickjs/
git submodule add https://github.com/bellard/quickjs.git baseline-quickjs
# 或手动放入

# 2. 确保 stress 测试在 tests/stress/
# (仓库已自带)

# 3. 构建
bash benchmark/build.sh

# 4. 下载 test262 (首次，约 500MB)
bash benchmark/fetch_test262.sh

# 5. 运行全量测试
python3 benchmark/run_benchmark.py
```

## 输出

```
benchmark/results/
├── report.md    # Markdown 报告
└── data.json    # 原始数据
```

## 评分体系

| 维度 | 满分 | QuickJS基准 | 逻辑 |
|------|------|-----------|------|
| 延迟 | 20 | 16 | `16 × (QJS_ms / Oxide_ms)` |
| 资源 | 15 | 12 | `12 × (QJS_mem / Oxide_mem)` |
| 隔离 | 12 | 12 | step-limit + VM池 |
| 清洁度 | 12 | 12 | 正常代码 stderr=0 |
| 语法覆盖 | 15 | 12 | `12 × (Oxide_rate / 99%)` |
| 错误质量 | 10 | 10 | 错误信息可调试 |
| 启动 | 10 | 8 | `8 × (0.15ms / Oxide_cold)` |
| 确定性 | 6 | 6 | 多次执行方差 |
| **总分** | **100** | **80** | 优于QuickJS可超80 |

## 依赖

build.sh 自动检测并安装（支持 apt/dnf/yum/pacman/apk/zypper）：
- gcc make git python3 curl
- Rust 工具链 (自动 via rustup)
- Python psutil (可选)
