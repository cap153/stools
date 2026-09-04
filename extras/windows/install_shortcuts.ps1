# extras/windows/install_shortcuts.ps1
#
# 为 stools 生成系统快捷指令至当前用户的开始菜单专属目录。
# 自动清理旧快捷方式、自愈编码乱码，并为每个指令定制最合适的窗口启动模式。
#
# 使用方法：右键该文件 -> "使用 PowerShell 运行" (Run with PowerShell)

# 1. 强制终端输出为 UTF-8
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$WshShell = New-Object -ComObject WScript.Shell

# 2. 🎯 目标路径：严格锁定为当前用户的开始菜单 (无需管理员权限，安全隔离)
#    实际对应：C:\Users\<你的用户名>\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\stools Commands
$TargetDir = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\stools Commands"

# 3. 自动清理该目录下的旧快捷方式（旧文件与乱码文件一键清空）
if (Test-Path $TargetDir) {
    Write-Host "正在清理旧的快捷指令..." -ForegroundColor Yellow
    Get-ChildItem -Path $TargetDir -Filter "*.lnk" | Remove-Item -Force -ErrorAction SilentlyContinue
} else {
    New-Item -ItemType Directory -Path $TargetDir -Force | Out-Null
}

# 4. 编码自愈函数（防止 Windows PowerShell 5.1 误把 UTF-8 当 GBK 解析）
function Repair-Mojibake([string]$str) {
    if ($str -match '[鍏闁]') {
        try {
            $bytes = [System.Text.Encoding]::GetEncoding(936).GetBytes($str)
            return [System.Text.Encoding]::UTF8.GetString($bytes)
        } catch {
            return $str
        }
    }
    return $str
}

# 5. 指令清单：WindowStyle: 1=正常前台窗口 (Normal), 7=最小化后台执行 (Minimized)
$Shortcuts = @(
    @{
        Name        = "关机 (Power Off)"
        Target      = "shutdown.exe"
        Args        = "/s /t 0"
        Icon        = "$env:SystemRoot\System32\shell32.dll,27"
        WindowStyle = 7  # 最小化静默执行
    },
    @{
        Name        = "重启 (Reboot)"
        Target      = "shutdown.exe"
        Args        = "/r /t 0"
        Icon        = "$env:SystemRoot\System32\shell32.dll,238"
        WindowStyle = 7  # 最小化静默执行
    },
    @{
        Name        = "清空回收站 (Clear Recycle Bin)"
        Target      = "powershell.exe"
        Args        = "-WindowStyle Hidden -Command Clear-RecycleBin -Force"
        Icon        = "$env:SystemRoot\System32\shell32.dll,31"
        WindowStyle = 7  # 最小化+隐藏，彻底无黑框
    },
    @{
        Name        = "打开回收站 (Recycle Bin)"
        Target      = "explorer.exe"
        Args        = "shell:RecycleBinFolder"
        Icon        = "$env:SystemRoot\System32\shell32.dll,32"
        WindowStyle = 1  # 🎯 关键修改：正常前台激活资源管理器窗口！
    },
    @{
        Name        = "锁定屏幕 (Lock Screen)"
        Target      = "rundll32.exe"
        Args        = "user32.dll,LockWorkStation"
        Icon        = "$env:SystemRoot\System32\shell32.dll,47"
        WindowStyle = 7  # 最小化静默执行
    }
)

foreach ($sc in $Shortcuts) {
    $cleanName = Repair-Mojibake $sc.Name
    $LnkPath = Join-Path $TargetDir "$cleanName.lnk"

    $Lnk = $WshShell.CreateShortcut($LnkPath)
    $Lnk.TargetPath    = $sc.Target
    $Lnk.Arguments     = $sc.Args
    $Lnk.IconLocation = $sc.Icon
    $Lnk.WindowStyle   = $sc.WindowStyle  # 🎯 针对每个项目应用独立的窗口行为
    $Lnk.Save()
}

Write-Host "已成功安装快捷指令至当前用户开始菜单：" -ForegroundColor Green
Write-Host "$TargetDir" -ForegroundColor Cyan