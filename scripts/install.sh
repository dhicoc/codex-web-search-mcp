#!/usr/bin/env bash
# codex-web-search-mcp — one-line installer
#
# Downloads the correct prebuilt binary for your platform from GitHub Releases,
# verifies its SHA-256 against checksums.txt, installs it, and prints the MCP
# config snippet. No Rust / Node required.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/dhicoc/codex-web-search-mcp/main/scripts/install.sh | bash
#   bash scripts/install.sh [--version v2.1.0] [--install-dir ~/.local/bin] [--write-config] [--repo owner/name]
#
# Flags:
#   --version <tag>      Release tag to fetch (default: latest)
#   --install-dir <dir>  Where to put the binary (default: ~/.local/bin on unix, ~ on Windows)
#   --write-config       Write a .mcp.json (Claude Code project config) into the CWD if none exists
#   --repo <owner/name>  Override the GitHub repo (default: dhicoc/codex-web-search-mcp)
#   --help               Show this help

set -euo pipefail

REPO="dhicoc/codex-web-search-mcp"
VERSION=""
INSTALL_DIR=""
WRITE_CONFIG=0

for arg in "$@"; do
  case "$arg" in
    --version) shift; VERSION="$1";;
    --install-dir) shift; INSTALL_DIR="$1";;
    --write-config) WRITE_CONFIG=1;;
    --repo) shift; REPO="$1";;
    --help|-h) sed -n '2,18p' "$0"; exit 0;;
  esac
done

# ---- detect platform -------------------------------------------------------
uname_s="$(uname -s 2>/dev/null || echo unknown)"
uname_m="$(uname -m 2>/dev/null || echo unknown)"
lowers="$(echo "$uname_s" | tr '[:upper:]' '[:lower:]')"

case "$lowers" in
  linux*)
    case "$uname_m" in
      x86_64|amd64) PLATFORM="linux-x64";;
      aarch64|arm64) PLATFORM="linux-arm64";;
      *) echo "不支持的架构: $uname_m" >&2; exit 1;;
    esac
    EXE=""
    ;;
  darwin*|*)
    if [ "$lowers" = "darwin" ]; then
      PLATFORM="darwin-universal"
      EXE=""
    else
      echo "无法识别的系统: $uname_s" >&2; exit 1
    fi
    ;;
esac

# Windows under Git Bash / MSYS / Cygwin
if echo "$lowers" | grep -qiE 'mingw|msys|cygwin'; then
  case "$uname_m" in
    x86_64|amd64) PLATFORM="win32-x64";;
    aarch64|arm64|ARM64) PLATFORM="win32-arm64";;
    *) echo "不支持的架构: $uname_m" >&2; exit 1;;
  esac
  EXE=".exe"
fi

# ---- resolve version -------------------------------------------------------
if [ -z "$VERSION" ]; then
  echo "查询最新版本…"
  VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
  if [ -z "$VERSION" ]; then
    echo "无法获取最新版本，请用 --version 指定，例如 v2.1.0" >&2; exit 1
  fi
fi
echo "平台: $PLATFORM   版本: $VERSION"

# ---- install dir -----------------------------------------------------------
if [ -z "$INSTALL_DIR" ]; then
  if [ -n "$EXE" ]; then
    INSTALL_DIR="$HOME/codex-web-search-mcp"
  else
    INSTALL_DIR="$HOME/.local/bin"
  fi
fi
mkdir -p "$INSTALL_DIR"

ASSET="codex-web-search-mcp-$PLATFORM$EXE"
URL="https://github.com/$REPO/releases/download/$VERSION/$ASSET"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "下载 $URL"
curl -fsSL "$URL" -o "$TMP/$ASSET"

# ---- checksum verify (best-effort) ----------------------------------------
if curl -fsSL "https://github.com/$REPO/releases/download/$VERSION/checksums.txt" -o "$TMP/checksums.txt" 2>/dev/null; then
  EXPECTED="$(grep -E "([[:space:]]|/)$ASSET\$" "$TMP/checksums.txt" | awk '{print $1}' | head -n1)"
  if [ -n "$EXPECTED" ]; then
    ACTUAL="$(sha256sum "$TMP/$ASSET" | awk '{print $1}')"
    if [ "$EXPECTED" = "$ACTUAL" ]; then
      echo "✓ SHA-256 校验通过"
    else
      echo "✗ SHA-256 校验失败！预期 $EXPECTED 实得 $ACTUAL" >&2
      echo "  二进制可能已被篡改，已中止安装。" >&2
      exit 1
    fi
  fi
else
  echo "（未找到 checksums.txt，跳过校验）"
fi

# ---- install ---------------------------------------------------------------
DEST="$INSTALL_DIR/codex-web-search-mcp$EXE"
install -m 0755 "$TMP/$ASSET" "$DEST" 2>/dev/null || cp "$TMP/$ASSET" "$DEST"
if [ -z "$EXE" ]; then chmod +x "$DEST"; fi
echo "✓ 已安装到 $DEST"

# ---- PATH hint -------------------------------------------------------------
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "提示: $INSTALL_DIR 不在 PATH 中，可加 export PATH=\"\$PATH:$INSTALL_DIR\" 到 shell 配置。" ;;
esac

# ---- MCP config ------------------------------------------------------------
if [ "$WRITE_CONFIG" -eq 1 ] && [ ! -f ".mcp.json" ]; then
  cat > .mcp.json <<JSON
{
  "mcpServers": {
    "codex-web-search": {
      "command": "$DEST"
    }
  }
}
JSON
  echo "✓ 已写入 .mcp.json（当前目录），重启 Claude Code 即生效"
fi

echo
echo "MCP 配置片段（写入客户端 mcpServers）："
echo '{'
echo "  \"mcpServers\": {"
echo "    \"codex-web-search\": {"
echo "      \"command\": \"$DEST\""
echo "    }"
echo "  }"
echo '}'
echo
echo "提示：本工具需要 Codex 登录态，先运行 \`codex login\`（或设置 CODEX_ACCESS_TOKEN）。"
