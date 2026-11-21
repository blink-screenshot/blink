# Memory use of every running Blink.exe process, over time.
#
#   powershell -ExecutionPolicy Bypass -File tools\memory.ps1                 # every second, until Ctrl+C
#   powershell -ExecutionPolicy Bypass -File tools\memory.ps1 -Seconds 60 -Csv blink-memory.csv
#
# Per process: its role (from the command line), private bytes (PrivateMemorySize64, what
# only this process uses) and the working set (WorkingSet64, also counts shared pages such
# as mapped fonts and DLLs), in MB (1 MB = 1048576 bytes), plus the peak working set.
# Compare versions with the same steps: start the tray, wait, take the same screenshots,
# and note the time of each step.

param(
    [int]$Seconds = 0,          # 0: until Ctrl+C
    [double]$Interval = 1.0,
    [string]$Csv = ""
)

function Role([string]$cmd) {
    if ($cmd -match '--edit-raw|--open|--clipboard') { return 'editor' }
    if ($cmd -match '--settings') { return 'settings' }
    if ($cmd -match '--welcome') { return 'welcome' }
    if ($cmd -match '--update') { return 'update' }
    if ($cmd -match '--record') { return 'recorder' }
    if ($cmd -match '--clip|--export-gif') { return 'clip' }
    if ($cmd -match '--') { return 'other' }
    # Blink.exe with an image file opens the editor; without arguments it is the tray.
    if ($cmd -match '\.exe"?\s+\S') { return 'editor' }
    return 'tray'
}

$mb = 1048576
$start = Get-Date
$rows = @()
while ($true) {
    $now = Get-Date
    $t = [math]::Round(($now - $start).TotalSeconds, 1)
    $procs = Get-CimInstance Win32_Process -Filter "Name = 'Blink.exe'"
    foreach ($p in $procs) {
        $gp = Get-Process -Id $p.ProcessId -ErrorAction SilentlyContinue
        if (-not $gp) { continue }
        $row = [pscustomobject]@{
            Time        = $now.ToString('HH:mm:ss')
            Seconds     = $t
            Pid         = $p.ProcessId
            Role        = Role $p.CommandLine
            PrivateMB   = [math]::Round($gp.PrivateMemorySize64 / $mb, 1)
            WorkingMB   = [math]::Round($gp.WorkingSet64 / $mb, 1)
            PeakWorkMB  = [math]::Round($gp.PeakWorkingSet64 / $mb, 1)
            CommandLine = $p.CommandLine
        }
        $rows += $row
        '{0}  {1,6}s  pid {2,-6} {3,-9} private {4,7} MB   working set {5,7} MB   peak {6,7} MB' -f `
            $row.Time, $row.Seconds, $row.Pid, $row.Role, $row.PrivateMB, $row.WorkingMB, $row.PeakWorkMB
    }
    if (-not $procs) { '{0}  no Blink.exe running' -f $now.ToString('HH:mm:ss') }
    if ($Csv) { $rows | Export-Csv -NoTypeInformation -Path $Csv }
    if ($Seconds -gt 0 -and ($now - $start).TotalSeconds -ge $Seconds) { break }
    Start-Sleep -Milliseconds ([int]($Interval * 1000))
}
