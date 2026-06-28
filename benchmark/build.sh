#!/bin/bash
# OxideJS Benchmark Suite — 一键构建
# 兼容: Debian/Ubuntu, RHEL/Fedora, Arch, Alpine, openSUSE
set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'
info()  { echo -e "${GREEN}[ ✓ ]${NC} $*"; }
warn()  { echo -e "${YELLOW}[ ⚠ ]${NC} $*"; }
err()   { echo -e "${RED}[ ✗ ]${NC} $*"; }
step()  { echo -e "\n${BOLD}${CYAN}═══ $* ═══${NC}"; }

# ── 包管理器检测 ─────────────────────────────────────────────────────
detect_pm() {
    if   command -v apt-get &>/dev/null; then echo "apt" "apt-get install -y" "apt-get update -qq"
    elif command -v dnf     &>/dev/null; then echo "dnf" "dnf install -y" "dnf check-update 2>/dev/null || true"
    elif command -v yum     &>/dev/null; then echo "yum" "yum install -y" "yum check-update 2>/dev/null || true"
    elif command -v pacman  &>/dev/null; then echo "pacman" "pacman -S --noconfirm" "pacman -Sy --noconfirm"
    elif command -v apk     &>/dev/null; then echo "apk" "apk add --no-cache" "apk update"
    elif command -v zypper  &>/dev/null; then echo "zypper" "zypper install -y" "zypper refresh"
    else echo "unknown" "" ""
    fi
}

map_pkgs() {
    case "$1" in
        apt)    echo "gcc make git python3 python3-pip curl";;
        dnf|yum) echo "gcc make git python3 python3-pip curl";;
        pacman) echo "gcc make git python python-pip curl";;
        apk)    echo "gcc make git python3 py3-pip curl";;
        zypper) echo "gcc make git python3 python3-pip curl";;
        *)      echo "gcc make git python3 curl";;
    esac
}

install_deps() {
    local pm=$1 icmd=$2 ucmd=$3; shift 3
    local missing=()
    for pkg in "$@"; do
        command -v "$pkg" &>/dev/null && continue
        [ "$pm" = "apt" ] && dpkg -s "$pkg" &>/dev/null 2>&1 && continue
        missing+=("$pkg")
    done
    [ ${#missing[@]} -eq 0 ] && return 0
    info "安装: ${missing[*]}"
    [ "$pm" = "pacman" ] || [ "$pm" = "apt" ] && $ucmd 2>/dev/null
    $icmd "${missing[@]}" 2>&1 | tail -3
}

ensure_rust() {
    step "Rust 工具链"
    command -v rustc &>/dev/null && info "rustc $(rustc --version)" && return 0
    info "正在安装 Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    source "$HOME/.cargo/env" 2>/dev/null || . "$HOME/.cargo/env" 2>/dev/null
}

ensure_python() {
    step "Python 环境"
    info "python3 $(python3 --version 2>&1)"
    python3 -c "import psutil" 2>/dev/null || pip3 install --break-system-packages psutil 2>/dev/null || apt-get install -y python3-psutil 2>/dev/null || true
}

# ── QuickJS ───────────────────────────────────────────────────────────
build_quickjs() {
    step "QuickJS (baseline)"
    local qjs_dir="$REPO_ROOT/baseline-quickjs"
    if [ ! -d "$qjs_dir" ]; then
        warn "baseline-quickjs/ 不存在，跳过 (请放 QuickJS 源码到此目录)"
        return 0
    fi
    cd "$qjs_dir"
    if [ -f build/qjs ] && [ -f build/run-test262 ]; then
        info "QuickJS 已构建"
        return 0
    fi
    info "编译 QuickJS..."
    if make -j"$(nproc 2>/dev/null || echo 4)" all 2>&1; then
        info "QuickJS 编译成功"
    else
        err "QuickJS 编译失败 (rc=$?)"
        cd "$SCRIPT_DIR"
        return 1
    fi
    # QuickJS Makefile 将二进制输出到源根目录，拷贝到 build/ 备用
    cp -f qjs run-test262 build/ 2>/dev/null || true
    cd "$SCRIPT_DIR"
    info "QuickJS 完成"
}

# ── OxideJS ────────────────────────────────────────────────────────────
build_oxide() {
    step "OxideJS"
    cd "$REPO_ROOT"
    if [ -f target/release/oxide ] && [ -f target/release/test262-runner ]; then
        info "OxideJS 已构建"
        return 0
    fi
    info "编译 OxideJS (release, 约 3-6 分钟)..."
    cargo build --release 2>&1 | grep -E "Compiling|Finished|error" || true
    info "OxideJS 完成"
}

# ── test262 ────────────────────────────────────────────────────────────
ensure_test262() {
    step "test262"
    local t262="$REPO_ROOT/tests/test262/test"
    if [ -d "$t262" ]; then
        info "test262 已存在: $(find "$t262" -name '*.js' | wc -l) 测试"
        return 0
    fi
    warn "test262 未找到"
    if [ -f "$SCRIPT_DIR/fetch_test262.sh" ]; then
        cd "$REPO_ROOT/tests" && bash "$SCRIPT_DIR/fetch_test262.sh"
    fi
}

# ── Main ────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}${CYAN}╔══════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}${CYAN}║     OxideJS Benchmark Suite — 构建               ║${NC}"
echo -e "${BOLD}${CYAN}╚══════════════════════════════════════════════════╝${NC}"

step "检测环境"
echo "  OS: $(uname -a 2>/dev/null | cut -d' ' -f1-3)"
echo "  CPU: $(nproc 2>/dev/null || echo '?') cores"
read pm icmd ucmd <<< $(detect_pm)
info "包管理器: $pm"

install_deps "$pm" "$icmd" "$ucmd" $(map_pkgs "$pm")
ensure_rust
ensure_python
build_quickjs
build_oxide
ensure_test262

echo ""
echo -e "${BOLD}${GREEN}╔══════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}${GREEN}║  构建完成! 运行: python3 benchmark/run_benchmark.py ║${NC}"
echo -e "${BOLD}${GREEN}╚══════════════════════════════════════════════════╝${NC}"
