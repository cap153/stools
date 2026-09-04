# extras/windows/install_shortcuts.ps1
#
# Generates system-action shortcuts into the Start Menu.
# Automatically cleans up old shortcuts and self-heals encoding issues.
#
# Usage: Right-click -> "Run with PowerShell"

# 1. 确保终端输出使用 UTF-8
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$WshShell = New-Object -ComObject WScript.Shell
$TargetDir = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\stools Commands"

# 2. 🎯 自动清理旧的快捷方式（包括之前的乱码文件）
if (Test-Path $TargetDir) {
    Write-Host "正在清理旧的快捷方式..." -ForegroundColor Yellow
    Get-ChildItem -Path $TargetDir -Filter "*.lnk" | Remove-Item -Force -ErrorAction SilentlyContinue
} else {
    New-Item -ItemType Directory -Path $TargetDir -Force | Out-Null
}

# 3. 🎯 双重保险：自动纠正 PowerShell 5.1 误把 UTF-8 当 GBK 解析的乱码
function Repair-Mojibake([string]$str) {
    # 如果字符串命中了 UTF-8 错读为 GBK 时的特征乱码字（如 鍏、闁、ç）
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

$Shortcuts = @(
    @{
        Name   = "关机 (Power Off)"
        Target = "shutdown.exe"
        Args   = "/s /t 0"
        Icon   = "$env:SystemRoot\System32\shell32.dll,27"
    },
    @{
        Name   = "重启 (Reboot)"
        Target = "shutdown.exe"
        Args   = "/r /t 0"
        Icon   = "$env:SystemRoot\System32\shell32.dll,238"
    },
    @{
        Name   = "清空回收站 (Clear Recycle Bin)"
        Target = "powershell.exe"
        Args   = "-WindowStyle Hidden -Command Clear-RecycleBin -Force"
        Icon   = "$env:SystemRoot\System32\shell32.dll,31"
    },
    @{
        Name   = "打开回收站 (Recycle Bin)"
        Target = "explorer.exe"
        Args   = "shell:RecycleBinFolder"
        Icon   = "$env:SystemRoot\System32\shell32.dll,32"
    },
    @{
        Name   = "锁定屏幕 (Lock Screen)"
        Target = "rundll32.exe"
        Args   = "user32.dll,LockWorkStation"
        Icon   = "$env:SystemRoot\System32\shell32.dll,47"
    }
)

foreach ($sc in $Shortcuts) {
    $cleanName = Repair-Mojibake $sc.Name
    $LnkPath = Join-Path $TargetDir "$cleanName.lnk"
    
    $Lnk = $WshShell.CreateShortcut($LnkPath)
    $Lnk.TargetPath    = $sc.Target
    $Lnk.Arguments     = $sc.Args
    $Lnk.IconLocation = $sc.Icon
    $Lnk.WindowStyle   = 7  # Minimized
    $Lnk.Save()
}

Write-Host "已成功安装快捷方式至开始菜单：$TargetDir" -ForegroundColor Green