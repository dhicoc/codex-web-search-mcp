#!/usr/bin/env bash
# 本机编译辅助：注入 Visual Studio 2023 的 MSVC 环境后 cargo build。
# 用法： bash scripts/build.sh [--release]
#
# 关键坑（本机 Windows 已验证）：
#   MSVC 的 link.exe / cl.exe 只认「反斜杠 + C:\ 盘符」的 INCLUDE/LIB 路径。
#   用正斜杠 "/" 或 "/c/" 风格会导致 LNK1181: 无法打开输入文件“kernel32.lib”。
#   因此这里 INCLUDE/LIB 一律用 C:\... 反斜杠；PATH 用 /c/... 让 bash 能找到 link.exe。
#   注意：本沙箱禁止从 Bash/PowerShell 调用 cmd.exe，所以不能用 `cmd /c vcvarsall.bat`，
#   只能像下面这样直接导出 VS 环境变量。
set -e

VS_VER="14.51.36231"
SDKVER=$(ls "/c/Program Files (x86)/Windows Kits/10/Include" 2>/dev/null | grep -E '^[0-9]' | sort | tail -1)

export PATH="/c/Users/DELL/.cargo/bin:/c/Program Files/Microsoft Visual Studio/18/Community/VC/Tools/MSVC/$VS_VER/bin/Hostx64/x64:$PATH"
export INCLUDE="C:\\Program Files\\Microsoft Visual Studio\\18\\Community\\VC\\Tools\\MSVC\\$VS_VER\\include;C:\\Program Files (x86)\\Windows Kits\\10\\Include\\$SDKVER\\um;C:\\Program Files (x86)\\Windows Kits\\10\\Include\\$SDKVER\\shared;C:\\Program Files (x86)\\Windows Kits\\10\\Include\\$SDKVER\\ucrt"
export LIB="C:\\Program Files\\Microsoft Visual Studio\\18\\Community\\VC\\Tools\\MSVC\\$VS_VER\\lib\\x64;C:\\Program Files (x86)\\Windows Kits\\10\\Lib\\$SDKVER\\um\\x64;C:\\Program Files (x86)\\Windows Kits\\10\\Lib\\$SDKVER\\ucrt\\x64"

cd "$(dirname "$0")/.."
cargo build "$@"
