# 查询占用 CPU 的进程
function Get-BusyProcess {
    param([int]$Minimum = 10)
    Get-Process | Where-Object { $_.CPU -gt $Minimum }
}

Get-BusyProcess -Minimum 20
