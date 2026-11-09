#Requires -Version 7
# Windows(x86_64-msvc) release 构建 + NSIS 安装包;对应 Linux 的 scripts/build-release.sh 三步。
# 本机总内存 7.9G,固定 -j3 控编译并发;mem-guard(ulimit -v)在 Windows 不适用,弃用。
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath (Split-Path -Parent $PSScriptRoot)

python3 scripts/check-capabilities.py
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo build --release --locked -j3
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo build --release --locked --manifest-path ../qaqh-backend/Cargo.toml --target-dir "$PWD/target" -p qaqh-daemon -j3
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo packager --release --formats nsis
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
