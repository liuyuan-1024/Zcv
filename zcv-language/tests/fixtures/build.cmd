@echo off
REM 准备输出目录
set "OUT=dist"
if not exist "%OUT%" (
  mkdir "%OUT%"
)
echo Build complete: %OUT%
