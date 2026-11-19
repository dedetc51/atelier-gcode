param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Binary,
    [Parameter(Mandatory)][string]$Setup
)
# Run on a disposable Windows test account. The script exercises a real per-user installation.
$ErrorActionPreference = 'Stop'
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\AtelierGcode'
$folder = Join-Path $env:LOCALAPPDATA 'Programs\Atelier G-code'
if ((Test-Path $key) -or (Test-Path $folder)) { throw "Le compte de test contient déjà une installation." }
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Atelier G-code.lnk'
$program = Join-Path $folder "atelier-gcode-$Version.exe"
$uninstaller = Join-Path $folder 'Desinstaller.exe'
$scratch = New-Item -ItemType Directory (Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString()))
$oldSetup = Join-Path $scratch 'ancienne.exe'

function Run-Setup($path, $arguments) {
    $process = Start-Process -FilePath $path -ArgumentList $arguments -PassThru -Wait
    return $process.ExitCode
}
function Check-Shortcut($expected, $number) {
    $link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
    if ($link.TargetPath -ne $expected -or $link.Arguments -ne '--mises-a-jour' -or
        (Get-ItemPropertyValue $key DisplayVersion) -ne $number) { throw "Version ou raccourci inattendu." }
}
function Wait-For($condition) {
    for ($i = 0; $i -lt 80; $i++) {
        try { if (& $condition) { return } } catch {}
        Start-Sleep -Milliseconds 250
    }
    throw "Délai dépassé pendant la vérification Windows."
}

$running = $null
try {
    & "$PSScriptRoot\package.ps1" -Version '0.0.1' -Binary $Binary -Output $oldSetup
    if ((Run-Setup $oldSetup '/S') -ne 0) { throw "Première installation impossible." }
    $old = Join-Path $folder 'atelier-gcode-0.0.1.exe'
    Check-Shortcut $old '0.0.1'
    $log = Join-Path $scratch 'atelier.log'
    $running = Start-Process -FilePath $old -ArgumentList '--sans-navigateur' -PassThru -RedirectStandardOutput $log
    Wait-For { (Get-Content $log -Raw) -match 'http://127[.]0[.]0[.]1:[0-9]+/' }
    $url = [regex]::Match((Get-Content $log -Raw), 'http://127[.]0[.]0[.]1:[0-9]+/').Value
    Wait-For { (Invoke-WebRequest -Uri $url -TimeoutSec 2).StatusCode -eq 200 }

    if ((Run-Setup $Setup '/S /UPDATE') -ne 0) { throw "Échec de la mise à jour." }
    Check-Shortcut $program $Version
    if ($running.HasExited -or (Invoke-WebRequest -Uri $url).StatusCode -ne 200) { throw "Session interrompue." }
    if ((Run-Setup $oldSetup '/S') -eq 0) { throw "Retour à une ancienne version accepté." }
    if ((& $program --version) -ne "atelier-gcode $Version") { throw "Exécutable installé inattendu." }
} finally {
    if ($running -and !$running.HasExited) { $running.Kill(); $running.WaitForExit() }
}
if ((Run-Setup $uninstaller '/S') -ne 0) { throw "Échec de désinstallation." }
Wait-For { !(Test-Path $key) -and !(Test-Path $shortcut) -and !(Test-Path $folder) }
if ((Run-Setup $Setup '/S /UPDATE') -eq 0 -or (Test-Path $folder)) { throw "Réinstallation après désinstallation." }
Write-Host "Installation, raccourci, serveur local, mise à jour en cours de session et désinstallation vérifiés."
