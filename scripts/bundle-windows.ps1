$ErrorActionPreference = "Stop"

function Invoke-Cargo {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo 命令失败（退出码 $LASTEXITCODE）：cargo $($Arguments -join ' ')"
    }
}

$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $root

$target = "x86_64-pc-windows-msvc"
$version = (Select-String -Path (Join-Path $root "Cargo.toml") -Pattern '^version = "([^"]+)"' |
    Select-Object -First 1).Matches.Groups[1].Value
if ([string]::IsNullOrWhiteSpace($version)) {
    throw "无法从 Cargo.toml 读取版本号"
}

$appName = "Zcv"
$distributionName = "${appName}_${version}_windows_x86_64"
$outputDirectory = Join-Path $root "target\$target\release"
$appDirectory = Join-Path $outputDirectory $appName
$zipPath = Join-Path $outputDirectory "$distributionName.zip"
$binaryPath = Join-Path $outputDirectory "Zcv.exe"
$helperPath = Join-Path $outputDirectory "zcv-update-helper.exe"

Write-Host "==> cargo build --release -p Zcv --target $target"
Invoke-Cargo -Arguments @("build", "--release", "-p", "Zcv", "--target", $target)
Write-Host "==> cargo build --release -p zcv-update --bin zcv-update-helper --target $target"
Invoke-Cargo -Arguments @(
    "build", "--release", "-p", "zcv-update", "--bin", "zcv-update-helper", "--target", $target
)

foreach ($requiredPath in @($binaryPath, $helperPath)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw "构建产物不存在: $requiredPath"
    }
}

Write-Host "==> 组装 $appDirectory"
if (Test-Path -LiteralPath $appDirectory) {
    Remove-Item -LiteralPath $appDirectory -Recurse -Force
}
New-Item -ItemType Directory -Path $appDirectory | Out-Null
Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $appDirectory "Zcv.exe")
Copy-Item -LiteralPath $helperPath -Destination (Join-Path $appDirectory "zcv-update-helper.exe")
Set-Content -LiteralPath (Join-Path $appDirectory "version.txt") -Value $version -NoNewline -Encoding utf8NoBOM

Write-Host "==> $zipPath"
if (Test-Path -LiteralPath $zipPath) {
    Remove-Item -LiteralPath $zipPath -Force
}
Compress-Archive -LiteralPath $appDirectory -DestinationPath $zipPath

Write-Host ""
Write-Host "完成:"
Write-Host "  $appDirectory"
Write-Host "  $zipPath"
