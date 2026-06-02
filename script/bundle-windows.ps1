# 打包 Windows 分发 zip。
# Release 资产命名: zom_0.1.0_windows_amd64.zip / zom_0.1.0_windows_arm64.zip
#
# 所有资源（含应用图标 SVG）都通过 include_bytes! 编译进 zom.exe，
# 因此分发包就是单文件可执行 + 一个 zip。
#
# 用法:
#   pwsh script/bundle-windows.ps1
#   pwsh script/bundle-windows.ps1 -Arch amd64
#   pwsh script/bundle-windows.ps1 -Arch arm64
#
# 注意: 文件开头有 UTF-8 BOM，Windows PowerShell 5.1 才能正确读中文。
#       编辑保存时请保留 BOM（VSCode/Zed 默认会保留）。

param(
    [ValidateSet("amd64", "arm64", "x64", "x86_64")]
    [string]$Arch = "amd64"
)

$ErrorActionPreference = "Stop"
$OutputEncoding = [System.Text.Encoding]::UTF8
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $Root

$AppName = "zom"
$BinName = "zom"
$Package = "zom-desktop"

switch ($Arch) {
    "amd64"  { $Target = "x86_64-pc-windows-msvc"; $ReleaseArch = "amd64" }
    "x64"    { $Target = "x86_64-pc-windows-msvc"; $ReleaseArch = "amd64" }
    "x86_64" { $Target = "x86_64-pc-windows-msvc"; $ReleaseArch = "amd64" }
    "arm64"  { $Target = "aarch64-pc-windows-msvc"; $ReleaseArch = "arm64" }
}

$Version = (Select-String -Path "Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches.Groups[1].Value

$OS = "windows"
$DistName = "${AppName}_${Version}_${OS}_${ReleaseArch}"

# ---------- 1. 构建 ----------
Write-Host "==> cargo build --release -p $Package --target $Target"
cargo build --release -p $Package --target $Target
$OutDir = "target/$Target/release"

$Exe = Join-Path $OutDir "$BinName.exe"
if (-not (Test-Path $Exe)) {
    throw "构建产物不存在: $Exe"
}

# ---------- 2. 打 zip ----------
$Stage = Join-Path $OutDir $DistName
$Zip   = Join-Path $OutDir "$DistName.zip"

if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
if (Test-Path $Zip)   { Remove-Item -Force $Zip }
New-Item -ItemType Directory -Path $Stage | Out-Null

Copy-Item $Exe (Join-Path $Stage "$BinName.exe")

# 第三方字体许可声明随包带上
$LicSrc = "zom-desktop/assets/icons/LICENSES.txt"
if (Test-Path $LicSrc) {
    Copy-Item $LicSrc (Join-Path $Stage "LICENSES.txt")
}

Write-Host "==> $Zip"
Compress-Archive -Path "$Stage/*" -DestinationPath $Zip

Write-Host ""
Write-Host "完成:"
Write-Host "  $Zip"
Write-Host ""
Write-Host "朋友首次运行时 SmartScreen 会提示 [未识别的应用],"
Write-Host "点 [更多信息] -> [仍要运行] 即可。"
