# extras/windows/install_shortcuts.ps1
#
# Generates system-action shortcuts into the Start Menu. Each shortcut carries a
# native shell32.dll icon and a bilingual name, so it shows up in stools whether
# you search "关机" or "poweroff".
#
# Usage: right-click -> "Run with PowerShell", or from a terminal:
#     powershell -ExecutionPolicy Bypass -File extras/windows/install_shortcuts.ps1

$WshShell = New-Object -ComObject WScript.Shell
$TargetDir = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\stools Commands"
if (!(Test-Path $TargetDir)) { New-Item -ItemType Directory -Path $TargetDir | Out-Null }

# Each entry: bilingual display name, the target program, its arguments, and a
# shell32.dll icon index (see https://ss64.com/nt/shell32.html for the gallery).
$Shortcuts = @(
    @{
        Name  = "关机 (Power Off)"
        Target = "shutdown.exe"
        Args  = "/s /t 0"
        Icon  = "$env:SystemRoot\System32\shell32.dll,27"
    },
    @{
        Name  = "重启 (Reboot)"
        Target = "shutdown.exe"
        Args  = "/r /t 0"
        Icon  = "$env:SystemRoot\System32\shell32.dll,238"
    },
    @{
        Name  = "清空回收站 (Clear Recycle Bin)"
        Target = "powershell.exe"
        Args  = "-WindowStyle Hidden -Command `"Clear-RecycleBin -Force`""
        Icon  = "$env:SystemRoot\System32\shell32.dll,31"
    },
    @{
        Name  = "打开回收站 (Recycle Bin)"
        Target = "explorer.exe"
        Args  = "shell:RecycleBinFolder"
        Icon  = "$env:SystemRoot\System32\shell32.dll,32"
    },
    @{
        Name  = "锁定屏幕 (Lock Screen)"
        Target = "rundll32.exe"
        Args  = "user32.dll,LockWorkStation"
        Icon  = "$env:SystemRoot\System32\shell32.dll,47"
    }
)

foreach ($sc in $Shortcuts) {
    $Lnk = $WshShell.CreateShortcut("$TargetDir\$($sc.Name).lnk")
    $Lnk.TargetPath    = $sc.Target
    $Lnk.Arguments     = $sc.Args
    $Lnk.IconLocation = $sc.Icon
    $Lnk.WindowStyle   = 7  # minimized
    $Lnk.Save()
}

Write-Host "已成功安装快捷方式至开始菜单：$TargetDir" -ForegroundColor Green
