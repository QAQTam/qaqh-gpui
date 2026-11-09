# QAQHarness 构建与打包入口:Windows 走 pwsh7,Linux 走 bash。
# recipe 体保持 sh/pwsh 双兼容(纯命令行,无变量展开差异);平台分派用属性切分。
set windows-shell := ["pwsh.exe", "-NoLogo", "-Command"]

# 列出全部 recipe
default:
    @just --list

# 开发期快速检查(dev profile,限并发控内存)
check *ARGS:
    cargo check --locked -j3 {{ARGS}}

# 全流程:协议门 → release 构建(client+daemon) → 安装包
[linux]
[macos]
build-release:
    bash scripts/build-release.sh

# 全流程:协议门 → release 构建(client+daemon) → NSIS 安装包
[windows]
build-release:
    pwsh -NoLogo -File scripts/build-release-windows.ps1

# 只装配安装包(跳过构建,需 release exe 已在)
[linux]
[macos]
package:
    python3 scripts/package.py

# 只装配 NSIS 安装包(跳过构建,需 release exe 已在)
[windows]
package:
    cargo packager --release --formats nsis

# 清理构建产物
clean:
    cargo clean
